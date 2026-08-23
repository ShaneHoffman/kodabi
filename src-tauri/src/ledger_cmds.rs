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
    digest, AgingConfig, ClosedVia, Digest, Direction, EntryDetail, EntryFilter, EntryState,
    Evidence, LedgerEntry, NoteContext,
};
use kodabi_core::ledger::{EnrollmentMode, RetroSource};
use kodabi_core::meeting;
use kodabi_core::note::{self, NoteId, INBOX};
use kodabi_core::vault::{self, AnnotateOutcome, ListedNote, SetDoneOutcome};
use tauri::{AppHandle, Emitter, Manager};

use crate::events::{LEDGER_CHANGED_EVENT, VAULT_CHANGED_EVENT};
use crate::index_state::{IndexReadHandle, IndexState};
use crate::ledger_state::{
    LedgerCallError, LedgerClient, LedgerOp, LedgerState, MutateReply, TrackItemRequest,
};
use crate::settings_cmds::SettingsState;
use crate::transcribe::knowledge_base_dir;
use crate::user_errors::{note_error, reported, user_sentence};

/// Serializes the daily digest's compute-if-due window.
///
/// The digest decides whether the day is due, then writes a note and advances
/// the marker; two callers interleaving inside that window would both see the
/// day as due and both write. Refetches make concurrent callers ordinary
/// rather than exotic: the card's hook re-runs on every vault-bus event, and
/// more than one window may be open.
///
/// A `Mutex` rather than an atomic flag because the section is a whole
/// read-write cycle across two stores, and it must be held for all of it.
/// Managed by the app so the single instance outlives any one call.
#[derive(Clone, Default)]
pub struct DigestGate(std::sync::Arc<std::sync::Mutex<()>>);

impl std::ops::Deref for DigestGate {
    type Target = std::sync::Mutex<()>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// The states a live commitments list shows.
const LIVE_STATES: [EntryState; 3] = [
    EntryState::Open,
    EntryState::NeedsReview,
    EntryState::Snoozed,
];

/// The states the settled shelf shows. Waived and untracked sit here with
/// closed: all three are judgements a person may want to take back, and
/// `reopen` takes any of them back.
///
/// Untracked belongs on the shelf for the same reason a closure does. An entry
/// that vanished with no trace would leave the person who untracked it by
/// mistake nothing to undo in the view where they did it.
const SETTLED_STATES: [EntryState; 3] = [
    EntryState::Closed,
    EntryState::Waived,
    EntryState::Untracked,
];

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
    /// The source meeting's genre in its kebab-case wire spelling, or `null`
    /// where the note carries none.
    category: Option<String>,
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

/// What the settled shelf adds up to over its whole window.
/// Mirrors [`kodabi_core::ledger::view::SettledSummary`].
#[derive(serde::Serialize)]
pub struct SettledSummaryDto {
    cleared: u32,
    closed_from_conversation: u32,
    closed_from_github: u32,
}

/// The Commitments view's whole payload.
#[derive(serde::Serialize)]
pub struct CommitmentsDto {
    /// Open, needs-review and snoozed entries, newest mention first.
    entries: Vec<CommitmentDto>,
    /// Recently closed or waived entries, newest first, for the undo shelf.
    settled: Vec<CommitmentDto>,
    /// What the window cleared, counted before `settled` was capped.
    settled_summary: SettledSummaryDto,
    /// When the user last reviewed newly enrolled commitments, RFC 3339 UTC.
    /// The triage strip lists entries whose `created_at` sorts above this.
    ///
    /// `None` only on a ledger that never opened far enough to be seeded, in
    /// which case the strip stays hidden rather than declaring everything new.
    last_seen: Option<String>,
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
    /// `manual | override | category`, present only on an untracked entry: what
    /// the shelf row says about how it left the working set.
    untracked_via: Option<String>,
}

/// One of a note's extracted lines, with whether the ledger is tracking it.
/// Mirrors [`kodabi_core::ledger::view::NoteItemEnrollment`].
#[derive(serde::Serialize)]
pub struct NoteCommitmentItemDto {
    item_id: String,
    description: String,
    owner: String,
    due_date: Option<String>,
    done: bool,
    /// `mine | theirs | unassigned`, from the line's owner.
    direction: String,
    /// `tracked | untracked | not_enrolled`.
    tracking: String,
    untracked_via: Option<String>,
    entry_id: Option<String>,
    entry_state: Option<String>,
}

/// The note view's enrollment panel payload.
#[derive(serde::Serialize)]
pub struct NoteCommitmentsDto {
    /// Whether this meeting carries the context-only override.
    context_only: bool,
    /// The note's extracted lines, in body order.
    items: Vec<NoteCommitmentItemDto>,
}

/// What flipping a meeting's tracking did.
#[derive(serde::Serialize)]
pub struct MeetingTrackingDto {
    context_only: bool,
    /// How many still-open entries the flip removed from the working set.
    untracked: usize,
    /// How many it put back.
    retracked: usize,
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
        untracked_via: entry.untracked_via.map(|via| via.as_str().to_string()),
    }
}

fn note_item_dto(enrollment: view::NoteItemEnrollment) -> NoteCommitmentItemDto {
    NoteCommitmentItemDto {
        item_id: enrollment.item.id,
        description: enrollment.item.description,
        owner: enrollment.item.owner,
        due_date: enrollment.item.due_date,
        done: enrollment.item.done,
        direction: enrollment.direction.as_str().to_string(),
        tracking: enrollment.tracking.as_str().to_string(),
        untracked_via: enrollment.untracked_via.map(|via| via.as_str().to_string()),
        entry_id: enrollment.entry_id,
        entry_state: enrollment.entry_state.map(|s| s.as_str().to_string()),
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
            category: source
                .category
                .map(|category| category.as_str().to_string()),
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
pub(crate) fn ledger_error(cmd: &str, err: LedgerCallError, refused: &str) -> String {
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

/// The same, for a read: nothing was at stake, so the sentence promises less.
const LEDGER_READ_REFUSED: &str =
    "The commitment ledger isn't available this session, so tracking \
                                   can't be shown. Restart Kodabi; your notes are untouched.";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The user's aging thresholds, as the read model wants them.
///
/// Read per call rather than cached: the Settings view can change these while
/// the Commitments view is mounted, and the ledger event that follows sends it
/// straight back here.
pub(crate) fn aging_config(app: &AppHandle) -> AgingConfig {
    aging_config_from(&app.state::<SettingsState>())
}

/// The same, from a `SettingsState` already in hand — for the one caller that
/// reaches it through `try_state` and so cannot take it twice.
fn aging_config_from(settings: &SettingsState) -> AgingConfig {
    let ledger = settings.snapshot().ledger;
    AgingConfig {
        aging_after_days: ledger.aging_after_days.get(),
        stale_after_days: ledger.stale_after_days.get(),
    }
}

/// The device's local calendar day.
///
/// Local rather than UTC on purpose, and one of the sanctioned local reads
/// (`.claude/rules/utc-timestamps.md`): a due date is a local calendar day, so
/// "overdue" has to be judged against the day the person is living in.
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

/// [`handles`], but `None` rather than a panic when the setup hook has not
/// installed them yet.
///
/// The webview begins loading alongside the setup closure, so a command the
/// shell fires on mount can genuinely arrive before `app.manage` has run, and
/// `Manager::state` panics rather than waiting. Every command that a *person*
/// triggers is past that window by definition, which is why the plain
/// [`handles`] is right for them; a read that fires on mount is not, and the
/// digest's whole failure posture is to show nothing rather than to take the
/// view down with it.
fn try_handles(app: &AppHandle) -> Option<(LedgerClient, Option<IndexReadHandle>)> {
    let client = app.try_state::<LedgerState>()?.client();
    let index = app
        .try_state::<IndexState>()
        .and_then(|state| state.read_handle());
    Some((client, index))
}

/// Re-indexes a note the app just rewrote, then tells every window.
///
/// The same eager pair `note_cmds` does after a write: the index upsert keeps
/// search current without waiting on the watcher, and the broadcast makes every
/// open window refetch. The watcher's own later reconcile is then a no-op.
pub(crate) fn reindex_and_broadcast(app: &AppHandle, listed: &ListedNote, kb: &std::path::Path) {
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

/// The daily digest: what changed in the ledger since it last ran.
///
/// **Compute-if-due on read, not a scheduler.** The trigger is simply that a
/// surface asked, and the marker in `ledger_meta` is what makes the answer
/// happen once a day rather than once a mount. That covers the two cases a
/// startup hook would not: the app hides to the tray rather than exiting, so a
/// process can cross midnight and still owe a digest, and a refetch on the
/// vault bus re-asks after any activity. Nothing is scheduled, nothing polls.
///
/// **Writes a note as a side effect, exactly once per digest.** The card and
/// the note are two renderings of one computation, so the write belongs to the
/// same call that computes it, guarded by `DigestGate` so two concurrent
/// refetches cannot both decide the day is due.
///
/// Every failure below the marker degrades to an empty digest rather than an
/// error: the card is a convenience mounted on the landing view, and a ledger
/// that cannot answer must not stop the Inbox from rendering.
#[tauri::command]
pub async fn daily_digest(app: AppHandle) -> Result<Digest, String> {
    // This one fires on the shell's mount, which can beat the setup hook that
    // installs the state below, so every read here is the fallible kind. A
    // digest that is not ready yet is simply no digest: the next refetch on
    // the vault bus asks again, and the day is still due because nothing
    // advanced the marker.
    let (Some((client, index)), Some(settings), Some(gate)) = (
        try_handles(&app),
        app.try_state::<SettingsState>(),
        app.try_state::<DigestGate>(),
    ) else {
        return Ok(Digest::empty(local_today(), local_today()));
    };
    let aging = aging_config_from(&settings);
    let quiet_after_days = settings.snapshot().ledger.quiet_after_days.get();
    let gate = gate.inner().clone();
    let kb = knowledge_base_dir(&app).ok();

    let run = tauri::async_runtime::spawn_blocking(move || {
        // Held across check, compute, write and store, so exactly one caller
        // can find the day due. A poisoned lock is not a reason to skip a
        // digest, so the guard is taken through the poison either way.
        let _guard = gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        compute_if_due(&client, index, kb.as_deref(), aging, quiet_after_days)
    })
    .await;

    // Every degraded path lands on the same answer: no digest today. The card
    // is a convenience on the landing view, and it must not be able to stop
    // the Inbox rendering.
    let run = match run {
        Ok(run) => run,
        Err(err) => {
            eprintln!("daily_digest failed: {err:?}");
            DigestRun::default()
        }
    };

    // Indexing happens after the gate is released and back where the
    // `AppHandle` lives, matching every other write-then-reindex command.
    if let (Some(written), Ok(kb)) = (run.written, knowledge_base_dir(&app)) {
        index_digest_note(&app, &written, &kb);
    }
    Ok(run.digest)
}

/// A digest run's two outputs: what to show, and the note it wrote (if any).
struct DigestRun {
    digest: Digest,
    written: Option<WrittenDigest>,
}

impl Default for DigestRun {
    fn default() -> Self {
        let today = local_today();
        DigestRun {
            digest: Digest::empty(today, today),
            written: None,
        }
    }
}

/// A digest note that reached the disk, waiting to be indexed.
struct WrittenDigest {
    note: note::Note,
    path: std::path::PathBuf,
}

/// The digest for today, computing and writing one if the day is due.
///
/// Runs entirely on the blocking thread, under the gate.
fn compute_if_due(
    client: &LedgerClient,
    index: Option<IndexReadHandle>,
    kb: Option<&std::path::Path>,
    aging: AgingConfig,
    quiet_after_days: u32,
) -> DigestRun {
    let today = local_today();
    let state = match client.digest_state() {
        Ok(state) => state,
        Err(err) => {
            eprintln!("daily_digest could not read the digest marker: {err:?}");
            return DigestRun::default();
        }
    };

    let Some(last_run) = state.last_run else {
        // First launch on this device. Seed the marker and show nothing: every
        // transition in the ledger's history crossed before today, and greeting
        // someone with all of them is the opposite of a digest. The same
        // restraint `SeedTriageMarker` shows.
        let digest = Digest::empty(today, today);
        record(client, &digest);
        return DigestRun {
            digest,
            written: None,
        };
    };

    let baseline = local_day_of(&last_run).unwrap_or(today);
    if baseline >= today {
        // Already run today. Serve what it produced, so the card holds the same
        // list all day instead of emptying the moment the marker is past.
        return DigestRun {
            digest: stored_digest(state.payload.as_deref(), today),
            written: None,
        };
    }

    // Ledger first, index second, never interleaved (see the module doc).
    let live = match client.list_details(EntryFilter {
        states: Some(LIVE_STATES.to_vec()),
        ..EntryFilter::default()
    }) {
        Ok(live) => live,
        Err(err) => {
            eprintln!("daily_digest could not read the ledger: {err:?}");
            return DigestRun::default();
        }
    };
    let notes: HashMap<String, NoteContext> = index
        .map(|index| index.note_contexts(&referenced_notes(&live)))
        .unwrap_or_default();

    let digest = digest::compute(
        &live,
        &notes,
        baseline,
        &last_run,
        today,
        aging,
        quiet_after_days,
    );
    // Recorded *before* the note is written, and the note only if that
    // succeeded. The marker is the sole thing that stops a second run today,
    // and writing the note is what makes this call broadcast `vault:changed`
    // — which is the same bus the card refetches on. So a run whose marker
    // never landed and whose note did would re-enter here on its own refetch,
    // find the day due again, and write another note, for as long as the
    // ledger keeps refusing the write; `write_note` disambiguates a colliding
    // filename rather than failing, so nothing downstream stops it. Recording
    // first costs at most a lost note if the process dies in between, and the
    // digest is still on the card either way.
    //
    // A quiet day still counts as run: the marker advances and no note is
    // written, so tomorrow measures from today rather than re-reporting a
    // week of transitions at once.
    let recorded = record(client, &digest);
    let written = (recorded && !digest.is_empty())
        .then(|| kb.and_then(|kb| write_digest_note(kb, &digest)))
        .flatten();
    DigestRun { digest, written }
}

/// Records the run: the payload first, then the marker (see
/// [`crate::ledger_state::LedgerJob::StoreDigest`]).
///
/// **The marker is the instant the digest ran, not the local day's midnight.**
/// It has to be, in both directions. Stamping the local date's midnight *as
/// UTC* would read back through [`local_day_of`] as the previous day for any
/// negative offset, so "has it run today" would never be true west of UTC and
/// every refetch would recompute and write another note. And the instant is
/// what the needs-review rule wants anyway: "parked since the last digest"
/// means since the moment it ran, not since that morning.
///
/// Returns whether the run was recorded. A failure is logged and swallowed —
/// the digest on screen is still true — but the caller must not write the note
/// without it, or the day stays due and the write repeats on every refetch.
fn record(client: &LedgerClient, digest: &Digest) -> bool {
    let payload = match serde_json::to_string(digest) {
        Ok(payload) => payload,
        Err(err) => {
            eprintln!("daily_digest could not serialize the digest: {err}");
            return false;
        }
    };
    let run_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    match client.store_digest(run_at, payload) {
        Ok(()) => true,
        Err(err) => {
            eprintln!("daily_digest could not record the run: {err:?}");
            false
        }
    }
}

/// Today's stored digest, or an empty one when it cannot be read.
///
/// The date check is what stops a payload from an earlier day being re-shown
/// after the marker advanced past it, which is the one thing a digest of
/// transitions must never do.
fn stored_digest(payload: Option<&str>, today: chrono::NaiveDate) -> Digest {
    payload
        .and_then(|payload| match serde_json::from_str::<Digest>(payload) {
            Ok(digest) => Some(digest),
            Err(err) => {
                eprintln!("daily_digest could not read the stored digest: {err}");
                None
            }
        })
        .filter(|digest| digest.date == today.to_string())
        .unwrap_or_else(|| Digest::empty(today, today))
}

/// Writes the digest into the vault.
///
/// **Deliberately no retention hook.** The sweep reads only `<kb>/sessions/`,
/// non-recursively, so a note under `Digests/` is outside it by construction
/// rather than by exemption. Digests are derived and regenerable, but they are
/// also the only record of what the ledger said on a given day, and being
/// regenerable is not the same as being reproducible: yesterday's transitions
/// cannot be recomputed once the state has moved on.
///
/// Best-effort: a failed write is logged, and the card still shows the digest.
fn write_digest_note(kb: &std::path::Path, digest: &Digest) -> Option<WrittenDigest> {
    let (note, title) = match digest::build_note(digest) {
        Ok(built) => built,
        Err(err) => {
            eprintln!("daily_digest could not build the note: {err}");
            return None;
        }
    };
    match note::write_note(kb, &note, Some(title.as_str())) {
        Ok(path) => Some(WrittenDigest { note, path }),
        Err(err) => {
            eprintln!("daily_digest could not write the note: {err}");
            None
        }
    }
}

/// Indexes the digest note and tells every window, the same eager pair every
/// in-app write makes.
///
/// The action-item guard is the belt to `ledger::digest`'s braces. That module
/// renders plain bullets precisely so nothing here can be extracted, and its
/// tests pin it; this is the boundary where a regression would turn into
/// enrolled commitments, so it refuses to carry any across rather than
/// trusting the renderer twice.
fn index_digest_note(app: &AppHandle, written: &WrittenDigest, kb: &std::path::Path) {
    let rel = written.path.strip_prefix(kb).unwrap_or(&written.path);
    let mut indexed = kodabi_core::index::IndexedNote::from_note(
        &written.note,
        &vault::effective_title(&written.note, &written.path),
        &rel.to_string_lossy().replace('\\', "/"),
    );
    indexed.meeting = meeting::meeting_facts_for(&written.note, kb);
    if let Some(facts) = indexed.meeting.as_mut() {
        if !facts.action_items.is_empty() {
            eprintln!(
                "daily_digest: the digest note derived {} action item(s); dropping them rather \
                 than enrolling the digest's own contents",
                facts.action_items.len()
            );
            facts.action_items.clear();
        }
    }
    // `try_state` for the same reason the rest of this command uses it: the
    // shell can fire `daily_digest` before the setup hook has managed the
    // index, and `Manager::state` panics rather than waiting. The note is
    // already on disk, so the watcher's reconcile picks it up either way; the
    // broadcast still runs, because the views should refresh regardless.
    if let Some(index) = app.try_state::<IndexState>() {
        index.index_note_best_effort(indexed);
    }
    broadcast_vault_changed(app);
}

/// The local calendar day an RFC 3339 instant falls on for this device.
///
/// Local rather than UTC for the same reason [`local_today`] is: the question
/// is whether the digest has run on the day the person is living in.
fn local_day_of(instant: &str) -> Option<chrono::NaiveDate> {
    chrono::DateTime::parse_from_rfc3339(instant)
        .ok()
        .map(|parsed| parsed.with_timezone(&Local).date_naive())
}

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
        // Read while the ledger answer is still in hand, before the index lock:
        // same store, same worker, so it costs one more queue round-trip and
        // keeps the two-store ordering rule intact.
        //
        // A marker the ledger cannot supply degrades to `None` rather than
        // failing the whole read. The strip is a convenience; the commitments
        // are the point, and a list that refuses to render because a viewing
        // timestamp is unreadable would be the wrong trade.
        let last_seen = client.triage_last_seen().unwrap_or_else(|err| {
            eprintln!("list_commitments could not read the triage marker: {err:?}");
            None
        });
        let cutoff = (Utc::now() - Duration::days(view::SETTLED_WINDOW_DAYS))
            .to_rfc3339_opts(SecondsFormat::Secs, true);
        let shelf = view::settled_shelf(settled, &cutoff, view::SETTLED_CAP);

        let mut wanted = referenced_notes(&live);
        wanted.extend(referenced_notes(&shelf.entries));
        let notes: HashMap<String, NoteContext> = index
            .map(|index| index.note_contexts(&wanted))
            .unwrap_or_default();

        let today = local_today();
        Ok(CommitmentsDto {
            entries: view::assemble(live, &notes, today, aging)
                .into_iter()
                .map(commitment_dto)
                .collect(),
            settled: view::assemble(shelf.entries, &notes, today, aging)
                .into_iter()
                .map(commitment_dto)
                .collect(),
            settled_summary: SettledSummaryDto {
                cleared: shelf.summary.cleared,
                closed_from_conversation: shelf.summary.closed_from_conversation,
                closed_from_github: shelf.summary.closed_from_github,
            },
            last_seen,
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

/// The dock's count: how many commitments are on the user, outstanding.
///
/// Mine only, because the number a person watches go down is their own; what
/// they are waiting on from someone else is a register, not a queue. The cut
/// matches the Mine group the Commitments view draws (open, needs review, and
/// snoozes whose day has arrived), so a snooze that is off the screen is also
/// off the number.
///
/// Whole-vault on purpose: the dock row navigates to the whole-vault view.
#[tauri::command]
pub async fn count_my_commitments(app: AppHandle) -> Result<u32, String> {
    // Like the digest, this fires on the dock's mount rather than on a click,
    // so it can beat the setup hook that manages the ledger. Zero is the
    // honest answer for "not ready": the row already renders nothing at zero,
    // and the next refetch on the vault bus asks again.
    let Some((client, _)) = try_handles(&app) else {
        return Ok(0);
    };
    tauri::async_runtime::spawn_blocking(move || {
        let details = client
            .list_details(EntryFilter {
                states: Some(LIVE_STATES.to_vec()),
                direction: Some(Direction::Mine),
                ..EntryFilter::default()
            })
            .map_err(|err| {
                ledger_error(
                    "count_my_commitments",
                    err,
                    "The commitment ledger isn't available this session, so commitments can't \
                     be counted. Restart Kodabi; your notes are untouched.",
                )
            })?;
        Ok(view::outstanding_count(&details, local_today()) as u32)
    })
    .await
    .map_err(|err| {
        reported(
            "count_my_commitments",
            err,
            "Couldn't count your commitments. The sidebar's number may be out of date; \
             opening Commitments shows the current list.",
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

/// What became of the name a claimed row was filed under.
///
/// Three outcomes rather than a bool, because two of them look identical from
/// the outside and mean opposite things. Refusing to learn `"Them"` is the
/// design working (`ledger::learnable_alias`); failing to write a real name is
/// the one case worth telling the user about, since the same misfiling will
/// happen again. A bool would have the view apologising for the former.
#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AliasOutcome {
    /// The spelling joined the user's names.
    Saved,
    /// Nothing to learn: a reserved token, or a spelling already known.
    NotNeeded,
    /// There was a name to learn and it could not be saved.
    Failed,
}

/// What a claim settled on: the re-filed entry, and what became of the name it
/// was filed under.
#[derive(serde::Serialize)]
pub struct ClaimMineDto {
    pub entry: CommitmentEntryDto,
    pub alias: AliasOutcome,
}

/// Re-files a commitment as the user's own, and learns the name it was filed
/// under so the next meeting gets it right unprompted.
///
/// The correction loop the routing examples established: every correction is
/// training data. Three writes, in a deliberate order.
///
/// 1. **The claim itself**, through the ordinary mutation path - so it marks
///    the entry `touched`, which is exactly right: a person has now judged this
///    row, and no later sweep may overrule them.
/// 2. **The alias**, unless the owner string is one the app already owns
///    (`ledger::learnable_alias`). A failure here does not fail the command:
///    the user's actual request - move this to Mine - has already landed, and
///    reporting it as an error would be a lie about what the ledger holds. The
///    view is told through [`AliasOutcome`] instead, which separates a refusal
///    from a failure so it only speaks up for the second.
/// 3. **The sweep**, best-effort in the background, so the sibling entries that
///    same name is sitting on move with it rather than needing a click each.
#[tauri::command]
pub async fn claim_commitment_mine(
    app: AppHandle,
    input: CommitmentEntryInput,
) -> Result<ClaimMineDto, String> {
    let (client, _) = handles(&app);
    let reply = mutate(
        &app,
        "claim_commitment_mine",
        client,
        LedgerOp::ClaimMine {
            entry_id: input.entry_id,
        },
    )
    .await?;

    let alias = learn_owner_alias(&app, &reply.entry.owner);
    Ok(ClaimMineDto {
        entry: entry_dto(&reply.entry),
        alias,
    })
}

/// Adds `owner` to the user's aliases and re-checks the rest of the ledger
/// against the new set.
///
/// Never fails the command - see [`claim_commitment_mine`]'s step 2. The return
/// value distinguishes the two ways nothing was learned, because only one of
/// them is the user's problem.
fn learn_owner_alias(app: &AppHandle, owner: &str) -> AliasOutcome {
    // A reserved token: "You" and "Unassigned" are resolved before any alias is
    // consulted, and "Them" is what the distill guidance writes for an unnamed
    // other, so adopting it would claim every future them-side line.
    let Some(alias) = kodabi_core::ledger::learnable_alias(owner) else {
        return AliasOutcome::NotNeeded;
    };
    let state = app.state::<SettingsState>();
    let mut learned = false;
    let updated = state.update(
        "Couldn't save that name, so future mentions of it may still file under \
Waiting on them.",
        |s| learned = s.identity.learn_alias(alias),
    );

    match updated {
        Ok(settings) if learned => {
            let _ = app.emit(
                crate::settings_cmds::SETTINGS_CHANGED_EVENT,
                settings.clone(),
            );
            crate::settings_cmds::resolve_owners_in_background(
                app,
                settings.identity.owner_identity(),
            );
            AliasOutcome::Saved
        }
        // The write landed but changed nothing: some spelling of this name was
        // already known.
        Ok(_) => AliasOutcome::NotNeeded,
        Err(err) => {
            eprintln!("claim_commitment_mine: couldn't learn {owner:?}: {err}");
            AliasOutcome::Failed
        }
    }
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

/// Removes a commitment from the working set: it never should have been in it.
///
/// The sibling of `waive_commitment`, and the distinction is worth keeping in
/// the two sentences the UI shows. Waiving is about the commitment (it was
/// mine, it stopped mattering); untracking is about the ledger (this was never
/// my business). Ledger-only, and reversible through the same Reopen.
#[tauri::command]
pub async fn untrack_commitment(
    app: AppHandle,
    input: CommitmentEntryInput,
) -> Result<CommitmentEntryDto, String> {
    let (client, _) = handles(&app);
    let reply = mutate(
        &app,
        "untrack_commitment",
        client,
        LedgerOp::Untrack {
            entry_id: input.entry_id,
        },
    )
    .await?;
    Ok(entry_dto(&reply.entry))
}

/// A request naming several entries.
#[derive(serde::Deserialize)]
pub struct CommitmentEntriesInput {
    entry_ids: Vec<String>,
}

/// The same, plus the day to sleep until.
#[derive(serde::Deserialize)]
pub struct SnoozeCommitmentsInput {
    entry_ids: Vec<String>,
    until: String,
}

/// What a batched gesture settled on: how many rows moved, and how many the
/// ledger declined.
///
/// Counts rather than entries because the caller refetches anyway — the
/// `ledger:changed` this broadcasts is the full truth, and the strip needs only
/// enough to say "3 couldn't be snoozed" beside the group.
#[derive(serde::Serialize)]
pub struct BulkMutateDto {
    updated: usize,
    skipped: usize,
}

/// Untracks several commitments as one gesture, for the triage strip's
/// group and selection verbs.
///
/// Semantics are the single verb's, repeated: refs stay active, the rows land
/// on the Settled shelf, Reopen is the undo, and each is stamped
/// `untracked_via = manual` so a later meeting re-track leaves them alone.
#[tauri::command]
pub async fn untrack_commitments(
    app: AppHandle,
    input: CommitmentEntriesInput,
) -> Result<BulkMutateDto, String> {
    let (client, _) = handles(&app);
    let ops = input
        .entry_ids
        .into_iter()
        .map(|entry_id| LedgerOp::Untrack { entry_id })
        .collect();
    mutate_many(&app, "untrack_commitments", client, ops).await
}

/// Snoozes several commitments as one gesture.
///
/// An entry the transition table refuses (a needs-review row cannot be snoozed
/// while it is asking a question) is reported in `skipped`, not fatal: the rest
/// of the group still sleeps.
#[tauri::command]
pub async fn snooze_commitments(
    app: AppHandle,
    input: SnoozeCommitmentsInput,
) -> Result<BulkMutateDto, String> {
    let (client, _) = handles(&app);
    let until = input.until;
    let ops = input
        .entry_ids
        .into_iter()
        .map(|entry_id| LedgerOp::Snooze {
            entry_id,
            until: until.clone(),
        })
        .collect();
    mutate_many(&app, "snooze_commitments", client, ops).await
}

/// A request advancing the triage marker.
#[derive(serde::Deserialize)]
pub struct MarkSeenInput {
    seen_through: String,
}

/// Records that the user has reviewed newly enrolled commitments up to
/// `seen_through` (RFC 3339 UTC).
///
/// **Deliberately silent.** No `ledger:changed`: no commitment changed, and
/// announcing would make every Keep refetch the list and recompute the strip
/// out from under the hand that is clearing it. The marker is read once per
/// view mount, which is exactly when it matters.
#[tauri::command]
pub async fn mark_commitments_seen(app: AppHandle, input: MarkSeenInput) -> Result<(), String> {
    let (client, _) = handles(&app);
    tauri::async_runtime::spawn_blocking(move || client.mark_triage_seen(input.seen_through))
        .await
        .map_err(|err| {
            reported(
                "mark_commitments_seen",
                err,
                "Couldn't save your place in the review list. It will show these again next time.",
            )
        })?
        .map_err(|err| {
            ledger_error(
                "mark_commitments_seen",
                err,
                "Couldn't save your place in the review list. It will show these again next time.",
            )
        })
}

/// A request naming one note.
#[derive(serde::Deserialize)]
pub struct NoteCommitmentsInput {
    note_id: String,
}

/// The note view's enrollment panel: this meeting's tracking mode and every
/// extracted line with whether the ledger is tracking it.
///
/// Ledger first, then the index, per the module doc. An index that cannot
/// supply the note's facts yields an empty list rather than an error: the note
/// may be a type that carries no commitments, which is an ordinary answer.
///
/// The *mode* comes from those same index facts rather than from the ledger,
/// because it is a frontmatter key now; the index row mirrors the note file,
/// and reading it here keeps the panel showing what the note actually says.
#[tauri::command]
pub async fn list_note_commitments(
    app: AppHandle,
    input: NoteCommitmentsInput,
) -> Result<NoteCommitmentsDto, String> {
    let (client, index) = handles(&app);
    let enrolment = crate::index_state::enrolment_settings(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let note_id = input.note_id;
        let details = client
            .note_entries(note_id.clone())
            .map_err(|err| ledger_error("list_note_commitments", err, LEDGER_READ_REFUSED))?;
        let facts = index.and_then(|index| index.note_facts(&note_id, &enrolment));
        // The *effective* mode, so the panel reports what actually gates this
        // meeting: a note with no override of its own still reads as context
        // only when its category says so.
        let mode = facts.as_ref().map(|facts| {
            kodabi_core::ledger::effective_mode(facts.note_override, facts.category_default)
        });
        let items = facts
            .map(|facts| {
                facts
                    .items
                    .into_iter()
                    .map(|item| kodabi_core::index::ActionItemRow {
                        id: item.id,
                        description: item.description,
                        owner: item.owner,
                        due_date: item.due_date,
                        done: item.done,
                        firm: item.firm,
                        extracted_date: item.extracted_date,
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(NoteCommitmentsDto {
            context_only: mode == Some(EnrollmentMode::ContextOnly),
            items: view::assemble_note_items(&note_id, items, &details, enrolment.identity())
                .into_iter()
                .map(note_item_dto)
                .collect(),
        })
    })
    .await
    .map_err(|err| {
        reported(
            "list_note_commitments",
            err,
            "Couldn't read this meeting's tracking. Reopen the note to try again.",
        )
    })?
}

/// A request setting one meeting's tracking mode.
#[derive(serde::Deserialize)]
pub struct MeetingTrackingInput {
    note_id: String,
    context_only: bool,
}

/// Sets whether a meeting is tracked in full or for direct asks only, and
/// re-evaluates the entries it already produced.
///
/// **The judgement is written to the note's frontmatter**, not to the ledger
/// database: it is a fact about the meeting, so it belongs with the meeting,
/// where it survives a re-route, a vault rebuild, and a sync to another
/// machine. The ledger keeps only the consequence.
///
/// **Both directions write a value; neither clears the key.** Since a meeting's
/// category now carries a default of its own, an absent `tracking:` is not
/// "tracked" but "whatever my kind says" — so switching context-only *off* by
/// clearing it would leave an all-hands exactly as gated as before. Flipping the
/// switch is a decision about this meeting, and it outranks the genre from then
/// on. There is deliberately no affordance here for returning a meeting to
/// inheriting its kind.
///
/// Order, and why: the ledger's availability is checked first, so a refusal
/// costs nothing; the vault write comes next, and a failure there leaves
/// everything unchanged; the retro-application follows and is *awaited*, so the
/// eager re-index that follows it cannot overtake it on the worker's queue.
/// That re-index is what makes re-tracking whole — it forwards the note's fresh
/// facts, whose `note_override` now reads the new value, and the ordinary
/// idempotent create leg enrols the items the old mode gated out. The two
/// acquisitions (index, then ledger) stay strictly sequential and never nested,
/// which is the property the module doc's ordering rule exists to protect.
///
/// If the retro-application fails after the note was written, the note's
/// frontmatter is still the truth and gates every future sync; only the
/// re-evaluation of entries that already exist is missing, and flipping again
/// re-runs it.
#[tauri::command]
pub async fn set_meeting_tracking(
    app: AppHandle,
    input: MeetingTrackingInput,
) -> Result<MeetingTrackingDto, String> {
    let kb = knowledge_base_dir(&app)?;
    let (client, _) = handles(&app);
    // Refused before the vault is touched: a switch that flips with nothing
    // behind it is worse than one that refuses to flip.
    if !client.is_available() {
        return Err(reported(
            "set_meeting_tracking",
            "the ledger worker is unavailable",
            LEDGER_REFUSED,
        ));
    }

    let note_id = NoteId::parse(&input.note_id).map_err(|err| {
        reported(
            "set_meeting_tracking",
            err,
            "That note id isn't valid, so its tracking can't be changed.",
        )
    })?;
    let context_only = input.context_only;

    let listed = tauri::async_runtime::spawn_blocking({
        let kb = kb.clone();
        move || {
            // Pinned in both directions, never cleared. Clearing would hand the
            // meeting back to its category's default, and on an all-hands or an
            // observer meeting that default *is* context-only — so switching
            // "context only" off would move the switch and change nothing. A
            // flip is a judgement about this meeting, so it outranks the genre
            // from here on.
            vault::set_note_tracking(
                &kb,
                &note_id,
                Some(if context_only {
                    EnrollmentMode::ContextOnly
                } else {
                    EnrollmentMode::Tracked
                }),
            )
        }
    })
    .await
    .map_err(|err| {
        reported(
            "set_meeting_tracking",
            err,
            "Couldn't change this meeting's tracking. Reopen the note and try again.",
        )
    })?
    .map_err(|err| {
        note_error(
            "set_meeting_tracking",
            err,
            "Couldn't change this meeting's tracking. The note is unchanged; try again.",
        )
    })?
    .ok_or_else(|| {
        "This meeting is no longer in the vault, so its tracking can't be changed.".to_string()
    })?;

    let project = listed.note.routing.project().to_string();
    let outcome = tauri::async_runtime::spawn_blocking({
        let note_id = input.note_id.clone();
        let project = project.clone();
        move || {
            client.retro_apply_note_tracking(
                note_id,
                project,
                context_only,
                // The meeting's own switch decided, whatever its genre says.
                RetroSource::Override,
            )
        }
    })
    .await
    .map_err(|err| {
        reported(
            "set_meeting_tracking",
            err,
            "This meeting's tracking was saved, but its existing commitments weren't \
             re-checked. Flip it again to re-check them.",
        )
    })?
    .map_err(|err| {
        ledger_error(
            "set_meeting_tracking",
            err,
            "This meeting's tracking was saved, but its existing commitments weren't \
             re-checked. Flip it again to re-check them.",
        )
    })?;

    // Re-index the rewritten note. The index worker forwards its fresh facts to
    // the ledger, and *that* is the follow-up sync: the facts now carry the new
    // `note_override`, so the create leg enrols what the old mode gated out.
    // Best-effort, like every other write path here: the watcher converges it.
    reindex_and_broadcast(&app, &listed, &kb);
    broadcast_ledger_changed(&app);
    Ok(MeetingTrackingDto {
        context_only: outcome.context_only,
        untracked: outcome.untracked.len(),
        retracked: outcome.retracked.len(),
    })
}

/// A request naming one extracted line.
#[derive(serde::Deserialize)]
pub struct TrackItemInput {
    note_id: String,
    item_id: String,
}

/// Tracks one extracted line by hand, whatever the meeting's mode says.
///
/// Idempotent, and deliberately incapable of resurrecting a settled
/// commitment: a note view left open while the entry was closed elsewhere can
/// only re-affirm what is already true.
#[tauri::command]
pub async fn track_commitment_item(
    app: AppHandle,
    input: TrackItemInput,
) -> Result<CommitmentEntryDto, String> {
    let (client, index) = handles(&app);
    let note_id = input.note_id.clone();
    let item_id = input.item_id.clone();
    let enrolment = crate::index_state::enrolment_settings(&app);
    let entry = tauri::async_runtime::spawn_blocking(move || {
        let Some(facts) = index.and_then(|index| index.note_facts(&note_id, &enrolment)) else {
            return Err(
                "Kodabi doesn't have this meeting indexed yet, so this line can't be \
                        tracked. Try again in a moment."
                    .to_string(),
            );
        };
        let Some(item) = facts.items.iter().find(|item| item.id == item_id).cloned() else {
            return Err(
                "That line changed since this note was loaded. Reopen the note and try \
                        again."
                    .to_string(),
            );
        };
        client
            .track_item(TrackItemRequest {
                note_id: facts.note_id,
                project: facts.project,
                note_date_utc: facts.date_utc,
                item,
                identity: facts.identity,
            })
            .map_err(|err| ledger_error("track_commitment_item", err, LEDGER_REFUSED))
    })
    .await
    .map_err(|err| {
        reported(
            "track_commitment_item",
            err,
            "Couldn't track this line. Reopen the note and try again.",
        )
    })??;

    broadcast_ledger_changed(&app);
    Ok(entry_dto(&entry))
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

/// Runs a batch of mutations as one gesture: one worker job, one reply, **one**
/// `ledger:changed`.
///
/// That last part is the reason this exists rather than the caller looping
/// [`mutate`]. N single mutations would emit N events, each triggering a full
/// refetch of the Commitments view, so clearing a standup's worth of
/// commitments would re-render the list a dozen times and rearm the snapshot
/// debounce on each pass. One job also means the ops cannot interleave with
/// another window's write half way through the sweep.
///
/// An empty batch still reports success and announces nothing — there is
/// nothing to hear about, and refusing it would make the caller special-case a
/// selection the user already emptied.
async fn mutate_many(
    app: &AppHandle,
    cmd: &'static str,
    client: LedgerClient,
    ops: Vec<LedgerOp>,
) -> Result<BulkMutateDto, String> {
    if ops.is_empty() {
        return Ok(BulkMutateDto {
            updated: 0,
            skipped: 0,
        });
    }
    let reply = tauri::async_runtime::spawn_blocking(move || client.mutate_many(ops))
        .await
        .map_err(|err| {
            reported(
                cmd,
                err,
                "Couldn't update these commitments. Reopen this view to see the current list.",
            )
        })?
        .map_err(|err| ledger_error(cmd, err, LEDGER_REFUSED))?;
    // Anything at all landed is worth announcing; a wholly declined batch
    // changed nothing, and the copy the caller renders says so.
    if !reply.applied.is_empty() {
        broadcast_ledger_changed(app);
    }
    for (entry_id, err) in &reply.skipped {
        eprintln!("{cmd} skipped {entry_id}: {err}");
    }
    Ok(BulkMutateDto {
        updated: reply.applied.len(),
        skipped: reply.skipped.len(),
    })
}
