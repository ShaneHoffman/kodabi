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
//! **Failure posture.** An unopenable `ledger.db` logs and degrades to a session
//! that records nothing, rather than blocking launch. That is defensible *for
//! this ticket* and no further: with no UI and no mutation surface yet, a
//! degraded session can only miss automatic ingestions, and those re-converge
//! from the notes at the next healthy startup reconcile. Nothing already
//! recorded is lost, because nothing is written.
//!
//! **This does not generalize.** The moment a surface exists that lets a person
//! waive, snooze, or close an entry, silently dropping that write would be a
//! lie: it must fail with copy instead. Whichever ticket adds that surface adds
//! an availability check here for it to branch on — deliberately not added
//! ahead of its caller, since an unused accessor is a promise nothing keeps.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{SecondsFormat, Utc};
use kodabi_core::ledger::{Ledger, NoteSync, LEDGER_DB_FILE};
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

/// One note's derived action items, as handed to the ledger worker.
pub struct NoteFacts {
    pub note_id: String,
    /// Project slug; the Inbox sentinel for an unfiled note.
    pub project: String,
    /// The note's `date_utc`, which becomes the entry's `last_mention`.
    pub date_utc: String,
    pub items: Vec<ActionItemFact>,
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
        LedgerHandle(self.sender.as_ref().map(|sender| {
            sender
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone()
        }))
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
                if pending_after > pending_before {
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
    }
    false
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
}
