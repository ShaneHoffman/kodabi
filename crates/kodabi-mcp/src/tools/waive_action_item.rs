//! `waive_action_item`: set a commitment aside as not happening, from chat.

use chrono::{Local, SecondsFormat, Utc};
use serde_json::{json, Value};

use kodabi_core::ledger::{EntryState, Ledger, LedgerEntry, LedgerError};
use kodabi_core::note::NoteId;
use kodabi_core::vault::{self, AnnotateOutcome};

use super::map_ledger_error;
use crate::envelope;
use crate::protocol::RpcError;
use crate::server::Server;

/// `waive_action_item` arguments.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WaiveActionItemParams {
    note_id: String,
    item_id: String,
}

/// Waives a tracked commitment: records in the ledger that it is deliberately
/// not happening, and writes a date-only line under the item saying so.
///
/// Waiving is the verb for a commitment that was real and stopped mattering —
/// distinct from ticking it (which claims it was done) and from untracking it
/// (which claims it was never this ledger's business). **The checkbox is never
/// touched**: waived is not done, and the annotation is the signal precisely
/// because a tick would say the opposite.
///
/// **The ledger moves first, then the note** — the reverse of
/// `update_action_item`, and for a reason worth stating. There the note's
/// `- [x]` *is* the source of truth, so writing it first leaves any partial
/// failure self-correcting. Here the ledger row is the fact and the line is its
/// echo, while the transition table is a real gate: a closed, superseded or
/// untracked entry cannot be waived. Writing the note first would risk a line
/// announcing a waive the ledger then refused, which is a lie in the file the
/// person trusts most.
///
/// Convergence rides that note write, as it does for a tick: the `.md` change
/// is what the open desktop app's watcher sees, and its reconcile — which never
/// reads these lines, they are inert to the grammar — emits `vault:changed`, at
/// which point every window refetches through the app's own ledger handle and
/// sees the row this process committed. This server has no `AppHandle` and
/// could not emit an event itself. When the annotation is skipped or fails, no
/// file event fires and an open window converges on its next one; the database
/// is the durable record either way.
///
/// Idempotent: waiving something already waived is an ordinary success that
/// writes nothing at all. It deliberately does **not** backfill a missing line
/// — stamping today's date onto a judgement made last month would misdate it —
/// so an annotation lost to a failed write stays lost, and the ledger remains
/// the record.
///
/// Reopening (`update_action_item` with `done: false`) undoes the state but
/// leaves the line: annotate, never destroy. The reversibility this tool claims
/// is a claim about the ledger, and the dated line stays true as a record of
/// the day the waive was made.
///
/// One asymmetry, shared with `update_action_item`: a commitment whose source
/// line has been edited away has no `(note_id, item_id)` to name it by, and can
/// only be waived in the app, by entry id.
pub fn call(server: &Server, arguments: Value) -> Result<Value, RpcError> {
    let backend = server.backend()?;
    let params: WaiveActionItemParams = serde_json::from_value(arguments).map_err(|error| {
        RpcError::invalid_params(format!("invalid waive_action_item arguments: {error}"))
    })?;

    let note_id = NoteId::parse(&params.note_id)
        .map_err(|_| RpcError::invalid_params(format!("invalid note id: {:?}", params.note_id)))?;

    // The ledger is the first write, so opening it first needs no separate
    // argument: an unset `KODABI_LEDGER_DB` refuses before anything happens.
    let mut ledger = backend.open_ledger()?;
    let kb_root = &backend.config.kb_root;

    // Found through the *live* ref, the same handle a tick uses.
    let entry = ledger
        .entry_for_item(note_id.as_str(), &params.item_id)
        .map_err(|error| RpcError::internal(error.to_string()))?;

    // Unlike a tick, a waive has no half it can perform against an item the
    // ledger does not track: there is no note-side fact to record on its own,
    // so this is a business answer *before* anything is written.
    let Some(entry) = entry else {
        return Ok(envelope::business_error(format!(
            "no tracked commitment for {}/{}: the item was never enrolled, or its source line has moved on",
            params.note_id, params.item_id
        )));
    };

    if entry.state == EntryState::Waived {
        return Ok(envelope::success(&payload(
            &params,
            &entry,
            false,
            "already_waived",
        )));
    }

    // The clock is read here, never in kodabi-core.
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let waived = match apply(&mut ledger, &entry.entry_id, &now).map_err(map_ledger_error)? {
        Ok(waived) => waived,
        Err(refused) => return Ok(envelope::business_error(refused)),
    };

    // Records that a person judged this, not a machine.
    ledger
        .mark_touched(&waived.entry_id)
        .map_err(|error| RpcError::internal(error.to_string()))?;

    // The vault's YAML mirror is written by whichever process dirtied it; the
    // app's worker has no idea this happened. Best-effort: the database is the
    // durable record, and a stale snapshot only matters on a rebuild.
    if let Err(error) = ledger.flush_snapshots(kb_root) {
        eprintln!("kodabi-mcp: ledger snapshot flush failed: {error}");
    }

    // A local calendar day, matching every other annotation this vault carries:
    // the line files under the day the person made the call, not under UTC's.
    let waived_on = Local::now().format("%Y-%m-%d").to_string();
    let annotated =
        vault::annotate_action_item_waived(kb_root, &note_id, &params.item_id, &waived_on);

    // Never a fault once the ledger has committed: reporting failure for a
    // waive that succeeded would invite a retry that cannot converge, since a
    // second call now takes the already-waived path and writes nothing.
    let note_outcome = match annotated {
        Ok(AnnotateOutcome::Annotated(_)) => "annotated",
        Ok(AnnotateOutcome::AlreadyAnnotated) => "already_annotated",
        Ok(AnnotateOutcome::NoteMissing) => "note_missing",
        Ok(AnnotateOutcome::ItemMissing) => "item_missing",
        Err(error) => {
            eprintln!("kodabi-mcp: waive annotation failed: {error}");
            "failed"
        }
    };

    Ok(envelope::success(&payload(
        &params,
        &waived,
        note_outcome == "annotated",
        note_outcome,
    )))
}

/// Waives the entry, or explains in the caller's terms why it will not move.
///
/// A refusal is a *business* answer rather than a fault: "you cannot waive
/// something you already closed" is something the model can act on (by
/// reopening first), and the state machine's own message names both states.
/// This one arm covers closed, superseded and untracked alike, which is why the
/// handler needs no per-state pre-checks.
#[allow(clippy::type_complexity)]
fn apply(
    ledger: &mut Ledger,
    entry_id: &str,
    now: &str,
) -> Result<Result<LedgerEntry, String>, LedgerError> {
    match ledger.waive(entry_id, now) {
        Ok(entry) => Ok(Ok(entry)),
        Err(LedgerError::IllegalTransition { entry_id, from, to }) => Ok(Err(format!(
            "cannot move commitment {entry_id} from {from} to {to}"
        ))),
        Err(other) => Err(other),
    }
}

/// The wire shape: what was named, where the ledger left it, and what the note
/// file got.
fn payload(
    params: &WaiveActionItemParams,
    entry: &LedgerEntry,
    note_annotated: bool,
    note_outcome: &str,
) -> Value {
    json!({
        "source": { "id": params.note_id, "item_id": params.item_id },
        "entry": {
            "entry_id": entry.entry_id,
            "state": entry.state,
            "closed_via": entry.closed_via.map(|via| via.as_str().to_string()),
            "updated_at": entry.updated_at,
        },
        "note_annotated": note_annotated,
        "note_outcome": note_outcome,
    })
}
