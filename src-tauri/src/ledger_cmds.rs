//! Thin Tauri command wrappers over `kodabi_core::ledger` — the Commitments
//! view's whole backend.
//!
//! The join, the snooze-expiry rule and the settled-shelf window all live in
//! `kodabi_core::ledger::view`; these commands own only the serde IPC DTOs,
//! resolve managed state, supply the clock and the device's local `today`, and
//! translate failures into copy (see `user_errors`).
//!
//! **Two stores, read in a fixed order.** A commitment is a ledger entry joined
//! to the index's row for its source line, and the two are reached through
//! different handles: the ledger through a worker channel that can block, the
//! index through a shared lock the worker takes for a whole-vault scan. Every
//! command here takes the ledger's answer *first*, then the index lock, never
//! interleaved and never one while holding the other. Neither side can wait on
//! the other by construction, and keeping it that way is the point of saying so
//! here.
//!
//! **A mutation checks availability before it touches the vault.** Ticking a
//! checkbox writes Markdown and records a judgement; if the ledger cannot take
//! the judgement, the write is refused whole rather than leaving a ticked box
//! with nothing behind it (`ledger_state`'s failure posture).

use std::collections::{BTreeSet, HashMap};

use chrono::{Duration, Local, SecondsFormat, Utc};
use kodabi_core::index::ActionItemStatus;
use kodabi_core::ledger::view::{self, Commitment};
use kodabi_core::ledger::{
    AgingConfig, ClosedVia, EntryDetail, EntryFilter, EntryState, Evidence, LedgerEntry,
    NoteContext,
};
use kodabi_core::meeting;
use kodabi_core::note::{self, NoteId, INBOX};
use kodabi_core::vault::{self, AnnotateOutcome, ListedNote, SetDoneOutcome};
use tauri::{AppHandle, Emitter, Manager};

use crate::events::{LEDGER_CHANGED_EVENT, VAULT_CHANGED_EVENT};
use crate::index_state::{IndexReadHandle, IndexState};
use crate::ledger_state::{LedgerCallError, LedgerClient, LedgerOp, LedgerState, MutateReply};
use crate::settings_cmds::SettingsState;
use crate::transcribe::knowledge_base_dir;
use crate::user_errors::{reported, user_sentence};

/// How far back the settled shelf reaches.
///
/// A closure reached by evidence has to be visible to be undoable, and a week
/// is the span over which "wait, that is not done" is still a live thought.
/// Older closures are history, and history is the note's job.
const SETTLED_WINDOW_DAYS: i64 = 7;

/// How many settled entries the shelf carries. A bound, not a page: a person
/// undoing a mistake is looking for something they just saw.
const SETTLED_CAP: usize = 20;

/// The states a live commitments list shows.
const LIVE_STATES: [EntryState; 3] = [
    EntryState::Open,
    EntryState::NeedsReview,
    EntryState::Snoozed,
];

/// The states the settled shelf shows. Waived sits here with closed: both are
/// judgements a person may want to take back, and `reopen` takes either back.
const SETTLED_STATES: [EntryState; 2] = [EntryState::Closed, EntryState::Waived];

// ---------------------------------------------------------------------------
// Wire DTOs
// ---------------------------------------------------------------------------

/// One commitment as the view renders it. Mirrors
/// [`kodabi_core::ledger::view::Commitment`], flattened for the wire.
#[derive(serde::Serialize)]
pub struct CommitmentDto {
    entry_id: String,
    state: String,
    direction: String,
    /// The ledger's cached owner. A reader prefers `item.owner` when a live
    /// line exists; this is what survives the line being edited away.
    owner: String,
    /// The ledger's cached description, same rule as `owner`.
    description: String,
    /// Project slug, or `null` for the Inbox sentinel.
    project: Option<String>,
    created_at: String,
    updated_at: String,
    last_mention: String,
    /// When an evidence provider last checked this commitment, if one ever
    /// has. Pairs with `last_mention` as the other half of the aging anchor.
    last_evidence_check: Option<String>,
    /// `fresh | aging | stale`: how long the entry has gone untouched,
    /// derived against the device's local today and the user's thresholds.
    tier: String,
    snoozed_until: Option<String>,
    /// Whether a snooze's day has arrived. Evaluated at read time; nothing
    /// writes when a snooze lapses.
    snooze_lapsed: bool,
    closed_via: Option<String>,
    review_reason: Option<String>,
    item: Option<CommitmentItemDto>,
    source: Option<CommitmentSourceDto>,
    evidence: Vec<CommitmentEvidenceDto>,
}

/// The live source line: the note's current text, and the checkbox that owns
/// done/not-done. Mirrors [`kodabi_core::ledger::view::CommitmentItem`].
#[derive(serde::Serialize)]
pub struct CommitmentItemDto {
    note_id: String,
    item_id: String,
    description: String,
    owner: String,
    due_date: Option<String>,
    done: bool,
    /// `open | overdue | done`, derived against the device's local today.
    status: String,
}

/// Where a commitment's source line lives, for a click-through. Mirrors
/// [`kodabi_core::ledger::view::CommitmentSource`].
#[derive(serde::Serialize)]
pub struct CommitmentSourceDto {
    note_id: String,
    title: String,
    project: Option<String>,
    path: String,
}

/// One evidence claim. Mirrors [`kodabi_core::ledger::Evidence`].
#[derive(serde::Serialize)]
pub struct CommitmentEvidenceDto {
    evidence_id: String,
    source: String,
    reference: Option<String>,
    confidence: f64,
    observed_at: String,
}

/// The Commitments view's whole payload.
#[derive(serde::Serialize)]
pub struct CommitmentsDto {
    /// Open, needs-review and snoozed entries, newest mention first.
    entries: Vec<CommitmentDto>,
    /// Recently closed or waived entries, newest first, for the undo shelf.
    settled: Vec<CommitmentDto>,
}

/// A mutation's echo: enough to recognise the row that changed. The full truth
/// arrives on the refetch the emitted event triggers.
#[derive(serde::Serialize)]
pub struct CommitmentEntryDto {
    entry_id: String,
    state: String,
    snoozed_until: Option<String>,
    closed_via: Option<String>,
    review_reason: Option<String>,
    updated_at: String,
}

/// The checkbox write's outcome.
#[derive(serde::Serialize)]
pub struct SetCommitmentDoneDto {
    entry: CommitmentEntryDto,
    /// Whether the note's checkbox actually moved. `false` when the source line
    /// was edited away since the ledger linked it, which is an ordinary answer,
    /// not a failure.
    note_updated: bool,
}

/// A confirmed claim's outcome: what the ledger settled, and what landed in the
/// note.
#[derive(serde::Serialize)]
pub struct ConfirmEvidenceDto {
    entry: CommitmentEntryDto,
    note_updated: bool,
    note_annotated: bool,
}

fn entry_dto(entry: &LedgerEntry) -> CommitmentEntryDto {
    CommitmentEntryDto {
        entry_id: entry.entry_id.clone(),
        state: entry.state.as_str().to_string(),
        snoozed_until: entry.snoozed_until.clone(),
        closed_via: entry.closed_via.map(|via| via.as_str().to_string()),
        review_reason: entry.review_reason.clone(),
        updated_at: entry.updated_at.clone(),
    }
}

fn commitment_dto(commitment: Commitment) -> CommitmentDto {
    let Commitment {
        detail,
        item,
        source,
        snooze_lapsed,
        tier,
    } = commitment;
    let entry = detail.entry;
    CommitmentDto {
        entry_id: entry.entry_id,
        state: entry.state.as_str().to_string(),
        direction: entry.direction.as_str().to_string(),
        owner: entry.owner,
        description: entry.description,
        // The Inbox sentinel is a real folder to the ledger and a null project
        // to the frontend, matching `note_cmds`' projection.
        project: (entry.project != INBOX).then_some(entry.project),
        created_at: entry.created_at,
        updated_at: entry.updated_at,
        last_mention: entry.last_mention,
        last_evidence_check: entry.last_evidence_check,
        tier: tier.as_str().to_string(),
        snoozed_until: entry.snoozed_until,
        snooze_lapsed,
        closed_via: entry.closed_via.map(|via| via.as_str().to_string()),
        review_reason: entry.review_reason,
        item: item.map(|item| CommitmentItemDto {
            note_id: item.note_id,
            item_id: item.item_id,
            description: item.description,
            owner: item.owner,
            due_date: item.due_date,
            done: item.done,
            status: match item.status {
                ActionItemStatus::Open => "open",
                ActionItemStatus::Overdue => "overdue",
                ActionItemStatus::Done => "done",
            }
            .to_string(),
        }),
        source: source.map(|source| CommitmentSourceDto {
            note_id: source.note_id,
            title: source.title,
            project: source.project,
            path: source.path,
        }),
        evidence: detail
            .evidence
            .into_iter()
            .map(|claim| CommitmentEvidenceDto {
                evidence_id: claim.evidence_id,
                source: claim.source.as_str().to_string(),
                reference: claim.reference,
                confidence: claim.confidence,
                observed_at: claim.observed_at,
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Error copy
// ---------------------------------------------------------------------------

/// Translates a [`LedgerCallError`] into the sentence the user reads.
///
/// `refused` is what to say when the ledger never opened, and is per-command
/// because the honest half of that sentence ("your notes are untouched", "this
/// change wasn't saved") depends on what the command would have done.
fn ledger_error(cmd: &str, err: LedgerCallError, refused: &str) -> String {
    match err {
        LedgerCallError::Unavailable => reported(cmd, "commitment ledger unavailable", refused),
        // Deliberately not "nothing was saved": a queued job is not cancelled
        // when the caller stops waiting, so it may still apply. Saying so is
        // the difference between copy that is cautious and copy that is wrong.
        LedgerCallError::NoReply => reported(
            cmd,
            "commitment ledger did not answer in time",
            "The commitment ledger didn't answer in time. Your change may still apply; this list \
             refreshes itself.",
        ),
        LedgerCallError::Ledger(err) => ledger_failure(cmd, err),
    }
}

/// Translates the ledger's own failures.
fn ledger_failure(cmd: &str, err: kodabi_core::ledger::LedgerError) -> String {
    use kodabi_core::ledger::LedgerError;
    match &err {
        // The view is stale: the row was resolved in another window, or the
        // entry is simply gone. Both are answered by looking again.
        LedgerError::EntryNotFound { .. } | LedgerError::EvidenceNotFound { .. } => reported(
            cmd,
            err,
            "This commitment is no longer in the ledger. Reopen this view to see the current list.",
        ),
        LedgerError::IllegalTransition { .. } => reported(
            cmd,
            err,
            "This commitment changed since the list was loaded. Reopen this view and try again.",
        ),
        // Already the user's words (a snooze date they picked).
        LedgerError::InvalidField { detail, .. } => {
            let sentence = user_sentence(detail);
            reported(cmd, &err, &sentence)
        }
        _ => reported(
            cmd,
            err,
            "Couldn't update this commitment. The ledger is unchanged; try again.",
        ),
    }
}

/// The sentence a mutation wears when the ledger never opened.
const LEDGER_REFUSED: &str = "The commitment ledger isn't available this session, so this change \
                              wasn't saved. Restart Kodabi and try again; your notes are \
                              untouched.";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The device's local calendar day.
///
/// Local rather than UTC on purpose, and one of the sanctioned local reads
/// (`.claude/rules/utc-timestamps.md`): a due date is a local calendar day, so
/// "overdue" has to be judged against the day the person is living in.
/// The user's aging thresholds, as the read model wants them.
///
/// Read per call rather than cached: the Settings view can change these while
/// the Commitments view is mounted, and the ledger event that follows sends it
/// straight back here.
fn aging_config(app: &AppHandle) -> AgingConfig {
    let ledger = app.state::<SettingsState>().snapshot().ledger;
    AgingConfig {
        aging_after_days: ledger.aging_after_days.get(),
        stale_after_days: ledger.stale_after_days.get(),
    }
}

fn local_today() -> chrono::NaiveDate {
    Local::now().date_naive()
}

fn broadcast_ledger_changed(app: &AppHandle) {
    let _ = app.emit(LEDGER_CHANGED_EVENT, ());
}

fn broadcast_vault_changed(app: &AppHandle) {
    let _ = app.emit(VAULT_CHANGED_EVENT, ());
}

/// Clones the two handles a read or a write needs out of managed state, before
/// anything crosses onto a blocking thread.
fn handles(app: &AppHandle) -> (LedgerClient, Option<IndexReadHandle>) {
    let client = app.state::<LedgerState>().client();
    let index = app.state::<IndexState>().read_handle();
    (client, index)
}

/// Re-indexes a note the app just rewrote, then tells every window.
///
/// The same eager pair `note_cmds` does after a write: the index upsert keeps
/// search current without waiting on the watcher, and the broadcast makes every
/// open window refetch. The watcher's own later reconcile is then a no-op.
fn reindex_and_broadcast(app: &AppHandle, listed: &ListedNote, kb: &std::path::Path) {
    let rel = listed.path.strip_prefix(kb).unwrap_or(&listed.path);
    let mut indexed = kodabi_core::index::IndexedNote::from_note(
        &listed.note,
        &listed.title,
        &rel.to_string_lossy().replace('\\', "/"),
    );
    indexed.meeting = meeting::meeting_facts_for(&listed.note, kb);
    app.state::<IndexState>().index_note_best_effort(indexed);
    broadcast_vault_changed(app);
}

/// Every note id the entries' live refs point at.
fn referenced_notes(details: &[EntryDetail]) -> BTreeSet<String> {
    details
        .iter()
        .filter_map(|detail| {
            detail
                .item_refs
                .iter()
                .find(|item_ref| item_ref.active)
                .map(|item_ref| item_ref.note_id.clone())
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// The Commitments view's read: live entries plus the recently settled shelf,
/// each joined to its source line.
///
/// `project` scopes to one project and its descendants; `None` is the whole
/// vault.
#[tauri::command]
pub async fn list_commitments(
    app: AppHandle,
    project: Option<String>,
) -> Result<CommitmentsDto, String> {
    let (client, index) = handles(&app);
    let aging = aging_config(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let filter = |states: &[EntryState]| EntryFilter {
            states: Some(states.to_vec()),
            project: project.clone(),
            include_descendants: project.is_some(),
            ..EntryFilter::default()
        };

        // Ledger first, index second, never interleaved (see the module doc).
        let live = client.list_details(filter(&LIVE_STATES)).map_err(|err| {
            ledger_error(
                "list_commitments",
                err,
                "The commitment ledger isn't available this session, so commitments can't be \
                 shown. Restart Kodabi; your notes are untouched.",
            )
        })?;
        let settled = client
            .list_details(filter(&SETTLED_STATES))
            .map_err(|err| {
                ledger_error(
                    "list_commitments",
                    err,
                    "The commitment ledger isn't available this session, so commitments can't be \
                 shown. Restart Kodabi; your notes are untouched.",
                )
            })?;
        let cutoff = (Utc::now() - Duration::days(SETTLED_WINDOW_DAYS))
            .to_rfc3339_opts(SecondsFormat::Secs, true);
        let settled = view::recently_settled(settled, &cutoff, SETTLED_CAP);

        let mut wanted = referenced_notes(&live);
        wanted.extend(referenced_notes(&settled));
        let notes: HashMap<String, NoteContext> = index
            .map(|index| index.note_contexts(&wanted))
            .unwrap_or_default();

        let today = local_today();
        Ok(CommitmentsDto {
            entries: view::assemble(live, &notes, today, aging)
                .into_iter()
                .map(commitment_dto)
                .collect(),
            settled: view::assemble(settled, &notes, today, aging)
                .into_iter()
                .map(commitment_dto)
                .collect(),
        })
    })
    .await
    .map_err(|err| {
        reported(
            "list_commitments",
            err,
            "Couldn't read your commitments. Reopen this view to try again.",
        )
    })?
}

/// A checkbox click.
#[derive(serde::Deserialize)]
pub struct SetCommitmentDoneInput {
    entry_id: String,
    note_id: String,
    item_id: String,
    done: bool,
}

/// Ticks or unticks a commitment's checkbox in its source note, and records the
/// matching judgement in the ledger.
///
/// **Two writes, note first.** The note's checkbox is the source of truth for
/// done/not-done, and the ledger records the *judgement* (`closed_via: manual`,
/// or back to open). Note-first is what makes a partial failure legible: a
/// ticked box whose ledger write failed shows up as a ticked, still-open row
/// that clicking again retries, whereas the reverse would close a commitment
/// whose note never moved.
#[tauri::command]
pub async fn set_commitment_done(
    app: AppHandle,
    input: SetCommitmentDoneInput,
) -> Result<SetCommitmentDoneDto, String> {
    let kb = knowledge_base_dir(&app)?;
    let (client, _) = handles(&app);
    // Refused before the vault is touched: a box that ticks with nothing behind
    // it is worse than a box that refuses to tick.
    if !client.is_available() {
        return Err(reported(
            "set_commitment_done",
            "commitment ledger unavailable",
            LEDGER_REFUSED,
        ));
    }
    let id = NoteId::parse(&input.note_id).map_err(|err| {
        reported(
            "set_commitment_done",
            err,
            "This commitment points at a note Kodabi can't read. Reopen this view to try again.",
        )
    })?;

    let item_id = input.item_id.clone();
    let done = input.done;
    let write_kb = kb.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        vault::set_action_item_done(&write_kb, &id, &item_id, done)
    })
    .await
    .map_err(|err| reported("set_commitment_done", err, SET_DONE_FAILED))?
    .map_err(|err| reported("set_commitment_done", err, SET_DONE_FAILED))?;

    let note_updated = match outcome {
        SetDoneOutcome::Updated(listed) => {
            reindex_and_broadcast(&app, &listed, &kb);
            true
        }
        // Ordinary answers: the line was edited away, or already read this way.
        // The ledger judgement below still stands, because a commitment whose
        // source line is gone is exactly what the ledger exists to keep.
        SetDoneOutcome::AlreadySet | SetDoneOutcome::NoteMissing | SetDoneOutcome::ItemMissing => {
            false
        }
    };

    let op = if input.done {
        LedgerOp::Close {
            entry_id: input.entry_id,
            via: ClosedVia::Manual,
        }
    } else {
        LedgerOp::Reopen {
            entry_id: input.entry_id,
        }
    };
    let reply = mutate(&app, "set_commitment_done", client, op).await?;

    Ok(SetCommitmentDoneDto {
        entry: entry_dto(&reply.entry),
        note_updated,
    })
}

/// The sentence a failed checkbox write wears.
const SET_DONE_FAILED: &str = "Couldn't update this commitment. Its note is unchanged; try again.";

/// A snooze request. `until` is a local `YYYY-MM-DD` day; the ledger validates
/// it and its complaint is already the user's words.
#[derive(serde::Deserialize)]
pub struct SnoozeCommitmentInput {
    entry_id: String,
    until: String,
}

/// Hides a commitment until a day of the user's choosing. Ledger-only: the note
/// is untouched.
#[tauri::command]
pub async fn snooze_commitment(
    app: AppHandle,
    input: SnoozeCommitmentInput,
) -> Result<CommitmentEntryDto, String> {
    let (client, _) = handles(&app);
    let reply = mutate(
        &app,
        "snooze_commitment",
        client,
        LedgerOp::Snooze {
            entry_id: input.entry_id,
            until: input.until,
        },
    )
    .await?;
    Ok(entry_dto(&reply.entry))
}

/// A request naming only an entry.
#[derive(serde::Deserialize)]
pub struct CommitmentEntryInput {
    entry_id: String,
}

/// Marks a commitment as deliberately not happening.
///
/// Ledger-only, and that is the whole point of the verb: waiving exists so a
/// person never has to edit a meeting note to pretend something was not said.
#[tauri::command]
pub async fn waive_commitment(
    app: AppHandle,
    input: CommitmentEntryInput,
) -> Result<CommitmentEntryDto, String> {
    let (client, _) = handles(&app);
    let reply = mutate(
        &app,
        "waive_commitment",
        client,
        LedgerOp::Waive {
            entry_id: input.entry_id,
        },
    )
    .await?;
    Ok(entry_dto(&reply.entry))
}

/// Returns a commitment to open, whatever it was.
///
/// The one undo behind every affordance: waking a snooze, taking back a waiver,
/// and reversing a closure an evidence pass made on its own. The note is left
/// exactly as it stands, including any closure annotation: annotate, never
/// destroy. Unticking a box that a closure ticked is `set_commitment_done`'s
/// job, which the caller runs instead of this one when a live line exists.
#[tauri::command]
pub async fn reopen_commitment(
    app: AppHandle,
    input: CommitmentEntryInput,
) -> Result<CommitmentEntryDto, String> {
    let (client, _) = handles(&app);
    let reply = mutate(
        &app,
        "reopen_commitment",
        client,
        LedgerOp::Reopen {
            entry_id: input.entry_id,
        },
    )
    .await?;
    Ok(entry_dto(&reply.entry))
}

/// A request naming an entry and one of its evidence claims.
#[derive(serde::Deserialize)]
pub struct CommitmentEvidenceInput {
    entry_id: String,
    evidence_id: String,
}

/// Accepts a parked evidence claim: closes the entry with that claim's
/// provenance, ticks the box, and writes the story into the note.
///
/// The human half of the confidence split. A provider confident enough to close
/// on its own does all three of these; a provider that was not parks the entry
/// in `needs_review` and this is where the person agrees. The closure is
/// recorded as `github` or `conversation`, never `manual`: the evidence
/// resolved it, the person only agreed.
///
/// Both note writes are best-effort and reported rather than fatal, because the
/// ledger already holds the judgement and the note is the narrative.
#[tauri::command]
pub async fn confirm_commitment_evidence(
    app: AppHandle,
    input: CommitmentEvidenceInput,
) -> Result<ConfirmEvidenceDto, String> {
    let kb = knowledge_base_dir(&app)?;
    let (client, _) = handles(&app);
    if !client.is_available() {
        return Err(reported(
            "confirm_commitment_evidence",
            "commitment ledger unavailable",
            LEDGER_REFUSED,
        ));
    }

    let reply = mutate(
        &app,
        "confirm_commitment_evidence",
        client,
        LedgerOp::ConfirmEvidence {
            entry_id: input.entry_id,
            evidence_id: input.evidence_id,
        },
    )
    .await?;

    let mut note_updated = false;
    let mut note_annotated = false;
    if let Some((note_id, item_id)) = reply.active_ref.clone() {
        let annotation = annotation_for(reply.evidence.as_ref());
        let closed_on = Local::now().format("%Y-%m-%d").to_string();
        let write_kb = kb.clone();
        let written = tauri::async_runtime::spawn_blocking(move || {
            // An unparseable id here is a corrupt ref rather than user input,
            // and the ledger already holds the judgement, so it degrades to a
            // note that simply says nothing.
            let Ok(id) = NoteId::parse(&note_id) else {
                return Ok((SetDoneOutcome::NoteMissing, AnnotateOutcome::NoteMissing));
            };
            let ticked = vault::set_action_item_done(&write_kb, &id, &item_id, true)?;
            let annotated =
                vault::annotate_action_item(&write_kb, &id, &item_id, &closed_on, &annotation)?;
            Ok::<_, note::NoteError>((ticked, annotated))
        })
        .await;

        match written {
            Ok(Ok((ticked, annotated))) => {
                note_updated = matches!(ticked, SetDoneOutcome::Updated(_));
                note_annotated = matches!(annotated, AnnotateOutcome::Annotated(_));
                // Either write may have rewritten the file; re-index from
                // whichever ran last so the row matches what is on disk.
                let listed = match annotated {
                    AnnotateOutcome::Annotated(listed) => Some(listed),
                    _ => match ticked {
                        SetDoneOutcome::Updated(listed) => Some(listed),
                        _ => None,
                    },
                };
                if let Some(listed) = listed {
                    reindex_and_broadcast(&app, &listed, &kb);
                }
            }
            Ok(Err(err)) => eprintln!("confirm_commitment_evidence: note write failed: {err}"),
            Err(err) => eprintln!("confirm_commitment_evidence: note write panicked: {err}"),
        }
    }

    Ok(ConfirmEvidenceDto {
        entry: entry_dto(&reply.entry),
        note_updated,
        note_annotated,
    })
}

/// The sentence written under the item when a claim is confirmed.
fn annotation_for(evidence: Option<&Evidence>) -> String {
    use kodabi_core::ledger::EvidenceSource;
    let Some(claim) = evidence else {
        return "confirmed.".to_string();
    };
    let source = match claim.source {
        EvidenceSource::Github => "confirmed from GitHub",
        EvidenceSource::Conversation => "confirmed from a conversation",
        EvidenceSource::Manual => "confirmed",
    };
    match &claim.reference {
        Some(reference) => format!("{source} ({reference})."),
        None => format!("{source}."),
    }
}

/// Rejects a parked evidence claim: removes it, and reopens the entry when that
/// claim is what closed it. The note is untouched.
#[tauri::command]
pub async fn dismiss_commitment_evidence(
    app: AppHandle,
    input: CommitmentEvidenceInput,
) -> Result<CommitmentEntryDto, String> {
    let (client, _) = handles(&app);
    let reply = mutate(
        &app,
        "dismiss_commitment_evidence",
        client,
        LedgerOp::DismissEvidence {
            entry_id: input.entry_id,
            evidence_id: input.evidence_id,
        },
    )
    .await?;
    Ok(entry_dto(&reply.entry))
}

/// Runs one mutation off the async runtime and announces it.
///
/// `spawn_blocking`, and that is load-bearing rather than tidy:
/// [`LedgerClient::mutate`] waits on the worker's reply for as long as
/// `REPLY_TIMEOUT`, and the worker may be part-way through a whole-vault
/// reconcile. Waiting for that on an async worker thread parks it for every
/// other command in the app, so the wait happens on the blocking pool, exactly
/// as `index_cmds` does with its search handle.
///
/// The announcement happens on success only, but note that a failure the *view*
/// caused (a stale row) is answered by copy telling the person to look again,
/// which is the same refetch by a slower route.
async fn mutate(
    app: &AppHandle,
    cmd: &'static str,
    client: LedgerClient,
    op: LedgerOp,
) -> Result<MutateReply, String> {
    let reply = tauri::async_runtime::spawn_blocking(move || client.mutate(op))
        .await
        // A panicked blocking task says nothing about what the worker did with
        // the job, so the copy claims nothing either.
        .map_err(|err| {
            reported(
                cmd,
                err,
                "Couldn't update this commitment. Reopen this view to see the current list.",
            )
        })?
        .map_err(|err| ledger_error(cmd, err, LEDGER_REFUSED))?;
    broadcast_ledger_changed(app);
    Ok(reply)
}
