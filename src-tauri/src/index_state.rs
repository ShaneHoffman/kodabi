//! Managed state for the SQLite note index and its embedder.
//!
//! The index is a **rebuildable cache** derived from the Markdown vault, so
//! nothing here is load-bearing for correctness: if the database can't be
//! opened the app still runs (notes just aren't searchable this session), and
//! indexing a note is best-effort — a failure is logged, never surfaced to the
//! command, because the `.md` file on disk is the source of truth.
//!
//! Indexing runs on a **dedicated background thread**, not on the note-write
//! command: [`index_note_best_effort`](IndexState::index_note_best_effort) only
//! hands the note to that worker over a channel and returns, so a save never
//! waits on (feature-gated) embedding, and the slow embed runs *off* the index
//! lock — a single worker serializes writes, so releasing the lock across an
//! embed can't lose a race. The embedder is present only in `embed`-feature
//! builds; the default build carries `None`, so the worker populates the note
//! row and full-text index but writes no vectors.
//!
//! The worker also serves whole-vault [`reconcile`](kodabi_core::reconcile)
//! jobs: a file watcher and a startup scan enqueue a reconcile whenever the
//! vault changes on disk, syncing note rows by their stable `id`, and the
//! rebuild command clears and repopulates the index from files alone. After a
//! reconcile the worker embeds any note still missing its vectors, so external
//! edits and changes made while the app was closed both converge without a
//! restart.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};

use kodabi_core::embed::{self, Embedder};
use kodabi_core::index::{IndexedNote, NoteIndex, SearchOptions, SearchParams, SearchResults};
use kodabi_core::ledger::NoteContext;
use kodabi_core::ledger::OwnerIdentity;
use kodabi_core::reconcile;
use kodabi_core::settings::CategorySettings;
use kodabi_core::watch::{self, VaultWatcher};

use chrono::Utc;
use tauri::{AppHandle, Emitter, Manager};

use crate::events::{INDEX_STATE_EVENT, VAULT_CHANGED_EVENT};
use crate::ledger_state::{self, LedgerHandle, NoteFacts};
use crate::transcribe::knowledge_base_dir;

/// A unit of work for the index worker.
enum Job {
    /// Upsert and re-embed a single note after an in-app write. Boxed because it
    /// dwarfs the other variants, which carry no data.
    Note(Box<IndexedNote>),
    /// Drop a single note's rows after it was deleted from disk — the delete
    /// analogue of `Note`. The `.md` file (the source of truth) is already gone,
    /// so this only converges the derived index; reconcile's stale sweep is the
    /// safety net if the job is ever dropped.
    DeleteNote(String),
    /// Sync the whole index to the vault on disk — a watcher burst or the
    /// startup scan.
    Reconcile,
    /// Drop and repopulate the index from every file ("files are truth").
    Rebuild,
}

/// Progress of a full rebuild, emitted on the `index:state` event. Mirrors the
/// `transcription:state` tagged-status shape the frontend already consumes.
#[derive(Clone, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum IndexStateEvent {
    Rebuilding,
    Ready { notes: usize },
    Error { message: String },
}

/// A handle to the background index worker, held as Tauri managed state.
pub struct IndexState {
    /// Sends jobs to the worker thread. `None` when the database failed to open
    /// (logged at startup). Wrapped in a `Mutex` so the shared state stays
    /// `Sync` — the lock is held only for a non-blocking channel send.
    sender: Option<Mutex<Sender<Job>>>,
    /// The same index the worker writes, kept here so a *read* can reach it
    /// without a round trip through the job channel. Every write still goes
    /// through the worker: sharing the handle adds a reader, not a second
    /// writer.
    index: Option<Arc<Mutex<NoteIndex>>>,
    /// The embedder the worker indexes with, so a query is embedded by the same
    /// model as the passages it is scored against. `None` in the default build.
    embedder: Option<Arc<dyn Embedder>>,
    /// Keeps the OS file watcher alive for the app's lifetime; dropping it stops
    /// watching. `None` when the index or the vault path is unavailable. The
    /// `Mutex` only makes the state `Sync`; it is never locked after construction.
    _watcher: Option<Mutex<VaultWatcher>>,
}

/// Forwards a note's freshly derived action items to the commitment ledger.
///
/// Every path that (re)derives facts funnels through this worker — an in-app
/// write enqueues `Job::Note` with the facts already attached, and a reconcile
/// or rebuild derives them inside `kodabi_core::reconcile` — so hooking the
/// worker covers all of them and the note commands need no ledger awareness.
///
/// Reconcile's unchanged-note fast path is deliberately not a gap: facts are a
/// pure function of the note's body, and item ids are content hashes, so a note
/// whose body did not change re-derives to exactly what the ledger already has.
fn forward_to_ledger(ledger: &LedgerHandle, note: &IndexedNote, settings: &EnrolmentSettings) {
    let Some(facts) = &note.meeting else {
        return; // a type that carries no commitments (`meeting::derives_facts`)
    };
    let Ok(date_utc) = kodabi_core::index::normalize_date_to_utc(&note.date) else {
        return; // an unparseable date has no place on the aging clock
    };
    ledger.sync(NoteFacts {
        note_id: note.id.clone(),
        project: ledger_state::project_slug(note.project.as_deref()),
        date_utc,
        items: facts.action_items.clone(),
        // The row being written *is* the note's frontmatter, so the gate reads
        // the same value the file carries.
        note_override: note
            .tracking
            .as_deref()
            .and_then(|raw| kodabi_core::ledger::EnrollmentMode::parse_frontmatter(raw).ok()),
        // The other half of the chain, resolved here because the ledger worker
        // cannot reach the settings. A note with no category resolves to `None`
        // and falls through to the global default.
        category_default: kodabi_core::ledger::category_default_for(
            note.category,
            &settings.categories,
        ),
        identity: settings.identity.clone(),
    });
}

/// Forwards every note a reconcile pass touched, reading each one's items back
/// out of the index it just wrote them to.
///
/// The read-back keeps `reconcile` a pure converger that knows nothing about the
/// ledger. It is cheap (rows the pass just wrote, under a lock it already
/// holds), and `sync_note_items` is idempotent, so a redundant forward costs a
/// transaction and changes nothing.
fn forward_reconciled(
    index: &Mutex<NoteIndex>,
    ledger: &LedgerHandle,
    ids: &[String],
    settings: &EnrolmentSettings,
) {
    if ids.is_empty() {
        return;
    }
    let idx = lock(index);
    let batch: Vec<NoteFacts> = ids
        .iter()
        .filter_map(|id| note_facts_from(&idx, id, settings))
        .collect();
    drop(idx);
    ledger.sync_batch(batch);
}

/// Forwards a *backfill* pass's notes, which is the same thing with one filter:
/// a hand-written note older than the enrolment window does not enrol.
///
/// The window exists because the two backfill legs are the only ones that hand
/// the ledger notes nobody just touched. A note the reconcile upserted changed
/// on disk, so it is a fresh mention whatever its date, and
/// [`forward_reconciled`] stays unwindowed for exactly that reason. A backfill
/// leg, by contrast, sweeps whatever the vault happens to hold — and when
/// `derives_facts` widened to plain notes, that became every checkbox the user
/// ever wrote. Enrolling a to-do from eight months ago is honest and useless: it
/// arrives already past the stale threshold, sorting above everything current.
///
/// Meetings and chats are deliberately *not* windowed: they have been forwarded
/// unconditionally since the v3 migration shipped, and narrowing that now would
/// silently drop commitments from field databases mid-backfill.
fn forward_backfilled(
    index: &Mutex<NoteIndex>,
    ledger: &LedgerHandle,
    ids: &[String],
    settings: &EnrolmentSettings,
) {
    if ids.is_empty() {
        return;
    }
    let cutoff = kodabi_core::ledger::mention_window_cutoff(Utc::now(), settings.stale_after_days);
    let idx = lock(index);
    let batch: Vec<NoteFacts> = ids
        .iter()
        .filter(|id| {
            // A row we cannot read is left to `note_facts_from` to drop, so it
            // is not excluded here.
            match idx.get_note(id) {
                Ok(Some(row)) => backfill_enrols(row.note_type, &row.date_utc, &cutoff),
                _ => true,
            }
        })
        .filter_map(|id| note_facts_from(&idx, id, settings))
        .collect();
    drop(idx);
    ledger.sync_batch(batch);
}

/// The enrolment window's one rule, split out so it can be read and tested on
/// its own: a hand-written note enrols only from inside the window; a meeting or
/// a chat always does.
fn backfill_enrols(
    note_type: kodabi_core::index::NoteType,
    date_utc: &str,
    cutoff_utc: &str,
) -> bool {
    note_type != kodabi_core::index::NoteType::Note || date_utc >= cutoff_utc
}

/// The user settings a note has to be read against before it reaches the
/// ledger: the enrollment chain's category slot, and the names that decide
/// which of its owners mean the local user.
///
/// One struct from one snapshot rather than two reads, so the pair can never
/// come from either side of a settings write.
pub(crate) struct EnrolmentSettings {
    categories: CategorySettings,
    identity: OwnerIdentity,
    /// The aging config's stale threshold, doubling as the enrolment window for
    /// the backfill legs (`forward_backfilled`, `seed_ledger_from_index`).
    stale_after_days: u32,
}

impl EnrolmentSettings {
    /// The user's own names, for a caller resolving directions at read time
    /// rather than through [`NoteFacts`].
    pub(crate) fn identity(&self) -> &OwnerIdentity {
        &self.identity
    }
}

/// Reads both halves in one go.
///
/// Read per job rather than cached, for the same reason `ledger_cmds`'
/// `aging_config` is: the Settings view can change a category's default, or the
/// user's own name, while the worker is running, and the next note through the
/// door should be read against the new value.
pub(crate) fn enrolment_settings(app: &AppHandle) -> EnrolmentSettings {
    let settings = app
        .state::<crate::settings_cmds::SettingsState>()
        .snapshot();
    EnrolmentSettings {
        categories: settings.categories,
        identity: settings.identity.owner_identity(),
        stale_after_days: settings.ledger.stale_after_days.get(),
    }
}

/// Reads one note's ledger-shaped facts back out of the index.
///
/// Shared by [`forward_reconciled`] and [`IndexReadHandle::note_facts`], so a
/// command that re-syncs one note builds exactly what the reconcile pass would
/// have. Returns `None` for a note the index does not know, one whose type
/// carries no commitments, or one whose rows cannot be read.
fn note_facts_from(idx: &NoteIndex, id: &str, settings: &EnrolmentSettings) -> Option<NoteFacts> {
    let row = idx.get_note(id).ok()??;
    if !kodabi_core::meeting::derives_facts(row.note_type.into()) {
        return None;
    }
    let items = idx.get_action_items(id).ok()?;
    // The gate's input, read from the column the note's frontmatter fills. A
    // value the index somehow holds but the enum does not know is treated as no
    // override rather than failing the sync: the note file is the truth, and
    // the next reconcile rewrites the row from it.
    let note_override = row
        .tracking
        .as_deref()
        .and_then(|raw| kodabi_core::ledger::EnrollmentMode::parse_frontmatter(raw).ok());
    let category_default =
        kodabi_core::ledger::category_default_for(row.category, &settings.categories);
    Some(NoteFacts {
        note_id: row.id,
        project: ledger_state::project_slug(row.project.as_deref()),
        date_utc: row.date_utc,
        note_override,
        category_default,
        identity: settings.identity.clone(),
        items: items
            .into_iter()
            .map(|item| kodabi_core::meeting::ActionItemFact {
                id: item.id,
                description: item.description,
                owner: item.owner,
                due_date: item.due_date,
                done: item.done,
                extracted_date: item.extracted_date,
            })
            .collect(),
    })
}

/// A cloneable read handle onto the index, for commands that query it.
///
/// It exists because managed state cannot cross onto a blocking thread: a
/// command clones this, moves it into `spawn_blocking`, and reads there. The
/// index worker holds the lock across a whole-vault scan, so a search really can
/// wait on it — which is exactly why that wait must not sit on the IPC thread.
#[derive(Clone)]
pub struct SearchHandle {
    index: Arc<Mutex<NoteIndex>>,
    embedder: Option<Arc<dyn Embedder>>,
}

impl SearchHandle {
    /// Runs one search under the index lock. Blocking; call it off the IPC
    /// thread.
    pub fn search(
        &self,
        params: &SearchParams,
        options: SearchOptions,
    ) -> kodabi_core::index::Result<SearchResults> {
        lock(&self.index).search_notes_with(params, self.embedder.as_deref(), options)
    }
}

/// A cloneable read handle for the commitments join.
///
/// Separate from [`SearchHandle`] because it answers a different question with
/// no embedder in it: given the note ids a set of ledger entries points at, what
/// does the index currently say those notes' action items are. Same bargain as
/// its sibling, for the same reason: managed state cannot cross onto a blocking
/// thread, so a command clones this and reads there.
#[derive(Clone)]
pub struct IndexReadHandle {
    index: Arc<Mutex<NoteIndex>>,
}

impl IndexReadHandle {
    /// Reads the note context for each id, under one lock.
    ///
    /// Best-effort per note: a missing row or a read error omits that id rather
    /// than failing the batch, because a commitment whose note the index has
    /// lost still renders from the ledger's cached text
    /// ([`kodabi_core::ledger::view`]). Blocking; call it off the IPC thread.
    pub fn note_contexts(&self, note_ids: &BTreeSet<String>) -> HashMap<String, NoteContext> {
        if note_ids.is_empty() {
            return HashMap::new();
        }
        let idx = lock(&self.index);
        let mut contexts = HashMap::with_capacity(note_ids.len());
        for id in note_ids {
            let Ok(Some(row)) = idx.get_note(id) else {
                continue;
            };
            let Ok(items) = idx.get_action_items(id) else {
                continue;
            };
            contexts.insert(
                id.clone(),
                NoteContext {
                    note_id: row.id,
                    title: row.title,
                    project: row.project,
                    path: row.path,
                    category: row.category,
                    items,
                },
            );
        }
        contexts
    }

    /// One note's ledger-shaped facts, or `None` when the index cannot supply
    /// them (unknown note, a type that carries no commitments, a read error).
    ///
    /// The caller for this is a command that changed a note's enrollment and
    /// has to re-sync it. [`NoteContext`] cannot stand in: it carries no
    /// `date_utc`, and that is what becomes an entry's `last_mention`.
    /// Blocking; call it off the IPC thread.
    ///
    /// `settings` is the caller's snapshot: resolving the category half of the
    /// enrollment chain and the user's own names both need it, and this handle
    /// deliberately holds no `AppHandle`.
    pub fn note_facts(&self, note_id: &str, settings: &EnrolmentSettings) -> Option<NoteFacts> {
        let idx = lock(&self.index);
        note_facts_from(&idx, note_id, settings)
    }
}

impl IndexState {
    /// Opens the index, builds the embedder, and spawns the worker. Never fails:
    /// an unopenable index degrades to "notes aren't indexed", not a launch
    /// failure.
    pub fn initialize(app: &AppHandle, ledger: LedgerHandle) -> Self {
        let Some(index) = open_index(app) else {
            eprintln!("note index unavailable — notes will not be searchable this session");
            return Self {
                sender: None,
                index: None,
                embedder: None,
                _watcher: None,
            };
        };
        let index = Arc::new(index);
        let embedder = build_embedder(app);

        // The vault root the watcher observes and reconcile scans. Best-effort:
        // if it can't be resolved, the worker still serves in-app writes; only
        // the live-sync and rebuild have nothing to converge against.
        let vault_root = knowledge_base_dir(app).ok();

        let (sender, jobs) = mpsc::channel::<Job>();
        let app_handle = app.clone();
        let worker_root = vault_root.clone();
        let worker_index = Arc::clone(&index);
        let worker_embedder = embedder.clone();
        std::thread::spawn(move || {
            run_worker(
                app_handle,
                worker_index,
                worker_embedder,
                worker_root,
                jobs,
                ledger,
            )
        });

        // A full reconciliation scan at startup converges files added or edited
        // while the app was closed, before any live watcher event arrives.
        let _ = sender.send(Job::Reconcile);

        // Watch the vault so on-disk edits (from the app, an editor, or git)
        // reconcile without a restart; a relevant change enqueues one reconcile.
        let watcher = vault_root.and_then(|root| {
            let watch_sender = sender.clone();
            match watch::watch_vault(&root, move || {
                let _ = watch_sender.send(Job::Reconcile);
            }) {
                Ok(watcher) => Some(Mutex::new(watcher)),
                Err(err) => {
                    eprintln!("failed to start the vault watcher: {err}");
                    None
                }
            }
        });

        Self {
            sender: Some(Mutex::new(sender)),
            index: Some(index),
            embedder,
            _watcher: watcher,
        }
    }

    /// A handle for querying the index, or `None` when the index is unavailable
    /// this session (the same condition that makes every write a no-op).
    pub fn search_handle(&self) -> Option<SearchHandle> {
        Some(SearchHandle {
            index: Arc::clone(self.index.as_ref()?),
            embedder: self.embedder.clone(),
        })
    }

    /// A read handle for the commitments join, when the index opened.
    pub fn read_handle(&self) -> Option<IndexReadHandle> {
        Some(IndexReadHandle {
            index: Arc::clone(self.index.as_ref()?),
        })
    }

    /// Title and project for one note id, for the chat's citation chips.
    ///
    /// A single indexed row read straight off the shared handle — no round trip
    /// through the job channel, the same reader-not-second-writer bargain
    /// [`search_handle`](Self::search_handle) makes. `None` whenever the answer
    /// can't be had (no index this session, an unknown id, a read error): the
    /// chip simply doesn't render and the tool line still does, so a citation is
    /// never load-bearing.
    ///
    /// Blocking on the index lock, which a whole-vault scan can hold — call it
    /// off the IPC thread. The chat pump thread is such a caller.
    pub fn note_title_project(&self, id: &str) -> Option<(String, Option<String>)> {
        let row = lock(self.index.as_ref()?).get_note(id).ok().flatten()?;
        Some((row.title, row.project))
    }

    /// Hands `note` to the background worker to be upserted and (re-)embedded,
    /// returning immediately. Called after a note is written or edited on disk;
    /// the disk write is what mattered, so indexing runs off the command path
    /// and can never fail or delay it.
    pub fn index_note_best_effort(&self, note: IndexedNote) {
        self.send(Job::Note(Box::new(note)));
    }

    /// Hands a deleted note's id to the worker so its rows (note, full-text,
    /// tags, and vectors) leave the index promptly — the delete counterpart to
    /// [`index_note_best_effort`]. Called after the `.md` file was removed; the
    /// removal is what mattered, so like every index write it never fails or
    /// delays the command, and reconcile would eventually sweep the row anyway.
    pub fn delete_note_best_effort(&self, id: &str) {
        self.send(Job::DeleteNote(id.to_string()));
    }

    /// Requests a full rebuild of the index from files alone, returning whether
    /// the worker accepted it (false when the index is unavailable this session).
    /// Progress arrives on the `index:state` event.
    pub fn request_rebuild(&self) -> bool {
        self.send(Job::Rebuild)
    }

    /// Requests a whole-vault reconcile pass (upsert by id plus the stale
    /// sweep), returning whether the worker accepted it. Used after a bulk
    /// mutation (a project deletion) so the index converges promptly instead
    /// of waiting on the file watcher's debounce.
    pub fn request_reconcile(&self) -> bool {
        self.send(Job::Reconcile)
    }

    /// Hands a job to the worker, returning whether it was queued. A closed
    /// channel (worker gone at shutdown) or an unopened index simply drops it —
    /// fine for a best-effort cache.
    fn send(&self, job: Job) -> bool {
        let Some(sender) = &self.sender else {
            return false;
        };
        let sender = sender.lock().unwrap_or_else(|poison| poison.into_inner());
        sender.send(job).is_ok()
    }
}

/// The worker loop: serve note upserts, whole-vault reconciles, and rebuilds in
/// arrival order.
///
/// A single worker means phases of [`process_note`] (and the reconcile embed
/// sweep) never interleave with another writer, so the lock can be released
/// across the slow embed without racing a concurrent write.
fn run_worker(
    app: AppHandle,
    index: Arc<Mutex<NoteIndex>>,
    embedder: Option<Arc<dyn Embedder>>,
    vault_root: Option<PathBuf>,
    jobs: Receiver<Job>,
    ledger: LedgerHandle,
) {
    for job in jobs {
        // Read per job, not once per worker: a category's enrollment default is
        // a live setting, so the next note synced after a change is gated by the
        // new value without restarting anything.
        let enrolment = enrolment_settings(&app);
        match job {
            Job::Note(note) => {
                process_note(&index, embedder.as_deref(), &note);
                forward_to_ledger(&ledger, &note, &enrolment);
            }
            Job::DeleteNote(id) => {
                process_delete(&index, &id);
                ledger.note_gone(&id);
            }
            Job::Reconcile => run_reconcile(
                &app,
                &index,
                embedder.as_deref(),
                vault_root.as_deref(),
                &ledger,
            ),
            Job::Rebuild => run_rebuild(
                &app,
                &index,
                embedder.as_deref(),
                vault_root.as_deref(),
                &ledger,
            ),
        }
    }
}

/// Removes a deleted note's rows from the index, embeddings included (the
/// transactional `delete_note`, `crates/kodabi-core/src/index/query.rs`).
/// Best-effort like every write here: a failure just leaves a stale row the
/// next reconcile sweep drops, since the `.md` file is already gone.
fn process_delete(index: &Mutex<NoteIndex>, id: &str) {
    let mut idx = lock(index);
    if let Err(err) = idx.delete_note(id) {
        eprintln!("failed to remove note {id} from the index: {err}");
    }
}

/// Syncs the whole index to the vault, then embeds any note left without
/// vectors. Rows first (the index lock is held only for the scan), embeddings
/// second (off the lock, per the three-phase discipline in [`process_note`]).
/// Best-effort: a failure is logged and the next burst converges. Redundant
/// reconciles are cheap — the pass skips unchanged notes without touching a row.
fn run_reconcile(
    app: &AppHandle,
    index: &Mutex<NoteIndex>,
    embedder: Option<&dyn Embedder>,
    vault_root: Option<&Path>,
    ledger: &LedgerHandle,
) {
    let Some(root) = vault_root else {
        return; // no vault path resolved — nothing to reconcile against.
    };
    let report = {
        let mut idx = lock(index);
        reconcile::reconcile(root, &mut idx)
    };
    match report {
        Ok(report) => {
            if report.upserted + report.deleted > 0 {
                eprintln!(
                    "index reconcile: {} upserted, {} unchanged, {} deleted",
                    report.upserted, report.unchanged, report.deleted
                );
            }
            // Every note whose facts were re-derived, plus every note that left
            // the vault, reaches the ledger before the embed sweep — the
            // commitments matter more than the vectors, and the sweep is slow.
            forward_reconciled(
                index,
                ledger,
                &report.upserted_ids,
                &enrolment_settings(app),
            );
            for id in &report.deleted_ids {
                ledger.note_gone(id);
            }
            // Meeting notes skipped by the fast path (unchanged on disk) still
            // need their facts derived after the v3 migration — backfill them.
            let backfilled = backfill_meeting_facts(index, root);
            forward_backfilled(index, ledger, &backfilled, &enrolment_settings(app));
            // A ledger that came up empty gets seeded from the index, which by
            // now holds facts for every note in the vault. Asked after the two
            // forwards above so those facts exist to be read.
            if ledger.take_startup_backfill() {
                seed_ledger_from_index(index, ledger, &enrolment_settings(app));
            }
            if let Some(embedder) = embedder {
                reconcile_missing(index, embedder);
            }
            // Refresh every window's disk-backed lists now that the index (and
            // the files it mirrors) settled.
            let _ = app.emit(VAULT_CHANGED_EVENT, ());
        }
        Err(err) => eprintln!("index reconcile failed: {err}"),
    }
}

/// Drops and repopulates the index from files alone, announcing progress on the
/// `index:state` event. Unlike a reconcile, a rebuild always signals start and
/// completion (the UI shows status) and always emits `vault:changed` on success.
fn run_rebuild(
    app: &AppHandle,
    index: &Mutex<NoteIndex>,
    embedder: Option<&dyn Embedder>,
    vault_root: Option<&Path>,
    ledger: &LedgerHandle,
) {
    let _ = app.emit(INDEX_STATE_EVENT, IndexStateEvent::Rebuilding);

    let Some(root) = vault_root else {
        let _ = app.emit(
            INDEX_STATE_EVENT,
            IndexStateEvent::Error {
                message: "Kodabi couldn't find its data folder, so the index wasn't rebuilt. \
                          Restart the app; your notes on disk are untouched."
                    .to_string(),
            },
        );
        return;
    };

    let report = {
        let mut idx = lock(index);
        reconcile::rebuild(root, &mut idx)
    };
    match report {
        Ok(report) => {
            forward_reconciled(
                index,
                ledger,
                &report.upserted_ids,
                &enrolment_settings(app),
            );
            for id in &report.deleted_ids {
                ledger.note_gone(id);
            }
            let backfilled = backfill_meeting_facts(index, root);
            forward_backfilled(index, ledger, &backfilled, &enrolment_settings(app));
            if let Some(embedder) = embedder {
                reconcile_missing(index, embedder);
            }
            let _ = app.emit(
                INDEX_STATE_EVENT,
                IndexStateEvent::Ready {
                    notes: report.upserted + report.unchanged,
                },
            );
            let _ = app.emit(VAULT_CHANGED_EVENT, ());
        }
        Err(err) => {
            // This message renders in Settings, so it is copy: the rusqlite
            // detail behind it goes to the log instead. A failed rebuild leaves
            // the previous index in place, and the notes were never at risk.
            let _ = app.emit(
                INDEX_STATE_EVENT,
                IndexStateEvent::Error {
                    message: crate::user_errors::reported(
                        "rebuild_index",
                        err,
                        "Couldn't rebuild the index. Search may be stale but your notes are \
                         unaffected; try again from here.",
                    ),
                },
            );
        }
    }
}

/// Derives meeting facts for any meeting or chat note the index has not
/// backfilled yet (`meeting::derives_facts`). Independent of embeddings (needs no
/// embedder): the v3 migration adds the meeting tables empty and a reconcile
/// fast-path skips unchanged notes, so a field database's existing notes need one
/// derive pass. Best-effort — a failure is logged and the next sweep retries.
/// Fast (a meeting is one small JSONL read; a chat is body-parse only), so it
/// runs under the index lock like `reconcile` itself.
/// Returns the ids it filled, so the caller can forward them to the ledger.
fn backfill_meeting_facts(index: &Mutex<NoteIndex>, vault_root: &Path) -> Vec<String> {
    let mut idx = lock(index);
    match reconcile::reconcile_missing_meeting_facts(vault_root, &mut idx) {
        Ok(ids) => {
            if !ids.is_empty() {
                eprintln!("backfilled meeting facts for {} note(s)", ids.len());
            }
            ids
        }
        Err(err) => {
            eprintln!("meeting-facts backfill failed: {err}");
            Vec::new()
        }
    }
}

/// Enrols the commitments an existing vault already holds, into a ledger that
/// came up empty.
///
/// **Why anything is needed at all.** Every other route into the ledger is
/// triggered by a note *changing*. On the first launch after the ledger shipped
/// — or after `ledger.db` is lost, with the snapshots gone too — nothing has
/// changed: the reconcile's fast path skips every unchanged note, so
/// `upserted_ids` is empty, and the meeting-facts backfill finds every row
/// already present. The vault is full of open commitments and the ledger stays
/// empty forever, because "forever" is however long it takes each note to be
/// edited again.
///
/// **Why it is windowed.** Only notes inside the enrolment window
/// (`mention_window_cutoff`, the stale threshold) are enrolled. A vault holds
/// months of notes, and their old open items are precisely the ones that would
/// arrive already stale and sort above everything the user is actually working
/// on — day one would be a wall of other people's forgotten business. The
/// window is the difference between a ledger that is useful on day one and one
/// that gets ignored on day one.
///
/// Open items only, by the query. `last_mention` comes from each note's own
/// date through the ordinary facts path, so the aging tiers place a backfilled
/// commitment exactly where its note sits in time rather than treating it as
/// minted today.
///
/// Best-effort and idempotent: `sync_note_items` converges, so a redundant seed
/// costs transactions and changes nothing.
fn seed_ledger_from_index(
    index: &Mutex<NoteIndex>,
    ledger: &LedgerHandle,
    settings: &EnrolmentSettings,
) {
    let cutoff = kodabi_core::ledger::mention_window_cutoff(Utc::now(), settings.stale_after_days);
    let idx = lock(index);
    let ids = match idx.note_ids_with_open_items_since(&cutoff) {
        Ok(ids) => ids,
        Err(err) => {
            drop(idx);
            eprintln!("ledger backfill: could not list recent open items: {err}");
            return;
        }
    };
    let batch: Vec<NoteFacts> = ids
        .iter()
        .filter_map(|id| note_facts_from(&idx, id, settings))
        .collect();
    drop(idx);

    if batch.is_empty() {
        return;
    }
    eprintln!(
        "ledger backfill: enrolling commitments from {} note(s) since {cutoff}",
        batch.len()
    );
    ledger.sync_batch(batch);
}

/// Upserts `note` and refreshes its embeddings, embedding *off* the index lock.
fn process_note(index: &Mutex<NoteIndex>, embedder: Option<&dyn Embedder>, note: &IndexedNote) {
    // Phase 1 (lock): upsert, and decide whether an embed is even needed.
    let pending = {
        let mut idx = lock(index);
        match embedder {
            None => {
                if let Err(err) = idx.upsert_note(note) {
                    eprintln!("failed to index note {}: {err}", note.id);
                }
                return;
            }
            Some(_) => match embed::upsert_and_plan(&mut idx, note) {
                Ok(pending) => pending,
                Err(err) => {
                    eprintln!("failed to index note {}: {err}", note.id);
                    return;
                }
            },
        }
    };
    let Some(pending) = pending else {
        return; // vectors already current, or an empty body
    };
    let embedder = embedder.expect("embedder present in the matched Some arm");

    // Phase 2 (no lock): the slow embed runs without blocking index readers.
    let embeddings = match embedder.embed_passages(&pending.inputs) {
        Ok(embeddings) => embeddings,
        Err(err) => {
            eprintln!("failed to embed note {}: {err}", note.id);
            return;
        }
    };

    // Phase 3 (lock): store the finished vectors.
    let mut idx = lock(index);
    if let Err(err) = embed::store_embeddings(&mut idx, &note.id, &pending.chunks, embeddings) {
        eprintln!("failed to store embeddings for note {}: {err}", note.id);
    }
}

/// Startup reconciliation: embed every indexed note that has no vectors yet,
/// reading its content straight from the index (no disk access). Idempotent —
/// an empty-body note legitimately has no chunks and is skipped every time.
fn reconcile_missing(index: &Mutex<NoteIndex>, embedder: &dyn Embedder) {
    let ids = {
        let idx = lock(index);
        match idx.note_ids_missing_embeddings() {
            Ok(ids) => ids,
            Err(err) => {
                eprintln!("failed to list notes missing embeddings: {err}");
                return;
            }
        }
    };
    if ids.is_empty() {
        return;
    }
    eprintln!("embedding {} note(s) missing vectors at startup", ids.len());

    for id in ids {
        let row = {
            let idx = lock(index);
            idx.get_note(&id)
        };
        let row = match row {
            Ok(Some(row)) => row,
            Ok(None) => continue, // removed between listing and read
            Err(err) => {
                eprintln!("failed to read note {id} for embedding: {err}");
                continue;
            }
        };
        let Some(pending) = embed::plan_from_content(&row.title, &row.body) else {
            continue; // empty body — legitimately has no chunks
        };
        let embeddings = match embedder.embed_passages(&pending.inputs) {
            Ok(embeddings) => embeddings,
            Err(err) => {
                // A backend failure (e.g. the model dir is unset) would hit
                // every remaining note — stop the sweep instead of logging once
                // per note. Write/edit jobs still run; each just re-logs.
                eprintln!("stopping startup embedding sweep: {err}");
                return;
            }
        };
        let mut idx = lock(index);
        if let Err(err) = embed::store_embeddings(&mut idx, &id, &pending.chunks, embeddings) {
            eprintln!("failed to store embeddings for note {id}: {err}");
        }
    }
}

/// Locks the index, recovering a poisoned lock: the index is a rebuildable
/// cache, and a prior panic mid-write leaves at worst a stale row the next
/// upsert overwrites.
fn lock(index: &Mutex<NoteIndex>) -> MutexGuard<'_, NoteIndex> {
    index.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Environment override for the index database, as a full path to the file.
///
/// Same name and same semantics as `kodabi-mcp`'s own `KODABI_INDEX_DB`
/// (`crates/kodabi-mcp/src/config.rs`), which the generated `.mcp.json` already
/// writes alongside `KODABI_KB_ROOT`.
///
/// It is the mandatory companion to `KODABI_KB_ROOT`, not an independent knob:
/// `IndexState::initialize` hands the KB root to the watcher and to the startup
/// `Job::Reconcile`, so redirecting the vault while the index stayed under the
/// real app-data dir would converge that index against the foreign vault and
/// delete every row for the notes it can no longer see.
///
/// `KODABI_SANDBOX` exists so that pairing cannot be got wrong: it derives both
/// paths from one base and writes them here before this resolver runs. Setting
/// this variable alongside it is refused at startup — see `crate::sandbox`.
const INDEX_DB_ENV: &str = "KODABI_INDEX_DB";

/// Resolves the index database path — the one place that decides it.
///
/// Unset, the index lives at `app_data_dir()/index.db` — derived straight from
/// `app_data_dir()` rather than the KB root, because it is a machine-local cache
/// that must stay put even when a future vault-path setting relocates the notes.
/// This is a pinned invariant: the index is "derived, never synced", so it must
/// never sit inside the (syncable) KB folder. By default the two directories
/// coincide, which is exactly why the watcher filters `index.db*` out of its
/// events; `KODABI_INDEX_DB` is how they are pulled apart, which is the case
/// the invariant above exists to allow.
///
/// Shared with `terminal_cmds::write_mcp_config`, which hands both this path and
/// the KB root to the `kodabi-mcp` sidecar. That pairing is the whole point of
/// having one resolver: a `.mcp.json` naming the overridden vault next to the
/// app-data index would point the sidecar's `search_notes` at one vault and its
/// `list_projects` at another.
pub(crate) fn index_db_path(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(value) = std::env::var_os(INDEX_DB_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(value));
    }
    // Not internal-only: `terminal_cmds::write_mcp_config` and `chat_cmds`
    // both resolve this while starting a session, so this string can reach the
    // terminal pane and the chat view.
    let dir = app.path().app_data_dir().map_err(|err| {
        crate::user_errors::reported(
            "index_db_path",
            err,
            "Kodabi couldn't find its data folder. Restart the app; your notes on disk are \
             untouched.",
        )
    })?;
    Ok(dir.join("index.db"))
}

/// Opens (creating if absent) the index database at [`index_db_path`].
fn open_index(app: &AppHandle) -> Option<Mutex<NoteIndex>> {
    let path = match index_db_path(app) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("{err}");
            return None;
        }
    };
    // First launch has no app-data dir yet — and an overridden path may name a
    // directory that does not exist either.
    if let Some(parent) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            eprintln!("failed to create the note index directory: {err}");
            return None;
        }
    }
    match NoteIndex::open(&path) {
        Ok(index) => Some(Mutex::new(index)),
        Err(err) => {
            eprintln!("failed to open the note index at {}: {err}", path.display());
            None
        }
    }
}

#[cfg(feature = "embed")]
fn build_embedder(app: &AppHandle) -> Option<Arc<dyn Embedder>> {
    use kodabi_embed::{BgeConfig, BgeEmbedder};
    let mut config = BgeConfig::from_env();
    if config.model_dir.as_os_str().is_empty() {
        // `from_env` leaves the directory empty when `KODABI_EMBED_MODEL_DIR` is
        // unset, which is the normal case for an installed app: fall back to the
        // models directory the first-run download populates. The environment
        // still wins where it is set, which is what CI and local benchmarking
        // rely on.
        if let Some(dir) = crate::models::resolve_embed_dir(app) {
            config.model_dir = dir;
        }
    }
    // Lazy: the model loads on the first embed, not here, so a missing model
    // directory doesn't delay launch — it surfaces as a logged indexing error.
    Some(Arc::new(BgeEmbedder::new(config)))
}

#[cfg(not(feature = "embed"))]
fn build_embedder(_app: &AppHandle) -> Option<Arc<dyn Embedder>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use kodabi_core::index::NoteType;

    const CUTOFF: &str = "2026-07-18T12:00:00Z";

    #[test]
    fn the_backfill_window_holds_back_only_old_hand_written_notes() {
        // The case the window exists for: a to-do the user wrote months ago,
        // which would arrive already past the stale threshold and sort above
        // everything they are actually working on.
        assert!(!backfill_enrols(
            NoteType::Note,
            "2026-01-05T00:00:00Z",
            CUTOFF
        ));
        // A recent one is exactly what it should catch.
        assert!(backfill_enrols(
            NoteType::Note,
            "2026-08-01T00:00:00Z",
            CUTOFF
        ));
        // The boundary is inclusive.
        assert!(backfill_enrols(NoteType::Note, CUTOFF, CUTOFF));

        // Meetings and chats have been forwarded unconditionally since the
        // meeting-facts backfill shipped; narrowing that now would silently
        // drop commitments from a field database mid-backfill.
        for note_type in [NoteType::Meeting, NoteType::Chat] {
            assert!(
                backfill_enrols(note_type, "2025-01-01T00:00:00Z", CUTOFF),
                "{note_type:?} is never windowed"
            );
        }
    }
}
