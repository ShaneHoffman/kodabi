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

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};

use kodabi_core::embed::{self, Embedder};
use kodabi_core::index::{IndexedNote, NoteIndex};
use kodabi_core::reconcile;
use kodabi_core::watch::{self, VaultWatcher};
use tauri::{AppHandle, Emitter, Manager};

use crate::events::{INDEX_STATE_EVENT, VAULT_CHANGED_EVENT};
use crate::transcribe::knowledge_base_dir;

/// A unit of work for the index worker.
enum Job {
    /// Upsert and re-embed a single note after an in-app write. Boxed because it
    /// dwarfs the other variants, which carry no data.
    Note(Box<IndexedNote>),
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
    /// Keeps the OS file watcher alive for the app's lifetime; dropping it stops
    /// watching. `None` when the index or the vault path is unavailable. The
    /// `Mutex` only makes the state `Sync`; it is never locked after construction.
    _watcher: Option<Mutex<VaultWatcher>>,
}

impl IndexState {
    /// Opens the index, builds the embedder, and spawns the worker. Never fails:
    /// an unopenable index degrades to "notes aren't indexed", not a launch
    /// failure.
    pub fn initialize(app: &AppHandle) -> Self {
        let Some(index) = open_index(app) else {
            eprintln!("note index unavailable — notes will not be searchable this session");
            return Self {
                sender: None,
                _watcher: None,
            };
        };
        let index = Arc::new(index);
        let embedder = build_embedder();

        // The vault root the watcher observes and reconcile scans. Best-effort:
        // if it can't be resolved, the worker still serves in-app writes; only
        // the live-sync and rebuild have nothing to converge against.
        let vault_root = knowledge_base_dir(app).ok();

        let (sender, jobs) = mpsc::channel::<Job>();
        let app_handle = app.clone();
        let worker_root = vault_root.clone();
        std::thread::spawn(move || run_worker(app_handle, index, embedder, worker_root, jobs));

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
            _watcher: watcher,
        }
    }

    /// Hands `note` to the background worker to be upserted and (re-)embedded,
    /// returning immediately. Called after a note is written or edited on disk;
    /// the disk write is what mattered, so indexing runs off the command path
    /// and can never fail or delay it.
    pub fn index_note_best_effort(&self, note: IndexedNote) {
        self.send(Job::Note(Box::new(note)));
    }

    /// Requests a full rebuild of the index from files alone, returning whether
    /// the worker accepted it (false when the index is unavailable this session).
    /// Progress arrives on the `index:state` event.
    pub fn request_rebuild(&self) -> bool {
        self.send(Job::Rebuild)
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
) {
    for job in jobs {
        match job {
            Job::Note(note) => process_note(&index, embedder.as_deref(), &note),
            Job::Reconcile => {
                run_reconcile(&app, &index, embedder.as_deref(), vault_root.as_deref())
            }
            Job::Rebuild => run_rebuild(&app, &index, embedder.as_deref(), vault_root.as_deref()),
        }
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
) {
    let _ = app.emit(INDEX_STATE_EVENT, IndexStateEvent::Rebuilding);

    let Some(root) = vault_root else {
        let _ = app.emit(
            INDEX_STATE_EVENT,
            IndexStateEvent::Error {
                message: "the knowledge base folder could not be resolved".to_string(),
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
            let _ = app.emit(
                INDEX_STATE_EVENT,
                IndexStateEvent::Error {
                    message: err.to_string(),
                },
            );
        }
    }
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

/// Opens (creating if absent) the index database under the app-data dir.
///
/// The index lives at `app_data_dir()/index.db` — derived straight from
/// `app_data_dir()` rather than the KB root, because it is a machine-local cache
/// that must stay put even when a future vault-path setting relocates the notes.
/// This is a pinned invariant: the index is "derived, never synced", so it must
/// never sit inside the (syncable) KB folder. Today the two directories coincide,
/// which is exactly why the watcher filters `index.db*` out of its events.
fn open_index(app: &AppHandle) -> Option<Mutex<NoteIndex>> {
    let dir = match app.path().app_data_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("failed to resolve app data dir for the note index: {err}");
            return None;
        }
    };
    // First launch has no app-data dir yet.
    if let Err(err) = std::fs::create_dir_all(&dir) {
        eprintln!("failed to create app data dir for the note index: {err}");
        return None;
    }
    let path = dir.join("index.db");
    match NoteIndex::open(&path) {
        Ok(index) => Some(Mutex::new(index)),
        Err(err) => {
            eprintln!("failed to open the note index at {}: {err}", path.display());
            None
        }
    }
}

#[cfg(feature = "embed")]
fn build_embedder() -> Option<Arc<dyn Embedder>> {
    use kodabi_embed::{BgeConfig, BgeEmbedder};
    // Lazy: the model loads on the first embed, not here, so a missing model
    // directory doesn't delay launch — it surfaces as a logged indexing error.
    Some(Arc::new(BgeEmbedder::new(BgeConfig::from_env())))
}

#[cfg(not(feature = "embed"))]
fn build_embedder() -> Option<Arc<dyn Embedder>> {
    None
}
