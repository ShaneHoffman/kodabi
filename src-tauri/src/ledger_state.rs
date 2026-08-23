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
    Evidence, Ledger, LedgerEntry, NoteSync, NoteTrackingOutcome, OwnerIdentity,
    OwnerResolutionOutcome, RetroSource, UntrackedVia, LEDGER_DB_FILE,
};
use kodabi_core::meeting::ActionItemFact;
use kodabi_core::note::INBOX;
use tauri::{AppHandle, Emitter, Manager};

use crate::events::LEDGER_CHANGED_EVENT;
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
    /// The note's frontmatter tracking override, or `None` to inherit. Read
    /// from the index row the facts were derived from, which is itself read
    /// from the note file — see `ledger::sync::NoteSync::note_override`.
    pub note_override: Option<EnrollmentMode>,
    /// The default this note's meeting category carries, resolved against the
    /// user's settings before the facts left the shell — see
    /// `ledger::sync::NoteSync::category_default`.
    ///
    /// Resolved here rather than in the worker because the worker holds no
    /// `AppHandle` and so cannot reach `SettingsState`; this is the same shape
    /// as `LedgerJob::DistillFollowUp`'s `autoclose_threshold`.
    pub category_default: Option<EnrollmentMode>,
    /// Who the local user is, resolved from `SettingsState` at the same
    /// boundary and for the same reason — see `ledger::sync::NoteSync::identity`.
    pub identity: OwnerIdentity,
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
    /// Re-file as the local user's own, because they said so. Only the
    /// direction moves: a claim corrects who a commitment belongs to, never
    /// where it stands.
    ClaimMine { entry_id: String },
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
            | LedgerOp::Untrack { entry_id }
            | LedgerOp::ClaimMine { entry_id } => entry_id,
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

/// What a batched gesture settled on: the entries it moved, and the ones it
/// could not, each with the reason.
///
/// **Best-effort by design, not by omission.** A bulk untrack of a day's
/// enrollments is a sweep over rows the user is looking at, and the ledger's
/// transition table legitimately refuses some of them — a needs-review entry
/// cannot be snoozed, a closed one cannot be untracked. Failing the whole
/// gesture because one row moved on since the view rendered would make the
/// button unusable exactly when there is most to clear; the caller reports the
/// remainder instead.
///
/// Nothing here is atomic across entries, and nothing needs to be: each op is
/// independently meaningful, and a partial sweep leaves the ledger in a state
/// the user can see and finish.
#[derive(Debug, Default)]
pub struct MutateManyReply {
    pub applied: Vec<MutateReply>,
    /// `(entry_id, why)` for each op the ledger declined.
    pub skipped: Vec<(String, ledger::LedgerError)>,
}

/// What the ledger remembers about the daily digest, read as one answer.
///
/// Both halves are `Option` and independently so: `last_run` absent means the
/// digest has never run on this device, while a `last_run` with no `payload`
/// means it ran and found nothing worth reporting. The command tells those
/// apart, and they mean different things to a first launch.
#[derive(Debug, Default)]
pub struct DigestState {
    pub last_run: Option<String>,
    pub payload: Option<String>,
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
    /// Answer whether this session still owes a first-run backfill, and clear
    /// the flag. See [`LedgerHandle::take_startup_backfill`].
    TakeStartupBackfill { reply: Sender<bool> },
    /// Graduate any tracking override still parked in the legacy
    /// `ledger_note_overrides` table into its note's frontmatter.
    DrainLegacyOverrides,
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
    /// Apply several mutations as one gesture, replying with what landed and
    /// what was declined. See [`MutateManyReply`].
    MutateMany {
        ops: Vec<LedgerOp>,
        reply: Sender<ledger::Result<MutateManyReply>>,
    },
    /// Read the triage marker: when the user last reviewed newly enrolled
    /// commitments, or `None` on a ledger that has never been reviewed.
    TriageLastSeen {
        reply: Sender<ledger::Result<Option<String>>>,
    },
    /// Advance the triage marker to `seen_through`, keeping the later of the
    /// two. Never announces `ledger:changed`: no commitment changed, and a
    /// refetch would race the strip's own batch out from under the user.
    MarkTriageSeen {
        seen_through: String,
        reply: Sender<ledger::Result<()>>,
    },
    /// Set the triage marker to now, but only if it was never set.
    ///
    /// Queued once at startup, before any handle escapes, so a ledger that
    /// predates triage does not greet its owner with every commitment it has
    /// ever held. Everything already enrolled counts as seen; only what arrives
    /// afterwards is new.
    SeedTriageMarker,
    /// Read the daily digest's two keys together: the marker saying when it
    /// last ran, and the digest that run produced.
    ///
    /// One job rather than two because the pair is only meaningful together —
    /// a marker read separately from its payload could be answered either side
    /// of a [`LedgerJob::StoreDigest`] and describe two different days.
    DigestState {
        reply: Sender<ledger::Result<DigestState>>,
    },
    /// Record a digest run: store what it produced, then advance the marker.
    ///
    /// In that order on purpose. The marker is what makes a digest not run
    /// again today, so advancing it first would mean a crash in between left
    /// the day marked done with nothing to show for it. Storing first can at
    /// worst recompute, which is idempotent.
    StoreDigest {
        run_at: String,
        payload: String,
        reply: Sender<ledger::Result<()>>,
    },
    /// Sync a freshly distilled note and apply what its conversation said
    /// about commitments the ledger already held.
    DistillFollowUp {
        follow_up: Box<OwnedFollowUp>,
        autoclose_threshold: f64,
        reply: Sender<ledger::Result<AppliedUpdates>>,
    },
    /// Read the entries one note produced, for the note view's enrollment
    /// panel. The panel's *mode* no longer comes from here: it is a frontmatter
    /// key, so the caller reads it from the note's index row.
    NoteEntries {
        note_id: String,
        reply: Sender<ledger::Result<Vec<EntryDetail>>>,
    },
    /// Re-evaluate the entries a note already produced, after its effective
    /// enrollment mode changed — its frontmatter override was flipped, or it was
    /// recategorized. The vault write happens first, in the command; this is
    /// only its consequence.
    RetroApplyNoteTracking {
        note_id: String,
        project: String,
        context_only: bool,
        source: RetroSource,
        reply: Sender<ledger::Result<NoteTrackingOutcome>>,
    },
    /// Promote one extracted line by hand, whatever the mode says.
    TrackItem {
        request: Box<TrackItemRequest>,
        reply: Sender<ledger::Result<LedgerEntry>>,
    },
    /// Re-evaluate which open commitments are the user's own, after the
    /// configured identity changed. The settings write happens first, in the
    /// command; this is only its consequence.
    RetroResolveOwners {
        identity: OwnerIdentity,
        reply: Sender<ledger::Result<OwnerResolutionOutcome>>,
    },
}

/// One manual promote, owned so it can cross the channel.
pub struct TrackItemRequest {
    pub note_id: String,
    pub project: String,
    pub note_date_utc: String,
    pub item: ActionItemFact,
    /// Who the local user is, for the direction the promoted entry is minted
    /// with.
    pub identity: OwnerIdentity,
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
    /// The note's frontmatter tracking override, taken from the `Note` the
    /// distill just wrote.
    pub note_override: Option<EnrollmentMode>,
    /// The default that note's meeting category resolved to, read from the
    /// same `Note` against the user's settings.
    pub category_default: Option<EnrollmentMode>,
    /// Who the local user is, from the same settings snapshot.
    pub identity: OwnerIdentity,
}

/// A handle to the background ledger worker, held as Tauri managed state.
pub struct LedgerState {
    /// `None` when the database failed to open (logged at startup).
    sender: Option<Mutex<Sender<LedgerJob>>>,
}

/// A cloneable write handle, for the index worker to forward facts through.
///
/// Every method is fire-and-forget and silently no-ops when the ledger is
/// unavailable, so the index worker never has to branch on it — with the single
/// exception of [`take_startup_backfill`](LedgerHandle::take_startup_backfill),
/// which asks a question and therefore has to wait for the answer. It is safe
/// for the index worker to block on: it is asked once per session, from that
/// worker's own thread, and it answers `false` rather than hanging if the ledger
/// never opened.
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

    /// Whether this session still owes a first-run backfill — claimed, so it
    /// answers `true` at most once.
    ///
    /// The handshake exists because neither worker can answer the question
    /// alone: only the ledger worker knows whether the database came up empty,
    /// and only the index worker can read the notes that would fill it. FIFO
    /// order is what makes the answer trustworthy — `RestoreIfEmpty` is queued
    /// before any handle escapes `initialize`, so by the time this request is
    /// served the restore has already run and set the flag.
    ///
    /// `false` on an unavailable ledger or a worker that does not answer
    /// within `REPLY_TIMEOUT`, because a blocked index worker would stall the
    /// whole reconcile. A timed-out answer is **not** free: the worker serves
    /// the request regardless and clears the flag, and by the next launch the
    /// ledger may no longer be empty, so the seed would never be re-offered.
    /// The caller therefore asks this *before* queueing any batch of its own —
    /// see `index_state::run_reconcile` — which is what keeps the wait a bare
    /// round trip on an otherwise idle queue.
    pub fn take_startup_backfill(&self) -> bool {
        let Some(sender) = &self.0 else {
            return false;
        };
        let (reply, answers) = mpsc::channel();
        if sender
            .send(LedgerJob::TakeStartupBackfill { reply })
            .is_err()
        {
            return false;
        }
        answers.recv_timeout(REPLY_TIMEOUT).unwrap_or(false)
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

    /// Applies several mutations as one gesture. One job, one transaction per
    /// entry, one reply — so the caller broadcasts `ledger:changed` once and
    /// the snapshot debounce arms once, however many rows moved.
    pub fn mutate_many(
        &self,
        ops: Vec<LedgerOp>,
    ) -> std::result::Result<MutateManyReply, LedgerCallError> {
        self.request(|reply| LedgerJob::MutateMany { ops, reply })
    }

    /// When the user last reviewed newly enrolled commitments.
    pub fn triage_last_seen(&self) -> std::result::Result<Option<String>, LedgerCallError> {
        self.request(|reply| LedgerJob::TriageLastSeen { reply })
    }

    /// Advances the triage marker, keeping the later of the stored and given
    /// instants.
    pub fn mark_triage_seen(
        &self,
        seen_through: String,
    ) -> std::result::Result<(), LedgerCallError> {
        self.request(|reply| LedgerJob::MarkTriageSeen {
            seen_through,
            reply,
        })
    }

    /// The daily digest's marker and stored payload, read together.
    pub fn digest_state(&self) -> std::result::Result<DigestState, LedgerCallError> {
        self.request(|reply| LedgerJob::DigestState { reply })
    }

    /// Records a digest run: the payload it produced, then the marker saying
    /// the day is done.
    pub fn store_digest(
        &self,
        run_at: String,
        payload: String,
    ) -> std::result::Result<(), LedgerCallError> {
        self.request(|reply| LedgerJob::StoreDigest {
            run_at,
            payload,
            reply,
        })
    }

    /// The entries one note produced. Its tracking *mode* is frontmatter, and
    /// reaches the caller through the index row instead.
    pub fn note_entries(
        &self,
        note_id: String,
    ) -> std::result::Result<Vec<EntryDetail>, LedgerCallError> {
        self.request(|reply| LedgerJob::NoteEntries { note_id, reply })
    }

    /// Re-evaluates the entries a note already produced, after its effective
    /// enrollment mode changed. The caller writes the note first, and passes the
    /// *effective* mode plus which half of the chain decided it.
    pub fn retro_apply_note_tracking(
        &self,
        note_id: String,
        project: String,
        context_only: bool,
        source: RetroSource,
    ) -> std::result::Result<NoteTrackingOutcome, LedgerCallError> {
        self.request(|reply| LedgerJob::RetroApplyNoteTracking {
            note_id,
            project,
            context_only,
            source,
            reply,
        })
    }

    /// Re-evaluates which open commitments are the user's own, after the
    /// configured identity changed. The caller saves the settings first.
    pub fn retro_resolve_owners(
        &self,
        identity: OwnerIdentity,
    ) -> std::result::Result<OwnerResolutionOutcome, LedgerCallError> {
        self.request(|reply| LedgerJob::RetroResolveOwners { identity, reply })
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
        // The worker's voice. Machine ingest reaches this database with no
        // command in flight, so without this a re-file, a delete, or a watcher
        // reconcile would leave an open Commitments view showing yesterday.
        let announcer = app.clone();
        std::thread::spawn(move || {
            run_worker(
                jobs,
                ledger,
                vault_root,
                SNAPSHOT_QUIET,
                SNAPSHOT_MAX_DELAY,
                move || {
                    let _ = announcer.emit(LEDGER_CHANGED_EVENT, ());
                },
            )
        });

        // Queued before any handle escapes this function, so it is the worker's
        // first job no matter how quickly the index starts syncing. That
        // ordering is what makes "restore only into an empty database" hold:
        // the first sync would otherwise make the database non-empty and defeat
        // the restore permanently.
        let _ = sender.send(LedgerJob::RestoreIfEmpty);
        // Behind the restore on the same FIFO, so a pre-graduation `_ledger.yml`
        // has already put its rows in the table by the time the drain reads it.
        let _ = sender.send(LedgerJob::DrainLegacyOverrides);
        // Last of the three, and behind the restore for the same reason: a
        // ledger rebuilt from vault snapshots must have its rows in place
        // before "everything already here counts as seen" is stamped, or the
        // first triage strip would list the user's entire history back at them.
        let _ = sender.send(LedgerJob::SeedTriageMarker);

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

/// Where `ledger.db` lives: the app config dir, beside `settings.toml`.
///
/// Resolved through [`crate::sandbox::config_dir`] rather than Tauri's
/// `app_config_dir` directly, which is the whole reason `KODABI_SANDBOX`
/// relocates it for free: an agent-driven launch must never write commitments
/// into the user's real data.
///
/// `KODABI_LEDGER_DB` overrides it, mirroring [`crate::index_state::index_db_path`]
/// and for the same reason: `terminal_cmds::write_mcp_config` hands this path to
/// the `kodabi-mcp` sidecar, which has no `AppHandle` and so cannot resolve a
/// config dir for itself. One resolver on both sides is what keeps the sidecar's
/// commitments and the app's from being two different databases. Under
/// `KODABI_SANDBOX` the variable is set by activation, and setting it *by hand*
/// alongside the switch is refused outright (`kodabi_core::sandbox::resolve`).
pub(crate) fn ledger_db_path(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(value) =
        std::env::var_os(crate::sandbox::LEDGER_DB_ENV).filter(|value| !value.is_empty())
    {
        return Ok(PathBuf::from(value));
    }
    Ok(crate::sandbox::config_dir(app)?.join(LEDGER_DB_FILE))
}

/// Opens (creating if absent) the ledger database at [`ledger_db_path`].
fn open_ledger(app: &AppHandle) -> Option<Ledger> {
    let path = match ledger_db_path(app) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("failed to resolve the commitment ledger path: {err}");
            return None;
        }
    };
    // First launch has no config dir yet — and an overridden path may name a
    // directory that does not exist either.
    if let Some(parent) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            eprintln!("failed to create the config directory for the ledger: {err}");
            return None;
        }
    }
    match Ledger::open(&path) {
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
///
/// `on_changed` announces machine ingest to the rest of the app — see
/// [`handle_job`] for which jobs count and why the worker is the one that
/// speaks. It is called with no lock held and after the write has committed.
fn run_worker(
    jobs: Receiver<LedgerJob>,
    mut ledger: Ledger,
    vault_root: Option<PathBuf>,
    quiet: Duration,
    max_delay: Duration,
    on_changed: impl Fn(),
) {
    // When the oldest un-flushed change arrived, and the most recent one.
    let mut oldest: Option<Instant> = None;
    let mut newest: Option<Instant> = None;
    // Whether this session still owes a first-run backfill. Set once by
    // `RestoreIfEmpty` (the first job queued), read once by the index worker
    // through `take_startup_backfill`. A worker local rather than shared state
    // because FIFO order over this channel is what makes the answer meaningful.
    let mut backfill_pending = false;

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
                let changed = handle_job(
                    &mut ledger,
                    vault_root.as_deref(),
                    &mut backfill_pending,
                    job,
                );
                let pending_after = ledger.dirty_projects().len();
                if changed {
                    on_changed();
                }

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

/// Runs one job. Returns whether it wrote something the rest of the app should
/// hear about, i.e. whether to announce `ledger:changed`.
///
/// **The rule: a job with no reply channel is machine ingest, and the worker
/// announces it; a job carrying a reply belongs to a command, which announces
/// for itself once it has the answer.** Machine ingest has no other voice — the
/// watcher's reconcile, a re-file's re-sync, a delete's retirement and the
/// startup restore all reach this database without any command being in flight,
/// and before this the Commitments view simply went stale until something else
/// happened to refetch it.
///
/// Announcing from the worker also gets the ordering right for free: the event
/// fires strictly *after* the write lands. The alternative — having the commands
/// emit — would have to either emit early (announcing a change the reader cannot
/// see yet) or block the command on the worker queue, which can be a whole-vault
/// batch away.
fn handle_job(
    ledger: &mut Ledger,
    vault_root: Option<&Path>,
    backfill_pending: &mut bool,
    job: LedgerJob,
) -> bool {
    match job {
        LedgerJob::Sync(facts) => return sync_one(ledger, &facts),
        LedgerJob::SyncBatch(batch) => {
            // Fold rather than short-circuit: every note in the batch must sync,
            // and any one of them writing is enough to announce.
            return batch
                .iter()
                .fold(false, |changed, facts| sync_one(ledger, facts) | changed);
        }
        LedgerJob::NoteGone(note_id) => match ledger.note_removed(&note_id, &now_utc()) {
            Ok(outcome) => return !outcome.is_noop(),
            Err(err) => {
                eprintln!("ledger could not retire note {note_id}: {err}");
            }
        },
        LedgerJob::RestoreIfEmpty => {
            let Some(root) = vault_root else {
                return false;
            };
            let restored = match ledger.restore_from_snapshots_if_empty(root) {
                Ok(report) if !report.restored => false,
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
                    report.entries_restored > 0
                }
                Err(err) => {
                    eprintln!("ledger restore failed: {err}");
                    false
                }
            };
            // Whatever the restore did or did not find, the answer to "is this
            // ledger still empty?" is settled *now*, before the first sync can
            // put anything in it. That is the whole reason this job is queued
            // before any handle escapes `initialize`.
            *backfill_pending = matches!(ledger.is_empty(), Ok(true));
            return restored;
        }
        LedgerJob::TakeStartupBackfill { reply } => {
            let _ = reply.send(*backfill_pending);
            // Consume-once: the seed is idempotent, but asking twice in one
            // session would log a second, misleading count.
            *backfill_pending = false;
        }
        LedgerJob::DrainLegacyOverrides => {
            // A one-time move of the per-meeting tracking override out of this
            // database and into the notes themselves; the whole contract lives
            // in `kodabi_core::ledger`. Best-effort, like the restore above: a
            // failure leaves the rows for the next launch.
            let Some(root) = vault_root else {
                return false;
            };
            match ledger.drain_legacy_note_overrides(root, &now_utc()) {
                Ok(outcome) if outcome == ledger::DrainOutcome::default() => {}
                Ok(outcome) => {
                    eprintln!(
                        "ledger: moved {} tracking override(s) into their notes ({} dropped, {} deferred)",
                        outcome.graduated, outcome.discarded, outcome.deferred
                    );
                    return outcome.graduated > 0;
                }
                Err(err) => {
                    eprintln!("ledger: couldn't drain the legacy tracking overrides: {err}")
                }
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
                    note_override: follow_up.note_override,
                    category_default: follow_up.category_default,
                    identity: &follow_up.identity,
                },
                autoclose_threshold,
                &now,
            );
            let _ = reply.send(result);
        }
        LedgerJob::NoteEntries { note_id, reply } => {
            let _ = reply.send(ledger.entries_for_note(&note_id));
        }
        LedgerJob::RetroApplyNoteTracking {
            note_id,
            project,
            context_only,
            source,
            reply,
        } => {
            let now = now_utc();
            let result =
                ledger.retro_apply_note_tracking(&note_id, &project, context_only, source, &now);
            let _ = reply.send(result);
        }
        LedgerJob::RetroResolveOwners { identity, reply } => {
            let now = now_utc();
            let _ = reply.send(ledger.retro_resolve_owners(&identity, &now));
        }
        LedgerJob::MutateMany { ops, reply } => {
            let mut result = MutateManyReply::default();
            for op in ops {
                let entry_id = op.entry_id().to_string();
                match apply_mutation(ledger, op) {
                    Ok(applied) => result.applied.push(applied),
                    Err(err) => result.skipped.push((entry_id, err)),
                }
            }
            let _ = reply.send(Ok(result));
        }
        LedgerJob::TriageLastSeen { reply } => {
            let _ = reply.send(ledger.meta_get(ledger::TRIAGE_LAST_SEEN_KEY));
        }
        LedgerJob::MarkTriageSeen {
            seen_through,
            reply,
        } => {
            let _ = reply.send(ledger.meta_advance(ledger::TRIAGE_LAST_SEEN_KEY, &seen_through));
        }
        LedgerJob::SeedTriageMarker => {
            match ledger.meta_get(ledger::TRIAGE_LAST_SEEN_KEY) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    if let Err(err) = ledger.meta_advance(ledger::TRIAGE_LAST_SEEN_KEY, &now_utc())
                    {
                        eprintln!("ledger could not seed the triage marker: {err}");
                    }
                }
                Err(err) => eprintln!("ledger could not read the triage marker: {err}"),
            }
            // Viewing state, not a commitment: nothing to announce.
        }
        LedgerJob::DigestState { reply } => {
            let state = ledger
                .meta_get(ledger::DIGEST_LAST_RUN_KEY)
                .and_then(|last_run| {
                    ledger
                        .meta_get(ledger::DIGEST_PAYLOAD_KEY)
                        .map(|payload| DigestState { last_run, payload })
                });
            let _ = reply.send(state);
        }
        LedgerJob::StoreDigest {
            run_at,
            payload,
            reply,
        } => {
            let stored = ledger
                .meta_set(ledger::DIGEST_PAYLOAD_KEY, &payload)
                .and_then(|()| ledger.meta_advance(ledger::DIGEST_LAST_RUN_KEY, &run_at));
            let _ = reply.send(stored);
        }
        LedgerJob::TrackItem { request, reply } => {
            let now = now_utc();
            let result = ledger
                .track_item(
                    &request.note_id,
                    &request.item,
                    &request.project,
                    &request.note_date_utc,
                    &request.identity,
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
        LedgerOp::ClaimMine { entry_id } => (ledger.claim_mine(entry_id, &now)?, None),
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
///
/// Returns whether anything was written, which is what the worker announces on.
fn sync_one(ledger: &mut Ledger, facts: &NoteFacts) -> bool {
    let now = now_utc();
    let result = ledger.sync_note_items(&NoteSync {
        note_id: &facts.note_id,
        project: &facts.project,
        note_date_utc: &facts.date_utc,
        items: &facts.items,
        link_hints: &[],
        note_override: facts.note_override,
        category_default: facts.category_default,
        identity: &facts.identity,
        now: &now,
    });
    match result {
        Ok(outcome) => {
            if outcome.is_noop() {
                return false;
            }
            eprintln!(
                "ledger {}: {} new, {} relinked, {} re-mentioned, {} to review, {} moved",
                facts.note_id,
                outcome.created.len(),
                outcome.relinked.len(),
                outcome.rementioned.len(),
                outcome.sent_to_review.len(),
                outcome.moved.len()
            );
            true
        }
        Err(err) => {
            eprintln!("ledger sync failed for {}: {err}", facts.note_id);
            false
        }
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
    use kodabi_core::ledger::{Direction, EntryFilter, EntryState};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::Arc;

    const QUIET: Duration = Duration::from_millis(60);
    const MAX_DELAY: Duration = Duration::from_millis(240);

    fn fact(id: &str, owner: &str, description: &str) -> ActionItemFact {
        ActionItemFact {
            id: id.to_string(),
            description: description.to_string(),
            owner: owner.to_string(),
            due_date: None,
            done: false,
            firm: true,
            extracted_date: Some("2026-08-01".to_string()),
        }
    }

    fn facts_for(note_id: &str, project: &str, item: &str, description: &str) -> NoteFacts {
        NoteFacts {
            identity: OwnerIdentity::default(),
            note_id: note_id.to_string(),
            project: project.to_string(),
            date_utc: "2026-08-01T00:00:00Z".to_string(),
            items: vec![fact(item, "Priya", description)],
            note_override: None,
            category_default: None,
        }
    }

    /// Runs the worker on a background thread against a fresh vault, returning
    /// the job sender and the vault dir.
    fn spawn_worker(vault: &tempfile::TempDir) -> (Sender<LedgerJob>, std::thread::JoinHandle<()>) {
        let (sender, handle, _) = spawn_counting_worker(vault);
        (sender, handle)
    }

    /// The same, plus a counter of how many times the worker announced a
    /// change — standing in for the `ledger:changed` emit, which needs an
    /// `AppHandle` a unit test has no way to build.
    fn spawn_counting_worker(
        vault: &tempfile::TempDir,
    ) -> (
        Sender<LedgerJob>,
        std::thread::JoinHandle<()>,
        Arc<AtomicUsize>,
    ) {
        let ledger = Ledger::open_in_memory().unwrap();
        let root = vault.path().to_path_buf();
        let (sender, jobs) = mpsc::channel::<LedgerJob>();
        let announced = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&announced);
        let handle = std::thread::spawn(move || {
            run_worker(jobs, ledger, Some(root), QUIET, MAX_DELAY, move || {
                counter.fetch_add(1, Ordering::SeqCst);
            })
        });
        (sender, handle, announced)
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

    /// Seeds `count` open entries in one project and returns their ids.
    fn seed_entries(sender: &Sender<LedgerJob>, count: usize) -> Vec<String> {
        for index in 0..count {
            sender
                .send(LedgerJob::Sync(Box::new(facts_for(
                    &format!("n_note{index:02}"),
                    "Ops",
                    &format!("a_item{index:02}"),
                    &format!("send deck {index}"),
                ))))
                .unwrap();
        }
        let details = request(sender, |reply| LedgerJob::ListDetails {
            filter: EntryFilter::default(),
            reply,
        })
        .unwrap();
        assert_eq!(details.len(), count);
        details
            .into_iter()
            .map(|detail| detail.entry.entry_id)
            .collect()
    }

    #[test]
    fn a_batch_untrack_lands_every_eligible_entry_and_reports_the_rest() {
        let vault = tempfile::tempdir().unwrap();
        let (sender, worker) = spawn_worker(&vault);
        let ids = seed_entries(&sender, 3);

        // One of them is already settled by a real judgement, which the
        // transition table refuses to untrack. The other two must still land:
        // a sweep over rows the view rendered a moment ago will always race
        // something, and failing the gesture whole would make it unusable.
        request(&sender, |reply| LedgerJob::Mutate {
            op: LedgerOp::Waive {
                entry_id: ids[1].clone(),
            },
            reply,
        })
        .unwrap();

        let result = request(&sender, |reply| LedgerJob::MutateMany {
            ops: ids
                .iter()
                .map(|entry_id| LedgerOp::Untrack {
                    entry_id: entry_id.clone(),
                })
                .collect(),
            reply,
        })
        .unwrap();

        assert_eq!(result.applied.len(), 2);
        assert_eq!(result.skipped.len(), 1);
        assert_eq!(result.skipped[0].0, ids[1]);

        let untracked = request(&sender, |reply| LedgerJob::ListDetails {
            filter: EntryFilter {
                states: Some(vec![EntryState::Untracked]),
                ..EntryFilter::default()
            },
            reply,
        })
        .unwrap();
        assert_eq!(untracked.len(), 2);
        // A person's own untrack, so a later meeting re-track leaves it alone.
        assert!(untracked.iter().all(|detail| detail.entry.touched));
        assert!(untracked
            .iter()
            .all(|detail| detail.entry.untracked_via == Some(UntrackedVia::Manual)));

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn a_batch_snooze_skips_an_entry_the_transition_table_refuses() {
        let vault = tempfile::tempdir().unwrap();
        let (sender, worker) = spawn_worker(&vault);
        let ids = seed_entries(&sender, 2);

        // A closed entry cannot be snoozed; the open one still sleeps.
        request(&sender, |reply| LedgerJob::Mutate {
            op: LedgerOp::Close {
                entry_id: ids[0].clone(),
                via: ClosedVia::Manual,
            },
            reply,
        })
        .unwrap();

        let result = request(&sender, |reply| LedgerJob::MutateMany {
            ops: ids
                .iter()
                .map(|entry_id| LedgerOp::Snooze {
                    entry_id: entry_id.clone(),
                    until: "2026-09-01".to_string(),
                })
                .collect(),
            reply,
        })
        .unwrap();

        assert_eq!(result.applied.len(), 1);
        assert_eq!(result.applied[0].entry.entry_id, ids[1]);
        assert_eq!(result.skipped.len(), 1);
        assert_eq!(result.skipped[0].0, ids[0]);

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn an_empty_batch_is_answered_and_writes_nothing() {
        let vault = tempfile::tempdir().unwrap();
        let (sender, worker) = spawn_worker(&vault);

        let result = request(&sender, |reply| LedgerJob::MutateMany {
            ops: Vec::new(),
            reply,
        })
        .unwrap();

        assert!(result.applied.is_empty());
        assert!(result.skipped.is_empty());

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn the_triage_marker_is_seeded_once_and_then_only_moves_forward() {
        let vault = tempfile::tempdir().unwrap();
        let (sender, worker) = spawn_worker(&vault);

        assert_eq!(
            request(&sender, |reply| LedgerJob::TriageLastSeen { reply }).unwrap(),
            None,
            "an unseeded ledger has no marker"
        );

        sender.send(LedgerJob::SeedTriageMarker).unwrap();
        let seeded = request(&sender, |reply| LedgerJob::TriageLastSeen { reply })
            .unwrap()
            .expect("seeding sets it");

        // Seeding again must not move it, or every launch would declare the
        // day's enrollments already reviewed.
        sender.send(LedgerJob::SeedTriageMarker).unwrap();
        assert_eq!(
            request(&sender, |reply| LedgerJob::TriageLastSeen { reply }).unwrap(),
            Some(seeded.clone())
        );

        request(&sender, |reply| LedgerJob::MarkTriageSeen {
            seen_through: "2099-01-01T00:00:00Z".to_string(),
            reply,
        })
        .unwrap();
        assert_eq!(
            request(&sender, |reply| LedgerJob::TriageLastSeen { reply })
                .unwrap()
                .as_deref(),
            Some("2099-01-01T00:00:00Z")
        );

        drop(sender);
        worker.join().unwrap();
    }

    /// The marker is device-local viewing state, so recording a review must
    /// neither announce a change nor arm the snapshot debounce. Otherwise
    /// glancing at the Commitments view would rewrite `_ledger.yml`.
    #[test]
    fn marking_triage_seen_announces_nothing_and_writes_no_snapshot() {
        let vault = tempfile::tempdir().unwrap();
        let (sender, worker, announced) = spawn_counting_worker(&vault);
        seed_entry(&sender);
        flush_and_wait(&sender);
        let before = announced.load(Ordering::SeqCst);

        sender.send(LedgerJob::SeedTriageMarker).unwrap();
        request(&sender, |reply| LedgerJob::MarkTriageSeen {
            seen_through: "2026-08-20T00:00:00Z".to_string(),
            reply,
        })
        .unwrap();
        flush_and_wait(&sender);

        assert_eq!(
            announced.load(Ordering::SeqCst),
            before,
            "viewing state is not a commitment change"
        );

        drop(sender);
        worker.join().unwrap();
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
    fn a_sync_carrying_a_context_only_note_is_gated() {
        // What `set_meeting_tracking` relies on after the override moved into
        // frontmatter: the mode rides in on the facts, so the sync that follows
        // a flip reads the new value off the note rather than racing a write.
        let vault = tempfile::tempdir().unwrap();
        let (sender, worker) = spawn_worker(&vault);

        let mut facts = facts_for("n_a1b2c3", "Ops", "a_111111", "send the deck");
        facts.note_override = Some(EnrollmentMode::ContextOnly);
        sender.send(LedgerJob::Sync(Box::new(facts))).unwrap();

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
    fn a_sync_carrying_a_gating_category_is_gated_the_same_way() {
        // The category half of the chain travels on the facts exactly as the
        // override does, resolved by the shell before the job was queued: the
        // worker holds no `AppHandle` and reads no settings of its own.
        let vault = tempfile::tempdir().unwrap();
        let (sender, worker) = spawn_worker(&vault);

        let mut facts = facts_for("n_a1b2c3", "Ops", "a_111111", "send the deck");
        facts.category_default = Some(EnrollmentMode::ContextOnly);
        sender.send(LedgerJob::Sync(Box::new(facts))).unwrap();

        let details = request(&sender, |reply| LedgerJob::ListDetails {
            filter: EntryFilter::default(),
            reply,
        })
        .unwrap();
        assert!(
            details.is_empty(),
            "an all-hands never enrols someone else's commitment"
        );

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn a_retro_application_carries_the_source_that_asked_for_it() {
        let vault = tempfile::tempdir().unwrap();
        let (sender, worker) = spawn_worker(&vault);
        seed_entry(&sender);

        let outcome = request(&sender, |reply| LedgerJob::RetroApplyNoteTracking {
            note_id: "n_a1b2c3".to_string(),
            project: "Ops".to_string(),
            context_only: true,
            source: RetroSource::Category,
            reply,
        })
        .unwrap();
        assert_eq!(outcome.untracked.len(), 1);

        let details = request(&sender, |reply| LedgerJob::NoteEntries {
            note_id: "n_a1b2c3".to_string(),
            reply,
        })
        .unwrap();
        assert_eq!(
            details[0].entry.untracked_via,
            Some(UntrackedVia::Category),
            "a recategorization says so, so the row can explain itself"
        );

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn the_worker_answers_the_note_entries_read_and_retro_applies_a_flip() {
        let vault = tempfile::tempdir().unwrap();
        let (sender, worker) = spawn_worker(&vault);
        seed_entry(&sender);

        let details = request(&sender, |reply| LedgerJob::NoteEntries {
            note_id: "n_a1b2c3".to_string(),
            reply,
        })
        .unwrap();
        assert_eq!(details.len(), 1);

        // The note's frontmatter has just been rewritten to context-only; the
        // worker's job is the consequence, not the judgement.
        request(&sender, |reply| LedgerJob::RetroApplyNoteTracking {
            note_id: "n_a1b2c3".to_string(),
            project: "Ops".to_string(),
            context_only: true,
            source: RetroSource::Override,
            reply,
        })
        .unwrap();

        let details = request(&sender, |reply| LedgerJob::NoteEntries {
            note_id: "n_a1b2c3".to_string(),
            reply,
        })
        .unwrap();
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
        // A context-only meeting, so the gate keeps Priya's line out entirely.
        let mut facts = facts_for("n_a1b2c3", "Ops", "a_111111", "send the deck");
        facts.note_override = Some(EnrollmentMode::ContextOnly);
        sender.send(LedgerJob::Sync(Box::new(facts))).unwrap();

        let entry = request(&sender, |reply| LedgerJob::TrackItem {
            request: Box::new(TrackItemRequest {
                identity: OwnerIdentity::default(),
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
    fn claiming_a_commitment_records_it_as_a_persons_judgement() {
        let vault = tempfile::tempdir().unwrap();
        let (sender, worker) = spawn_worker(&vault);
        let entry_id = seed_entry(&sender);

        let reply = request(&sender, |reply| LedgerJob::Mutate {
            op: LedgerOp::ClaimMine {
                entry_id: entry_id.clone(),
            },
            reply,
        })
        .unwrap();

        assert_eq!(reply.entry.direction, Direction::Mine);
        assert_eq!(
            reply.entry.state,
            EntryState::Open,
            "a claim corrects who it belongs to, not where it stands"
        );
        assert!(
            reply.entry.touched,
            "the claim is a judgement, so no later sweep may overrule it"
        );

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn the_worker_re_files_what_a_new_name_resolves() {
        let vault = tempfile::tempdir().unwrap();
        let (sender, worker) = spawn_worker(&vault);
        // Synced before the user said who they were, so their own line landed
        // on the them side.
        let entry_id = seed_entry(&sender);

        let outcome = request(&sender, |reply| LedgerJob::RetroResolveOwners {
            identity: OwnerIdentity::new("Priya", &[]),
            reply,
        })
        .unwrap();

        assert_eq!(outcome.claimed, vec![entry_id.clone()]);

        let details = request(&sender, |reply| LedgerJob::ListDetails {
            filter: EntryFilter::default(),
            reply,
        })
        .unwrap();
        assert_eq!(details[0].entry.direction, Direction::Mine);
        assert!(
            !details[0].entry.touched,
            "a sweep is a default, not a person acting"
        );

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn a_sync_carrying_an_identity_enrols_the_users_own_line_under_context_only() {
        let vault = tempfile::tempdir().unwrap();
        let (sender, worker) = spawn_worker(&vault);
        // The gate would drop this line outright if "Priya" did not resolve to
        // the local user.
        let mut facts = facts_for("n_a1b2c3", "Ops", "a_111111", "send the deck");
        facts.note_override = Some(EnrollmentMode::ContextOnly);
        facts.identity = OwnerIdentity::new("Priya", &[]);
        sender.send(LedgerJob::Sync(Box::new(facts))).unwrap();

        let details = request(&sender, |reply| LedgerJob::ListDetails {
            filter: EntryFilter::default(),
            reply,
        })
        .unwrap();

        assert_eq!(details.len(), 1);
        assert_eq!(details[0].entry.direction, Direction::Mine);

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

    /// Asks the worker whether it still owes a startup backfill, the way the
    /// index worker's `LedgerHandle::take_startup_backfill` does.
    fn take_startup_backfill(sender: &Sender<LedgerJob>) -> bool {
        let (reply, answer) = mpsc::channel();
        sender
            .send(LedgerJob::TakeStartupBackfill { reply })
            .unwrap();
        answer.recv_timeout(Duration::from_secs(5)).unwrap()
    }

    #[test]
    fn an_empty_ledger_asks_for_a_startup_backfill_exactly_once() {
        let vault = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(vault.path().join("Ops")).unwrap();
        let (sender, worker) = spawn_worker(&vault);

        // No snapshots to restore from, so the ledger is still empty after the
        // restore — which is exactly the first-run-on-an-existing-vault case.
        sender.send(LedgerJob::RestoreIfEmpty).unwrap();

        assert!(take_startup_backfill(&sender), "the seed is owed");
        assert!(
            !take_startup_backfill(&sender),
            "claimed, so a second reconcile does not re-run it"
        );

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn a_restored_snapshot_suppresses_the_startup_backfill() {
        // The ledger was rebuilt from the vault's own snapshots, so it already
        // holds the user's commitments and seeding would be redundant.
        let vault = tempfile::tempdir().unwrap();
        let dir = vault.path().join("Ops");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("_ledger.yml"),
            "version: 1\nentries:\n\
             - entry_id: le_aaaaaaaaaaaa\n  \
               state: open\n  \
               direction: theirs\n  \
               owner: Priya\n  \
               description: an old promise\n  \
               created_at: 2026-07-01T00:00:00Z\n  \
               updated_at: 2026-07-01T00:00:00Z\n  \
               last_mention: 2026-07-01T00:00:00Z\n",
        )
        .unwrap();
        let (sender, worker) = spawn_worker(&vault);

        sender.send(LedgerJob::RestoreIfEmpty).unwrap();

        assert!(!take_startup_backfill(&sender));

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn machine_ingest_announces_only_when_it_writes() {
        let vault = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(vault.path().join("Ops")).unwrap();
        let (sender, worker, announced) = spawn_counting_worker(&vault);
        let facts = || {
            LedgerJob::Sync(Box::new(facts_for(
                "n_a1b2c3",
                "Ops",
                "a_111111",
                "send the deck",
            )))
        };

        sender.send(facts()).unwrap();
        flush_and_wait(&sender);
        assert_eq!(announced.load(Ordering::SeqCst), 1, "a new entry is news");

        // The identical note again: every item is already linked, so the pass
        // writes nothing and must stay silent, or an idle watcher burst would
        // have every Commitments view refetching on a loop.
        sender.send(facts()).unwrap();
        flush_and_wait(&sender);
        assert_eq!(announced.load(Ordering::SeqCst), 1);

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn a_refile_announces_even_though_no_item_changed() {
        // The end-to-end pin for `SyncOutcome::moved`: re-filing a note leaves
        // every item exactly as it was, so without the move being reported this
        // sync reads as a no-op and an open Commitments view keeps showing the
        // old project.
        let vault = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(vault.path().join("Ops")).unwrap();
        std::fs::create_dir_all(vault.path().join("Growth")).unwrap();
        let (sender, worker, announced) = spawn_counting_worker(&vault);

        sender
            .send(LedgerJob::Sync(Box::new(facts_for(
                "n_a1b2c3",
                "Ops",
                "a_111111",
                "send the deck",
            ))))
            .unwrap();
        flush_and_wait(&sender);
        let after_create = announced.load(Ordering::SeqCst);

        // The same note, same items, filed somewhere new.
        sender
            .send(LedgerJob::Sync(Box::new(facts_for(
                "n_a1b2c3",
                "Growth",
                "a_111111",
                "send the deck",
            ))))
            .unwrap();
        flush_and_wait(&sender);

        assert_eq!(
            announced.load(Ordering::SeqCst),
            after_create + 1,
            "the move is a write and has to be announced"
        );

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn a_note_leaving_the_vault_announces_its_retirement() {
        let vault = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(vault.path().join("Ops")).unwrap();
        let (sender, worker, announced) = spawn_counting_worker(&vault);

        sender
            .send(LedgerJob::Sync(Box::new(facts_for(
                "n_a1b2c3",
                "Ops",
                "a_111111",
                "send the deck",
            ))))
            .unwrap();
        flush_and_wait(&sender);
        let after_create = announced.load(Ordering::SeqCst);

        sender
            .send(LedgerJob::NoteGone("n_a1b2c3".to_string()))
            .unwrap();
        flush_and_wait(&sender);
        assert_eq!(announced.load(Ordering::SeqCst), after_create + 1);

        // A note the ledger never knew retires nothing, so it says nothing.
        sender
            .send(LedgerJob::NoteGone("n_zzzzzz".to_string()))
            .unwrap();
        flush_and_wait(&sender);
        assert_eq!(announced.load(Ordering::SeqCst), after_create + 1);

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
        let worker = std::thread::spawn(move || {
            run_worker(jobs, ledger, Some(root), QUIET, MAX_DELAY, || {})
        });
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
            &mut false,
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
                identity: OwnerIdentity::default(),
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
                note_override: None,
                category_default: None,
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
