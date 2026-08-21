//! `update_action_item`: tick or untick one action item, from chat.

use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};

use kodabi_core::ledger::{ClosedVia, EntryState, Ledger, LedgerEntry, LedgerError};
use kodabi_core::note::NoteId;
use kodabi_core::vault::{self, SetDoneOutcome};

use super::map_ledger_error;
use crate::envelope;
use crate::protocol::RpcError;
use crate::server::Server;

/// `update_action_item` arguments.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateActionItemParams {
    note_id: String,
    item_id: String,
    done: bool,
}

/// Marks an action item done (or reopens it), writing **both** halves of what
/// that means: the checkbox in the note, and the judgement in the ledger.
///
/// The two are genuinely different facts. The note's `- [x]` is the source of
/// truth for done/not-done — the ledger has no `done` column at all — while the
/// ledger records what a checkbox cannot spell: that this was closed, and by
/// whom. Writing only the box would leave the Commitments view still listing
/// the item as open, which is exactly the chat/view disagreement this tool
/// exists to prevent.
///
/// **Note first, then the ledger**, matching the desktop `set_commitment_done`
/// command: a partial failure that leaves the box ticked and the entry open is
/// legible and self-correcting (the next sync sees the tick), while the reverse
/// would show a commitment closed against a line that still reads open.
///
/// Convergence needs no extra machinery: the note write is a `.md` write, so the
/// open desktop app's file watcher observes it, reconciles, and emits
/// `vault:changed`, at which point every window refetches its commitments
/// through the app's own ledger handle and sees this process's committed close.
/// This server has no `AppHandle` and could not emit an event itself.
///
/// Idempotent in both directions: an already-ticked box and an already-closed
/// entry are both ordinary successes, never errors.
///
/// Two cases write the box and leave the ledger alone, and both are ordinary
/// answers rather than faults: an item the enrollment gate never enrolled (no
/// entry at all, reported as `entry: null`), and one the person untracked by
/// hand (reported with its untracked state). In both, the ledger has nothing to
/// say about the commitment and the checkbox is the whole truth.
pub fn call(server: &Server, arguments: Value) -> Result<Value, RpcError> {
    let backend = server.backend()?;
    let params: UpdateActionItemParams = serde_json::from_value(arguments).map_err(|error| {
        RpcError::invalid_params(format!("invalid update_action_item arguments: {error}"))
    })?;

    let note_id = NoteId::parse(&params.note_id)
        .map_err(|_| RpcError::invalid_params(format!("invalid note id: {:?}", params.note_id)))?;

    // Open the ledger *before* touching the vault. A configuration fault must
    // not leave a ticked box behind with nothing recorded against it — the same
    // refusal the desktop command makes when its ledger is unavailable.
    let mut ledger = backend.open_ledger()?;

    let kb_root = &backend.config.kb_root;
    let outcome = vault::set_action_item_done(kb_root, &note_id, &params.item_id, params.done)
        .map_err(|error| RpcError::internal(error.to_string()))?;

    // The entry is found through the *live* ref, which survives the line being
    // edited away — so a note or item that has moved on does not doom the
    // judgement half.
    let entry = ledger
        .entry_for_item(note_id.as_str(), &params.item_id)
        .map_err(|error| RpcError::internal(error.to_string()))?;

    match (&outcome, &entry) {
        // Nothing was written and nothing is tracked: the caller named something
        // this vault does not have, which is a business fault it should reason
        // about rather than a protocol error.
        (SetDoneOutcome::NoteMissing, None) => {
            return Ok(envelope::business_error(format!(
                "note not found: {}",
                params.note_id
            )));
        }
        (SetDoneOutcome::ItemMissing, None) => {
            return Ok(envelope::business_error(format!(
                "action item not found in {}: {}",
                params.note_id, params.item_id
            )));
        }
        _ => {}
    }

    let entry_payload = match entry {
        // An untracked entry is one the person took *out* of the working set,
        // and its item refs deliberately stay live. There is no judgement to
        // record against it, so the checkbox is written and the ledger is left
        // exactly as they set it — reported back so the caller can see why no
        // commitment moved. Attempting a close here would fail the state
        // machine and report an error for a tick that plainly succeeded.
        Some(entry) if entry.state == EntryState::Untracked => json!({
            "entry_id": entry.entry_id,
            "state": entry.state,
            "closed_via": entry.closed_via.map(|via| via.as_str().to_string()),
            "updated_at": entry.updated_at,
        }),
        Some(entry) => {
            // The clock is read here, never in kodabi-core.
            let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
            let updated =
                apply(&mut ledger, &entry.entry_id, params.done, &now).map_err(map_ledger_error)?;
            match updated {
                Ok(updated) => {
                    // Records that a person judged this, not a machine — the
                    // same flag the desktop mutation sets, and what keeps a
                    // chat-made close from reading as an automated one.
                    ledger
                        .mark_touched(&updated.entry_id)
                        .map_err(|error| RpcError::internal(error.to_string()))?;

                    // The vault's YAML mirror of the ledger is written by
                    // whichever process dirtied it; the app's worker has no idea
                    // this happened. Best-effort: the database is the durable
                    // record, and a stale snapshot only matters if the ledger is
                    // ever rebuilt from empty.
                    if let Err(error) = ledger.flush_snapshots(kb_root) {
                        eprintln!("kodabi-mcp: ledger snapshot flush failed: {error}");
                    }

                    json!({
                        "entry_id": updated.entry_id,
                        "state": updated.state,
                        "closed_via": updated.closed_via.map(|via| via.as_str().to_string()),
                        "updated_at": updated.updated_at,
                    })
                }
                Err(refused) => return Ok(envelope::business_error(refused)),
            }
        }
        None => Value::Null,
    };

    let payload = json!({
        "source": { "id": params.note_id, "item_id": params.item_id },
        "done": params.done,
        "note_outcome": note_outcome(&outcome),
        "note_updated": matches!(outcome, SetDoneOutcome::Updated(_)),
        "entry": entry_payload,
    });
    Ok(envelope::success(&payload))
}

/// Moves the entry, or explains in the caller's terms why it will not move.
///
/// A refusal here is a *business* answer rather than a fault: "you cannot close
/// something you already waived" is something the model can act on (by
/// reopening first), and the state machine's own message names both states.
#[allow(clippy::type_complexity)]
fn apply(
    ledger: &mut Ledger,
    entry_id: &str,
    done: bool,
    now: &str,
) -> Result<Result<LedgerEntry, String>, LedgerError> {
    let result = if done {
        ledger.close(entry_id, ClosedVia::Manual, now)
    } else {
        ledger.reopen(entry_id, now)
    };
    match result {
        Ok(entry) => Ok(Ok(entry)),
        Err(LedgerError::IllegalTransition { entry_id, from, to }) => Ok(Err(format!(
            "cannot move commitment {entry_id} from {from} to {to}"
        ))),
        Err(other) => Err(other),
    }
}

/// The wire spelling of what happened to the note file.
fn note_outcome(outcome: &SetDoneOutcome) -> &'static str {
    match outcome {
        SetDoneOutcome::Updated(_) => "updated",
        SetDoneOutcome::AlreadySet => "already_set",
        SetDoneOutcome::NoteMissing => "note_missing",
        SetDoneOutcome::ItemMissing => "item_missing",
    }
}
