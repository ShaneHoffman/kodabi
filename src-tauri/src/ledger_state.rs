//! Managed state for the durable commitment ledger.
//!
//! Deliberately a sibling of [`crate::index_state`] rather than a tenant of it,
//! because the two have opposite contracts. The index is a rebuildable cache
//! whose module doc opens by saying nothing in it is load-bearing; the ledger
//! holds judgements that exist nowhere else. Concretely the ledger needs three
//! things the index worker cannot give it: a worker that runs even when the
//! index failed to open, a `recv_timeout` loop of its own for the debounced
//! snapshot writer, and a shutdown flush it can acknowledge.
//!
//! The ledger worker owns its `Ledger` outright and single-threaded, so there is
//! no lock to contend and no poisoning story. The index worker only ever does a
//! non-blocking channel send into it, handing over facts by value.
//!
//! **Failure posture, in two halves.** An unopenable `ledger.db` logs and
//! degrades to a session that records nothing, rather than blocking launch.
//! For *automatic* ingestion that is the right trade: [`LedgerHandle`] is
//! fire-and-forget, a degraded session only misses derivations, and those
//! re-converge from the notes at the next healthy startup reconcile. Nothing
//! recorded is lost, because nothing is written.
//!
//! **A person's own judgement is different, and gets [`LedgerClient`].** Waiving,
//! snoozing, or closing an entry is a decision that exists nowhere else, so
//! dropping it silently would be a lie. Every command-facing call therefore goes
//! through the client, which answers [`LedgerCallError::Unavailable`] when the
//! ledger never opened and [`LedgerCallError::NoReply`] when the worker does not
//! answer in time, and the wrapper turns each into copy that says what did and
//! did not happen. Note the asymmetry a timeout forces: a queued job may still
//! run after the caller has given up, so `NoReply` copy says the change *may*
//! have applied rather than claiming it did not.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{SecondsFormat, Utc};
use kodabi_core::distill::LedgerUpdateDraft;
use kodabi_core::ledger::{
    self, AppliedUpdates, ClosedVia, DistillFollowUp, EnrollmentMode, EntryDetail, EntryFilter,
    Evidence, Ledger, LedgerEntry, NoteSync, NoteTrackingOutcome, UntrackedVia, LEDGER_DB_FILE,
};
use kodabi_core::meeting::ActionItemFact;
use kodabi_core::note::INBOX;
use tauri::{AppHandle, Manager};

use crate::transcribe::knowledge_base_dir;

/// Quiet period after the last change before snapshots are written.
///
/// Deliberately laxer than the vault watcher's 500ms: that window races a user's
/// perception of their own edit, while a snapshot is a backup whose staleness
/// costs nothing at all as long as the database is alive.
const SNAPSHOT_QUIET: Duration = Duration::from_secs(2);

/// Longest a change may wait for a lull before snapshots are written anyway.
/// Bounds the crash-loss window during a sustained reconcile burst, which would
/// otherwise keep resetting the quiet timer.
const SNAPSHOT_MAX_DELAY: Duration = Duration::from_secs(15);

/// How long a shutdown flush may block the app's exit.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a command waits for the worker to answer a read or a mutation.
///
/// Far laxer than [`FLUSH_TIMEOUT`], which races the user's patience at Quit.
/// This one races nothing: the request sits behind whatever the worker is
/// already doing, and a whole-vault reconcile's `SyncBatch` is one job. Timing
/// out early would report failure for work that then succeeds.
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// One note's derived action items, as handed to the ledger worker.
pub struct NoteFacts {
    pub note_id: String,
    /// Project slug; the Inbox sentinel for an unfiled note.
    pub project: String,
    /// The note's `date_utc`, which becomes the entry's `last_mention`.
    pub date_utc: String,
    pub items: Vec<ActionItemFact>,
}

/// A mutation a person asked for, addressed to one entry.
///
/// Named as a request rather than called directly on the [`Ledger`] because the
/// worker owns it: this is the wire between a command thread and that owner.
pub enum LedgerOp {
    /// Resolve, with the provenance of whoever established it.
    Close { entry_id: String, via: ClosedVia },
    /// Deliberately not going to happen.
    Waive { entry_id: String },
    /// Out of sight until a local `YYYY-MM-DD` day.
    Snooze { entry_id: String, until: String },
    /// Back to open, whatever it was: the undo behind every affordance.
    Reopen { entry_id: String },
    /// Accept a parked claim, closing with that claim's provenance.
    ConfirmEvidence {
        entry_id: String,
        evidence_id: String,
    },
    /// Reject a parked claim, reopening the entry if that claim closed it.
    DismissEvidence {
        entry_id: String,
        evidence_id: String,
    },
    /// Remove from the working set: it never should have been in it. Distinct
    /// from [`LedgerOp::Waive`], which says it was mine and stopped mattering.
    Untrack { entry_id: String },
}

impl LedgerOp {
    /// The entry every variant addresses.
    fn entry_id(&self) -> &str {
        match self {
            LedgerOp::Close { entry_id, .. }
            | LedgerOp::Waive { entry_id }
            | LedgerOp::Snooze { entry_id, .. }
            | LedgerOp::Reopen { entry_id }
            | LedgerOp::ConfirmEvidence { entry_id, .. }
            | LedgerOp::DismissEvidence { entry_id, .. }
            | LedgerOp::Untrack { entry_id } => entry_id,
        }
    }
}

/// What a mutation settled on, plus what the caller needs to finish the job in
/// the vault.
#[derive(Debug)]
pub struct MutateReply {
    pub entry: LedgerEntry,
    /// The claim that closed the entry ([`LedgerOp::ConfirmEvidence`] only), so
    /// the caller can name the source in the note's annotation.
    pub evidence: Option<Evidence>,
    /// The entry's live `(note_id, item_id)`, when it still has one. The
    /// caller needs it to tick the box or write the annotation, and neither is
    /// possible for an entry whose source line is gone.
    pub active_ref: Option<(String, String)>,
}

/// A unit of work for the ledger worker.
enum LedgerJob {
    /// Reconcile one note's items after an in-app write.
    Sync(Box<NoteFacts>),
    /// The same for a batch, as a whole-vault reconcile produces.
    SyncBatch(Vec<NoteFacts>),
    /// A note left the vault: retire its refs, flag what is left.
    NoteGone(String),
    /// Rebuild from the vault's snapshots, but only into an empty database.
    RestoreIfEmpty,
    /// Write every pending snapshot now and acknowledge.
    Flush(Sender<()>),
    /// Read entries matching a filter, hydrated, and reply.
    ListDetails {
        filter: EntryFilter,
        reply: Sender<ledger::Result<Vec<EntryDetail>>>,
    },
    /// Apply one mutation and reply with what it settled on.
    Mutate {
        op: LedgerOp,
        reply: Sender<ledger::Result<MutateReply>>,
    },
    /// Sync a freshly distilled note and apply what its conversation said
    /// about commitments the ledger already held.
    DistillFollowUp {
        follow_up: Box<OwnedFollowUp>,
        autoclose_threshold: f64,
        reply: Sender<ledger::Result<AppliedUpdates>>,
    },
    /// Read one note's tracking override and the entries it produced, for the
    /// note view's enrollment panel. One round trip rather than two, because
    /// the panel always needs both.
    NoteEnrollment {
        note_id: String,
        #[allow(clippy::type_complexity)]
        reply: Sender<ledger::Result<(Option<EnrollmentMode>, Vec<EntryDetail>)>>,
    },
    /// Set (or clear) a note's tracking override and retro-apply it.
    SetNoteTracking {
        note_id: String,
        project: String,
        context_only: bool,
        reply: Sender<ledger::Result<NoteTrackingOutcome>>,
    },
    /// Promote one extracted line by hand, whatever the mode says.
    TrackItem {
        request: Box<TrackItemRequest>,
        reply: Sender<ledger::Result<LedgerEntry>>,
    },
}

/// One manual promote, owned so it can cross the channel.
pub struct TrackItemRequest {
    pub note_id: String,
    pub project: String,
    pub note_date_utc: String,
    pub item: ActionItemFact,
}

/// A [`DistillFollowUp`] that owns its strings, so it can cross the channel.
///
/// The core type borrows because it is built and used inside one call; a job
/// outlives its sender, so this is the same shape with the lifetimes paid off.
pub struct OwnedFollowUp {
    pub note_id: String,
    pub project: String,
    pub note_date_utc: String,
    pub items: Vec<ActionItemFact>,
    pub updates: Vec<LedgerUpdateDraft>,
}

/// A handle to the background ledger worker, held as Tauri managed state.
pub struct LedgerState {
    /// `None` when the database failed to open (logged at startup).
    sender: Option<Mutex<Sender<LedgerJob>>>,
}

/// A cloneable write handle, for the index worker to forward facts through.
///
/// Every method is fire-and-forget and silently no-ops when the ledger is
/// unavailable, so the index worker never has to branch on it.
#[derive(Clone)]
pub struct LedgerHandle(Option<Sender<LedgerJob>>);

impl LedgerHandle {
    /// Forwards one note's freshly derived items.
    pub fn sync(&self, facts: NoteFacts) {
        self.send(LedgerJob::Sync(Box::new(facts)));
    }

    /// Forwards a whole reconcile pass's worth of notes as one job, so the
    /// worker takes one transaction per note rather than one wake-up per note.
    pub fn sync_batch(&self, facts: Vec<NoteFacts>) {
        if !facts.is_empty() {
            self.send(LedgerJob::SyncBatch(facts));
        }
    }

    /// Tells the ledger a note is gone from the vault.
    pub fn note_gone(&self, note_id: &str) {
        self.send(LedgerJob::NoteGone(note_id.to_string()));
    }

    fn send(&self, job: LedgerJob) {
        if let Some(sender) = &self.0 {
            let _ = sender.send(job);
        }
    }
}

/// Why a [`LedgerClient`] call could not be answered.
///
/// Distinct from [`ledger::LedgerError`] on purpose: the two failures a person
/// must be told about honestly are structural, not about the entry.
#[derive(Debug)]
pub enum LedgerCallError {
    /// The ledger never opened this session, so nothing can be recorded.
    Unavailable,
    /// The worker did not answer within [`REPLY_TIMEOUT`]. **The job may still
    /// run**: nothing cancels a queued job, so copy must not claim the change
    /// was discarded.
    NoReply,
    /// The ledger answered, and the answer was a failure.
    Ledger(ledger::LedgerError),
}

/// A cloneable request handle for the command layer.
///
/// The honest counterpart to [`LedgerHandle`]: every call here is a person's
/// judgement rather than a derivation, so it waits for an answer and reports
/// what happened. Mutations are routed through the worker rather than opening a
/// second connection so that the snapshot debounce sees them exactly as it sees
/// a sync (see [`run_worker`]) and the database keeps its single owner.
#[derive(Clone)]
pub struct LedgerClient(Option<Sender<LedgerJob>>);

impl LedgerClient {
    /// Whether the ledger opened this session.
    ///
    /// A command checks this *before* touching the vault, so a change that
    /// cannot be recorded is refused whole rather than half-applied.
    pub fn is_available(&self) -> bool {
        self.0.is_some()
    }

    /// Entries matching `filter`, with their refs, evidence, and links.
    pub fn list_details(
        &self,
        filter: EntryFilter,
    ) -> std::result::Result<Vec<EntryDetail>, LedgerCallError> {
        self.request(|reply| LedgerJob::ListDetails { filter, reply })
    }

    /// Applies one mutation.
    pub fn mutate(&self, op: LedgerOp) -> std::result::Result<MutateReply, LedgerCallError> {
        self.request(|reply| LedgerJob::Mutate { op, reply })
    }

    /// One note's tracking override and the entries it produced.
    #[allow(clippy::type_complexity)]
    pub fn note_enrollment(
        &self,
        note_id: String,
    ) -> std::result::Result<(Option<EnrollmentMode>, Vec<EntryDetail>), LedgerCallError> {
        self.request(|reply| LedgerJob::NoteEnrollment { note_id, reply })
    }

    /// Sets (or clears) a note's tracking override, retro-applying it.
    pub fn set_note_tracking(
        &self,
        note_id: String,
        project: String,
        context_only: bool,
    ) -> std::result::Result<NoteTrackingOutcome, LedgerCallError> {
        self.request(|reply| LedgerJob::SetNoteTracking {
            note_id,
            project,
            context_only,
            reply,
        })
    }

    /// Promotes one extracted line by hand.
    pub fn track_item(
        &self,
        request: TrackItemRequest,
    ) -> std::result::Result<LedgerEntry, LedgerCallError> {
        self.request(|reply| LedgerJob::TrackItem {
            request: Box::new(request),
            reply,
        })
    }

    /// Syncs a distilled note and applies its commitment classifications.
    pub fn distill_follow_up(
        &self,
        follow_up: OwnedFollowUp,
        autoclose_threshold: f64,
    ) -> std::result::Result<AppliedUpdates, LedgerCallError> {
        self.request(|reply| LedgerJob::DistillFollowUp {
            follow_up: Box::new(follow_up),
            autoclose_threshold,
            reply,
        })
    }

    /// Sends a job carrying a reply channel and waits, bounded.
    ///
    /// Blocking by design: callers are commands, which run this inside
    /// `spawn_blocking` rather than on an async worker thread.
    fn request<T>(
        &self,
        build: impl FnOnce(Sender<ledger::Result<T>>) -> LedgerJob,
    ) -> std::result::Result<T, LedgerCallError> {
        let sender = self.0.as_ref().ok_or(LedgerCallError::Unavailable)?;
        let (reply, answer) = mpsc::channel();
        sender
            .send(build(reply))
            .map_err(|_| LedgerCallError::Unavailable)?;
        match answer.recv_timeout(REPLY_TIMEOUT) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(err)) => Err(LedgerCallError::Ledger(err)),
            Err(_) => Err(LedgerCallError::NoReply),
        }
    }
}

impl LedgerState {
    /// Opens the ledger, queues the startup restore, and spawns the worker.
    ///
    /// Never fails: see the module doc's failure posture.
    pub fn initialize(app: &AppHandle) -> Self {
        let Some(ledger) = open_ledger(app) else {
            eprintln!(
                "commitment ledger unavailable — commitments will not be tracked this session"
            );
            return Self { sender: None };
        };

        // Resolved once here rather than per flush: the worker outlives any
        // single job, and a vault path that cannot be resolved means snapshots
        // are skipped, not that the ledger stops recording.
        let vault_root = knowledge_base_dir(app).ok();

        let (sender, jobs) = mpsc::channel::<LedgerJob>();
        std::thread::spawn(move || {
            run_worker(jobs, ledger, vault_root, SNAPSHOT_QUIET, SNAPSHOT_MAX_DELAY)
        });

        // Queued before any handle escapes this function, so it is the worker's
        // first job no matter how quickly the index starts syncing. That
        // ordering is what makes "restore only into an empty database" hold:
        // the first sync would otherwise make the database non-empty and defeat
        // the restore permanently.
        let _ = sender.send(LedgerJob::RestoreIfEmpty);

        Self {
            sender: Some(Mutex::new(sender)),
        }
    }

    /// A write handle for the index worker.
    pub fn handle(&self) -> LedgerHandle {
        LedgerHandle(self.sender())
    }

    /// A request handle for the command layer.
    ///
    /// Cloned out of managed state before a command crosses onto a blocking
    /// thread, which is what lets the blocking wait happen off the async
    /// runtime (the `SearchHandle` shape in [`crate::index_state`]).
    pub fn client(&self) -> LedgerClient {
        LedgerClient(self.sender())
    }

    /// Clones the job sender, if the ledger opened.
    fn sender(&self) -> Option<Sender<LedgerJob>> {
        self.sender.as_ref().map(|sender| {
            sender
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone()
        })
    }
}

/// Writes every pending snapshot before the app exits, waiting briefly for the
/// worker to acknowledge.
///
/// Bounded by [`FLUSH_TIMEOUT`] so a wedged disk cannot hang Quit. Losing the
/// flush costs only snapshot freshness: the database is already durable, and the
/// next mutation touching those projects rewrites them.
pub fn flush(app: &AppHandle) {
    let Some(state) = app.try_state::<LedgerState>() else {
        return;
    };
    let Some(sender) = &state.sender else {
        return;
    };
    let (ack, done) = mpsc::channel();
    {
        let sender = sender.lock().unwrap_or_else(|poison| poison.into_inner());
        if sender.send(LedgerJob::Flush(ack)).is_err() {
            return;
        }
    }
    if done.recv_timeout(FLUSH_TIMEOUT).is_err() {
        eprintln!("ledger snapshot flush did not finish before exit");
    }
}

/// Opens `ledger.db` in the app config dir, beside `settings.toml`.
///
/// Resolved through [`crate::sandbox::config_dir`] rather than Tauri's
/// `app_config_dir` directly, which is the whole reason `KODABI_SANDBOX`
/// relocates it for free: an agent-driven launch must never write commitments
/// into the user's real data.
fn open_ledger(app: &AppHandle) -> Option<Ledger> {
    let dir = match crate::sandbox::config_dir(app) {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("failed to resolve the config directory for the ledger: {err}");
            return None;
        }
    };
    if let Err(err) = std::fs::create_dir_all(&dir) {
        eprintln!("failed to create the config directory for the ledger: {err}");
        return None;
    }
    match Ledger::open(&dir.join(LEDGER_DB_FILE)) {
        Ok(ledger) => Some(ledger),
        Err(err) => {
            eprintln!("failed to open the commitment ledger: {err}");
            None
        }
    }
}

/// The current instant as the ledger stores them: RFC 3339 UTC, seconds.
fn now_utc() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// The worker loop: serve ledger jobs in arrival order, and write the snapshots
/// for whatever they dirtied once the writes settle.
///
/// The debounce lives here rather than in kodabi-core because core owns no
/// timers and reads no clock. Parameterized on both intervals so a test can
/// drive it with short ones over an injected channel, the shape
/// `watch::run_debounce` established.
fn run_worker(
    jobs: Receiver<LedgerJob>,
    mut ledger: Ledger,
    vault_root: Option<PathBuf>,
    quiet: Duration,
    max_delay: Duration,
) {
    // When the oldest un-flushed change arrived, and the most recent one.
    let mut oldest: Option<Instant> = None;
    let mut newest: Option<Instant> = None;

    loop {
        let deadline = oldest.zip(newest).map(|(oldest, newest)| {
            // Whichever comes first: the lull, or the cap on waiting for one.
            (newest + quiet).min(oldest + max_delay)
        });
        let received = match deadline {
            Some(deadline) => jobs.recv_timeout(deadline.saturating_duration_since(Instant::now())),
            None => jobs.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };

        match received {
            Ok(job) => {
                let pending_before = ledger.dirty_projects().len();
                let stop = handle_job(&mut ledger, vault_root.as_deref(), job);
                let pending_after = ledger.dirty_projects().len();

                // Only a *newly* dirtied project extends the quiet window. A
                // further change to a project already queued needs no extension:
                // the pending flush writes the project's current contents
                // whenever it runs. Without this, a steady stream of edits to
                // one project would starve the writer until `max_delay`.
                //
                // The `oldest.is_none()` leg re-arms after a flush whose write
                // failed: those projects are still dirty but carry no deadline,
                // so without it the retry would wait for some *other* project to
                // be dirtied. Arming on the next job rather than immediately is
                // what keeps a wedged disk from spinning while the app is idle.
                if pending_after > 0 && (pending_after > pending_before || oldest.is_none()) {
                    let now = Instant::now();
                    oldest.get_or_insert(now);
                    newest = Some(now);
                }
                if pending_after == 0 {
                    oldest = None;
                    newest = None;
                }
                if stop {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                flush_snapshots(&mut ledger, vault_root.as_deref());
                oldest = None;
                newest = None;
            }
            Err(RecvTimeoutError::Disconnected) => {
                // The app is going away; save what is pending on the way out.
                flush_snapshots(&mut ledger, vault_root.as_deref());
                return;
            }
        }
    }
}

/// Runs one job. Returns whether the worker should stop.
fn handle_job(ledger: &mut Ledger, vault_root: Option<&Path>, job: LedgerJob) -> bool {
    match job {
        LedgerJob::Sync(facts) => sync_one(ledger, &facts),
        LedgerJob::SyncBatch(batch) => {
            for facts in &batch {
                sync_one(ledger, facts);
            }
        }
        LedgerJob::NoteGone(note_id) => {
            if let Err(err) = ledger.note_removed(&note_id, &now_utc()) {
                eprintln!("ledger could not retire note {note_id}: {err}");
            }
        }
        LedgerJob::RestoreIfEmpty => {
            let Some(root) = vault_root else {
                return false;
            };
            match ledger.restore_from_snapshots_if_empty(root) {
                Ok(report) if !report.restored => {}
                Ok(report) => {
                    if report.entries_restored > 0 {
                        eprintln!(
                            "ledger restored {} commitment(s) from {} vault snapshot(s)",
                            report.entries_restored, report.files_read
                        );
                    }
                    for warning in &report.warnings {
                        eprintln!("ledger restore: {warning}");
                    }
                }
                Err(err) => eprintln!("ledger restore failed: {err}"),
            }
        }
        LedgerJob::Flush(ack) => {
            flush_snapshots(ledger, vault_root);
            let _ = ack.send(());
        }
        // A caller that gave up waiting has dropped its receiver, so the send
        // fails and is ignored: the work is done either way, and the ledger is
        // the truth the next read converges on.
        LedgerJob::ListDetails { filter, reply } => {
            let _ = reply.send(ledger.list_details(&filter));
        }
        LedgerJob::Mutate { op, reply } => {
            let _ = reply.send(apply_mutation(ledger, op));
        }
        LedgerJob::DistillFollowUp {
            follow_up,
            autoclose_threshold,
            reply,
        } => {
            // The worker owns the clock and the database both, so the sync and
            // the classifications it feeds cannot interleave with a watcher
            // sync of the same note.
            let now = now_utc();
            let result = ledger::apply_distill_follow_up(
                ledger,
                &DistillFollowUp {
                    note_id: &follow_up.note_id,
                    project: &follow_up.project,
                    note_date_utc: &follow_up.note_date_utc,
                    items: &follow_up.items,
                    updates: &follow_up.updates,
                },
                autoclose_threshold,
                &now,
            );
            let _ = reply.send(result);
        }
        LedgerJob::NoteEnrollment { note_id, reply } => {
            let result = ledger
                .note_tracking_override(&note_id)
                .and_then(|mode| Ok((mode, ledger.entries_for_note(&note_id)?)));
            let _ = reply.send(result);
        }
        LedgerJob::SetNoteTracking {
            note_id,
            project,
            context_only,
            reply,
        } => {
            let now = now_utc();
            let result = ledger.set_note_tracking(&note_id, &project, context_only, &now);
            let _ = reply.send(result);
        }
        LedgerJob::TrackItem { request, reply } => {
            let now = now_utc();
            let result = ledger
                .track_item(
                    &request.note_id,
                    &request.item,
                    &request.project,
                    &request.note_date_utc,
                    &now,
                )
                // A promote is a person's judgement, so it counts as touching
                // the entry: no later tracking flip may undo it. Re-read after,
                // so the reply is the entry as it now stands.
                .and_then(|entry| {
                    ledger.mark_touched(&entry.entry_id)?;
                    Ok(ledger
                        .get_entry(&entry.entry_id)?
                        .map(|detail| detail.entry)
                        .unwrap_or(entry))
                });
            let _ = reply.send(result);
        }
    }
    false
}

/// Applies one mutation and gathers what the caller needs to finish in the
/// vault.
///
/// `now` is minted here rather than by the caller for the same reason
/// [`sync_one`] does it: kodabi-core never reads the clock, and the worker is
/// the shell-side owner of this database.
///
/// **Every successful op marks its entry touched**, and this is the one place
/// that happens. The two-handle split makes it exact: [`LedgerClient`] carries a
/// person's judgement and routes here, while the machine's paths ([`sync_one`],
/// the distill follow-up) never do. That is what lets a tracking flip know which
/// entries it may quietly override and which somebody has already decided about.
fn apply_mutation(ledger: &mut Ledger, op: LedgerOp) -> ledger::Result<MutateReply> {
    let now = now_utc();
    let entry_id = op.entry_id().to_string();

    let (entry, evidence) = match &op {
        LedgerOp::Close { entry_id, via } => (ledger.close(entry_id, *via, &now)?, None),
        LedgerOp::Waive { entry_id } => (ledger.waive(entry_id, &now)?, None),
        LedgerOp::Snooze { entry_id, until } => (ledger.snooze(entry_id, until, &now)?, None),
        LedgerOp::Reopen { entry_id } => (ledger.reopen(entry_id, &now)?, None),
        LedgerOp::ConfirmEvidence {
            entry_id,
            evidence_id,
        } => {
            let (entry, claim) = ledger.close_from_evidence(entry_id, evidence_id, &now)?;
            (entry, Some(claim))
        }
        LedgerOp::DismissEvidence {
            entry_id,
            evidence_id,
        } => (ledger.dismiss_evidence(entry_id, evidence_id, &now)?, None),
        LedgerOp::Untrack { entry_id } => {
            (ledger.untrack(entry_id, UntrackedVia::Manual, &now)?, None)
        }
    };

    ledger.mark_touched(&entry_id)?;

    // Read back rather than tracked through the mutation: the live ref is a
    // property of the entry, and an entry whose source line is gone has none.
    // The re-read also picks up the `touched` flag just set, so the reply is
    // the entry as it now stands rather than as the mutator left it.
    let detail = ledger.get_entry(&entry_id)?;
    let active_ref = detail.as_ref().and_then(|detail| {
        detail
            .item_refs
            .iter()
            .find(|item_ref| item_ref.active)
            .map(|item_ref| (item_ref.note_id.clone(), item_ref.item_id.clone()))
    });

    Ok(MutateReply {
        entry: detail.map(|detail| detail.entry).unwrap_or(entry),
        evidence,
        active_ref,
    })
}

/// Reconciles one note, logging a failure rather than propagating it — the
/// database is transactional, so a failed sync leaves the ledger consistent and
/// the next re-derivation of that note converges it.
fn sync_one(ledger: &mut Ledger, facts: &NoteFacts) {
    let now = now_utc();
    let result = ledger.sync_note_items(&NoteSync {
        note_id: &facts.note_id,
        project: &facts.project,
        note_date_utc: &facts.date_utc,
        items: &facts.items,
        link_hints: &[],
        now: &now,
    });
    match result {
        Ok(outcome) => {
            if !outcome.is_noop() {
                eprintln!(
                    "ledger {}: {} new, {} relinked, {} re-mentioned, {} to review",
                    facts.note_id,
                    outcome.created.len(),
                    outcome.relinked.len(),
                    outcome.rementioned.len(),
                    outcome.sent_to_review.len()
                );
            }
        }
        Err(err) => eprintln!("ledger sync failed for {}: {err}", facts.note_id),
    }
}

/// Writes every stale project's snapshot.
fn flush_snapshots(ledger: &mut Ledger, vault_root: Option<&Path>) {
    let Some(root) = vault_root else {
        return;
    };
    if let Err(err) = ledger.flush_snapshots(root) {
        // The project stays dirty, so the next flush retries it.
        eprintln!("ledger snapshot write failed: {err}");
    }
}

/// The project slug the ledger should file a note's commitments under. An
/// unfiled note's `None` becomes the Inbox sentinel, which is a real folder and
/// gets a real snapshot.
pub fn project_slug(project: Option<&str>) -> String {
    project.unwrap_or(INBOX).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kodabi_core::ledger::{EntryFilter, EntryState};
    use std::sync::mpsc::Sender;

    const QUIET: Duration = Duration::from_millis(60);
    const MAX_DELAY: Duration = Duration::from_millis(240);

    fn fact(id: &str, owner: &str, description: &str) -> ActionItemFact {
        ActionItemFact {
            id: id.to_string(),
            description: description.to_string(),
            owner: owner.to_string(),
            due_date: None,
            done: false,
            extracted_date: Some("2026-08-01".to_string()),
        }
    }

    fn facts_for(note_id: &str, project: &str, item: &str, description: &str) -> NoteFacts {
        NoteFacts {
            note_id: note_id.to_string(),
            project: project.to_string(),
            date_utc: "2026-08-01T00:00:00Z".to_string(),
            items: vec![fact(item, "Priya", description)],
        }
    }

    /// Runs the worker on a background thread against a fresh vault, returning
    /// the job sender and the vault dir.
    fn spawn_worker(vault: &tempfile::TempDir) -> (Sender<LedgerJob>, std::thread::JoinHandle<()>) {
        let ledger = Ledger::open_in_memory().unwrap();
        let root = vault.path().to_path_buf();
        let (sender, jobs) = mpsc::channel::<LedgerJob>();
        let handle =
            std::thread::spawn(move || run_worker(jobs, ledger, Some(root), QUIET, MAX_DELAY));
        (sender, handle)
    }

    /// Blocks until the worker acknowledges a flush, so a test never sleeps on a
    /// guess.
    fn flush_and_wait(sender: &Sender<LedgerJob>) {
        let (ack, done) = mpsc::channel();
        sender.send(LedgerJob::Flush(ack)).unwrap();
        done.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    /// Sends a job carrying a reply channel and waits for the answer, the way a
    /// command's `LedgerClient` does.
    fn request<T>(
        sender: &Sender<LedgerJob>,
        build: impl FnOnce(Sender<ledger::Result<T>>) -> LedgerJob,
    ) -> ledger::Result<T> {
        let (reply, answer) = mpsc::channel();
        sender.send(build(reply)).unwrap();
        answer.recv_timeout(Duration::from_secs(5)).unwrap()
    }

    /// Seeds one open entry through the worker and returns its id.
    fn seed_entry(sender: &Sender<LedgerJob>) -> String {
        sender
            .send(LedgerJob::Sync(Box::new(facts_for(
                "n_a1b2c3",
                "Ops",
                "a_111111",
                "send the deck",
            ))))
            .unwrap();
        let details = request(sender, |reply| LedgerJob::ListDetails {
            filter: EntryFilter::default(),
            reply,
        })
        .unwrap();
        assert_eq!(details.len(), 1);
        details[0].entry.entry_id.clone()
    }

    #[test]
    fn every_human_mutation_marks_its_entry_touched() {
        // The seam retro-application depends on. A tracking flip may override
        // an entry nobody has looked at and must never override one somebody
        // has, so the flag has to be set by the path a person's judgement takes
        // and by nothing else.
        let vault = tempfile::tempdir().unwrap();
        let (sender, worker) = spawn_worker(&vault);
        let entry_id = seed_entry(&sender);

        let details = request(&sender, |reply| LedgerJob::ListDetails {
            filter: EntryFilter::default(),
            reply,
        })
        .unwrap();
        assert!(
            !details[0].entry.touched,
            "a sync is the machine, not a person"
        );

        let reply = request(&sender, |reply| LedgerJob::Mutate {
            op: LedgerOp::Snooze {
                entry_id: entry_id.clone(),
                until: "2026-08-20".to_string(),
            },
            reply,
        })
        .unwrap();

        assert!(
            reply.entry.touched,
            "the reply carries the entry as it now is"
        );

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn an_untrack_through_the_worker_reaches_the_snapshot() {
        let vault = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(vault.path().join("Ops")).unwrap();
        let (sender, worker) = spawn_worker(&vault);
        let entry_id = seed_entry(&sender);

        let reply = request(&sender, |reply| LedgerJob::Mutate {
            op: LedgerOp::Untrack {
                entry_id: entry_id.clone(),
            },
            reply,
        })
        .unwrap();
        assert_eq!(reply.entry.state, EntryState::Untracked);
        assert_eq!(reply.entry.untracked_via, Some(UntrackedVia::Manual));
        // The refs stay live, which is what the caller needs to tell the note
        // view which line this was.
        assert!(reply.active_ref.is_some());

        flush_and_wait(&sender);
        let raw = std::fs::read_to_string(vault.path().join("Ops").join("_ledger.yml")).unwrap();
        assert!(raw.contains("state: untracked"));
        assert!(raw.contains("untracked_via: manual"));

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn a_tracking_flip_queued_before_a_sync_gates_that_sync() {
        // The ordering `set_meeting_tracking` relies on: it sets the mode and
        // then hands the same channel a re-sync, so the sync must see the new
        // mode rather than racing it.
        let vault = tempfile::tempdir().unwrap();
        let (sender, worker) = spawn_worker(&vault);

        let outcome = request(&sender, |reply| LedgerJob::SetNoteTracking {
            note_id: "n_a1b2c3".to_string(),
            project: "Ops".to_string(),
            context_only: true,
            reply,
        })
        .unwrap();
        assert!(outcome.context_only);

        sender
            .send(LedgerJob::Sync(Box::new(facts_for(
                "n_a1b2c3",
                "Ops",
                "a_111111",
                "send the deck",
            ))))
            .unwrap();

        let details = request(&sender, |reply| LedgerJob::ListDetails {
            filter: EntryFilter::default(),
            reply,
        })
        .unwrap();
        assert!(
            details.is_empty(),
            "Priya's line is not mine, so a context-only meeting never enrolls it"
        );

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn the_worker_answers_the_note_enrollment_read() {
        let vault = tempfile::tempdir().unwrap();
        let (sender, worker) = spawn_worker(&vault);
        seed_entry(&sender);

        let (mode, details) = request(&sender, |reply| LedgerJob::NoteEnrollment {
            note_id: "n_a1b2c3".to_string(),
            reply,
        })
        .unwrap();
        assert_eq!(mode, None, "a note with no override reads as the default");
        assert_eq!(details.len(), 1);

        request(&sender, |reply| LedgerJob::SetNoteTracking {
            note_id: "n_a1b2c3".to_string(),
            project: "Ops".to_string(),
            context_only: true,
            reply,
        })
        .unwrap();

        let (mode, details) = request(&sender, |reply| LedgerJob::NoteEnrollment {
            note_id: "n_a1b2c3".to_string(),
            reply,
        })
        .unwrap();
        assert_eq!(mode, Some(EnrollmentMode::ContextOnly));
        // Untracked by the flip, but still the note's entry: the panel has to
        // show it as untracked rather than as never enrolled.
        assert_eq!(details.len(), 1);
        assert_eq!(details[0].entry.state, EntryState::Untracked);

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn a_manual_track_is_recorded_as_a_persons_judgement() {
        let vault = tempfile::tempdir().unwrap();
        let (sender, worker) = spawn_worker(&vault);
        request(&sender, |reply| LedgerJob::SetNoteTracking {
            note_id: "n_a1b2c3".to_string(),
            project: "Ops".to_string(),
            context_only: true,
            reply,
        })
        .unwrap();
        sender
            .send(LedgerJob::Sync(Box::new(facts_for(
                "n_a1b2c3",
                "Ops",
                "a_111111",
                "send the deck",
            ))))
            .unwrap();

        let entry = request(&sender, |reply| LedgerJob::TrackItem {
            request: Box::new(TrackItemRequest {
                note_id: "n_a1b2c3".to_string(),
                project: "Ops".to_string(),
                note_date_utc: "2026-08-01T00:00:00Z".to_string(),
                item: fact("a_111111", "Priya", "send the deck"),
            }),
            reply,
        })
        .unwrap();

        assert_eq!(entry.state, EntryState::Open);
        assert!(
            entry.touched,
            "a promote is a judgement, so no later flip may undo it"
        );

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn the_worker_answers_a_hydrated_read() {
        let vault = tempfile::tempdir().unwrap();
        let (sender, worker) = spawn_worker(&vault);
        seed_entry(&sender);

        let details = request(&sender, |reply| LedgerJob::ListDetails {
            filter: EntryFilter {
                states: Some(vec![EntryState::Open]),
                ..EntryFilter::default()
            },
            reply,
        })
        .unwrap();

        assert_eq!(details.len(), 1);
        assert_eq!(details[0].entry.description, "send the deck");
        // Hydrated, not bare: the surface renders the live ref without a second
        // round trip.
        assert_eq!(details[0].item_refs.len(), 1);
        assert!(details[0].item_refs[0].active);

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn a_mutation_through_the_worker_reaches_the_snapshot() {
        // The reason mutations are routed through the worker at all: the
        // debounced snapshot writer watches what the worker dirties, so a
        // person's judgement has to travel the same path a sync does or it
        // would never reach `_ledger.yml`.
        let vault = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(vault.path().join("Ops")).unwrap();
        let (sender, worker) = spawn_worker(&vault);
        let entry_id = seed_entry(&sender);

        let reply = request(&sender, |reply| LedgerJob::Mutate {
            op: LedgerOp::Close {
                entry_id: entry_id.clone(),
                via: ClosedVia::Manual,
            },
            reply,
        })
        .unwrap();
        assert_eq!(reply.entry.state, EntryState::Closed);
        assert_eq!(reply.entry.closed_via, Some(ClosedVia::Manual));
        // The live ref rides along, so a caller can tick the box it names.
        assert_eq!(
            reply.active_ref,
            Some(("n_a1b2c3".to_string(), "a_111111".to_string()))
        );

        flush_and_wait(&sender);
        let snapshot =
            std::fs::read_to_string(vault.path().join("Ops").join("_ledger.yml")).unwrap();
        assert!(
            snapshot.contains("state: closed"),
            "the mutation never reached the vault snapshot: {snapshot}"
        );

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn a_snooze_and_its_undo_round_trip() {
        let vault = tempfile::tempdir().unwrap();
        let (sender, worker) = spawn_worker(&vault);
        let entry_id = seed_entry(&sender);

        let reply = request(&sender, |reply| LedgerJob::Mutate {
            op: LedgerOp::Snooze {
                entry_id: entry_id.clone(),
                until: "2026-09-01".to_string(),
            },
            reply,
        })
        .unwrap();
        assert_eq!(reply.entry.state, EntryState::Snoozed);
        assert_eq!(reply.entry.snoozed_until.as_deref(), Some("2026-09-01"));

        let reply = request(&sender, |reply| LedgerJob::Mutate {
            op: LedgerOp::Reopen {
                entry_id: entry_id.clone(),
            },
            reply,
        })
        .unwrap();
        assert_eq!(reply.entry.state, EntryState::Open);
        assert_eq!(reply.entry.snoozed_until, None);

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn a_bad_snooze_date_comes_back_as_the_ledgers_own_complaint() {
        // The wrapper passes this detail through as the user's words, so the
        // worker must not swallow it.
        let vault = tempfile::tempdir().unwrap();
        let (sender, worker) = spawn_worker(&vault);
        let entry_id = seed_entry(&sender);

        let err = request(&sender, |reply| LedgerJob::Mutate {
            op: LedgerOp::Snooze {
                entry_id: entry_id.clone(),
                until: "next Tuesday".to_string(),
            },
            reply,
        })
        .unwrap_err();

        assert!(matches!(
            err,
            ledger::LedgerError::InvalidField {
                field: "snoozed_until",
                ..
            }
        ));

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn an_unavailable_ledger_refuses_every_client_call() {
        // The failure posture this ticket owes: a session with no ledger must
        // say so, not drop a person's judgement on the floor.
        let state = LedgerState { sender: None };
        let client = state.client();

        assert!(!client.is_available());
        assert!(matches!(
            client.list_details(EntryFilter::default()),
            Err(LedgerCallError::Unavailable)
        ));
        assert!(matches!(
            client.mutate(LedgerOp::Waive {
                entry_id: "le_whatever".to_string()
            }),
            Err(LedgerCallError::Unavailable)
        ));
    }

    #[test]
    fn a_client_call_after_the_worker_stops_is_unavailable() {
        // The worker died but the handle outlived it. The send fails at once,
        // which is what keeps this from waiting out the whole reply timeout
        // before telling the user something they could have been told now.
        let (sender, jobs) = mpsc::channel::<LedgerJob>();
        let client = LedgerClient(Some(sender));
        drop(jobs);

        assert!(matches!(
            client.mutate(LedgerOp::Waive {
                entry_id: "le_whatever".to_string()
            }),
            Err(LedgerCallError::Unavailable)
        ));
        assert!(matches!(
            client.list_details(EntryFilter::default()),
            Err(LedgerCallError::Unavailable)
        ));
    }

    #[test]
    fn a_burst_of_syncs_produces_one_snapshot_per_project() {
        let vault = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(vault.path().join("Ops")).unwrap();
        let (sender, worker) = spawn_worker(&vault);

        // Three separate notes, as a watcher burst over three edited files
        // produces. (Three syncs of one note would be that note's item list
        // being replaced each time, which is a different thing entirely.)
        for (index, description) in ["send the deck", "book the venue", "call the club"]
            .iter()
            .enumerate()
        {
            sender
                .send(LedgerJob::Sync(Box::new(facts_for(
                    &format!("n_note{index}0"),
                    "Ops",
                    &format!("a_00000{index}"),
                    description,
                ))))
                .unwrap();
        }

        // The debounce writes on its own once the burst goes quiet.
        let path = vault.path().join("Ops").join("_ledger.yml");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !path.is_file() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(path.is_file(), "the debounce never fired");

        let snapshot = std::fs::read_to_string(&path).unwrap();
        assert_eq!(snapshot.matches("entry_id:").count(), 3);

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn a_flush_acknowledges_after_writing() {
        let vault = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(vault.path().join("Ops")).unwrap();
        let (sender, worker) = spawn_worker(&vault);

        sender
            .send(LedgerJob::Sync(Box::new(facts_for(
                "n_a1b2c3",
                "Ops",
                "a_111111",
                "send the deck",
            ))))
            .unwrap();
        flush_and_wait(&sender);

        // The ack is the guarantee: the file is on disk by the time it arrives,
        // with no sleeping involved.
        assert!(vault.path().join("Ops").join("_ledger.yml").is_file());

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn only_dirty_projects_are_rewritten() {
        let vault = tempfile::tempdir().unwrap();
        for project in ["Ops", "Growth"] {
            std::fs::create_dir_all(vault.path().join(project)).unwrap();
        }
        let (sender, worker) = spawn_worker(&vault);

        sender
            .send(LedgerJob::Sync(Box::new(facts_for(
                "n_a1b2c3",
                "Ops",
                "a_111111",
                "send the deck",
            ))))
            .unwrap();
        flush_and_wait(&sender);
        assert!(vault.path().join("Ops").join("_ledger.yml").is_file());
        assert!(
            !vault.path().join("Growth").join("_ledger.yml").exists(),
            "a project with no entries is never given a file"
        );

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn restore_runs_before_the_first_sync() {
        // Seed a vault snapshot, then queue RestoreIfEmpty *and* a sync behind
        // it. If the sync ran first the database would be non-empty and the
        // restore would be skipped forever, so the entry from the snapshot is
        // the proof of ordering.
        let vault = tempfile::tempdir().unwrap();
        let dir = vault.path().join("Ops");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("_ledger.yml"),
            "version: 1\nentries:\n\
             - entry_id: le_aaaaaaaaaaaa\n  \
               state: waived\n  \
               direction: theirs\n  \
               owner: Priya\n  \
               description: an old promise\n  \
               created_at: 2026-07-01T00:00:00Z\n  \
               updated_at: 2026-07-01T00:00:00Z\n  \
               last_mention: 2026-07-01T00:00:00Z\n",
        )
        .unwrap();

        let ledger = Ledger::open_in_memory().unwrap();
        let root = vault.path().to_path_buf();
        let (sender, jobs) = mpsc::channel::<LedgerJob>();
        // Both queued before the worker starts, so their order in the channel is
        // exactly the order `initialize` guarantees.
        sender.send(LedgerJob::RestoreIfEmpty).unwrap();
        sender
            .send(LedgerJob::Sync(Box::new(facts_for(
                "n_a1b2c3",
                "Ops",
                "a_111111",
                "send the deck",
            ))))
            .unwrap();
        let worker =
            std::thread::spawn(move || run_worker(jobs, ledger, Some(root), QUIET, MAX_DELAY));
        // Dropping the sender disconnects the channel, which flushes on the way
        // out; joining is then a deterministic wait for that flush.
        drop(sender);
        worker.join().unwrap();

        // Both the restored entry and the freshly synced one are in the file.
        let written = std::fs::read_to_string(dir.join("_ledger.yml")).unwrap();
        assert!(written.contains("le_aaaaaaaaaaaa"), "{written}");
        assert!(written.contains("an old promise"), "{written}");
        assert!(written.contains("send the deck"), "{written}");
    }

    #[test]
    fn note_gone_flags_entries_without_deleting_them() {
        let vault = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(vault.path().join("Ops")).unwrap();
        let mut ledger = Ledger::open_in_memory().unwrap();

        // Drive the ledger directly: this asserts the worker's semantics, not
        // its timing.
        let facts = facts_for("n_a1b2c3", "Ops", "a_111111", "send the deck");
        sync_one(&mut ledger, &facts);
        assert_eq!(
            ledger.list_entries(&EntryFilter::default()).unwrap().len(),
            1
        );

        handle_job(
            &mut ledger,
            Some(vault.path()),
            LedgerJob::NoteGone("n_a1b2c3".to_string()),
        );

        let entries = ledger.list_entries(&EntryFilter::default()).unwrap();
        assert_eq!(entries.len(), 1, "a commitment outlives its note");
        assert_eq!(entries[0].state, EntryState::NeedsReview);
    }

    #[test]
    fn an_unfiled_note_files_its_commitments_under_the_inbox() {
        assert_eq!(project_slug(None), INBOX);
        assert_eq!(project_slug(Some("Growth/Q3")), "Growth/Q3");
    }

    #[test]
    fn a_distill_follow_up_syncs_and_classifies_in_one_pass() {
        let vault = tempfile::tempdir().unwrap();
        let (sender, handle) = spawn_worker(&vault);
        let entry_id = seed_entry(&sender);

        // A later conversation reporting the commitment done, confidently.
        let applied = request(&sender, |reply| LedgerJob::DistillFollowUp {
            follow_up: Box::new(OwnedFollowUp {
                note_id: "n_d4e5f6".to_string(),
                project: "Ops".to_string(),
                note_date_utc: "2026-08-19T00:00:00Z".to_string(),
                items: Vec::new(),
                updates: vec![LedgerUpdateDraft {
                    entry_id: entry_id.clone(),
                    kind: kodabi_core::distill::LedgerUpdateKind::Completed,
                    item: None,
                    confidence: 0.95,
                    quote: None,
                }],
            }),
            autoclose_threshold: 0.8,
            reply,
        })
        .unwrap();

        assert_eq!(applied.auto_closed.len(), 1);
        // The worker mints `now` itself, so the caller never passes a clock in.
        assert!(!applied.auto_closed[0].entry.updated_at.is_empty());

        let entries = request(&sender, |reply| LedgerJob::ListDetails {
            filter: EntryFilter {
                states: Some(vec![EntryState::Closed]),
                ..EntryFilter::default()
            },
            reply,
        })
        .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].entry.entry_id, entry_id);

        flush_and_wait(&sender);
        drop(sender);
        handle.join().unwrap();
    }
}
