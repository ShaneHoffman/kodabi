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
//! At startup the worker also reconciles: any note already in the index but
//! missing its vectors — one indexed in a no-embed build, or stranded by a
//! transient embed failure — gets embedded from its stored body. Notes not yet
//! in the index at all await the vault rebuild (#49).

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};

use kodabi_core::embed::{self, Embedder};
use kodabi_core::index::{IndexedNote, NoteIndex};
use tauri::{AppHandle, Manager};

/// A handle to the background index worker, held as Tauri managed state.
pub struct IndexState {
    /// Sends notes to the worker thread. `None` when the database failed to
    /// open (logged at startup). Wrapped in a `Mutex` so the shared state stays
    /// `Sync` — the lock is held only for a non-blocking channel send.
    sender: Option<Mutex<Sender<IndexedNote>>>,
}

impl IndexState {
    /// Opens the index, builds the embedder, and spawns the worker. Never fails:
    /// an unopenable index degrades to "notes aren't indexed", not a launch
    /// failure.
    pub fn initialize(app: &AppHandle) -> Self {
        let Some(index) = open_index(app) else {
            eprintln!("note index unavailable — notes will not be searchable this session");
            return Self { sender: None };
        };
        let index = Arc::new(index);
        let embedder = build_embedder();
        let (sender, jobs) = mpsc::channel::<IndexedNote>();
        std::thread::spawn(move || run_worker(index, embedder, jobs));
        Self {
            sender: Some(Mutex::new(sender)),
        }
    }

    /// Hands `note` to the background worker to be upserted and (re-)embedded,
    /// returning immediately. Called after a note is written or edited on disk;
    /// the disk write is what mattered, so indexing runs off the command path
    /// and can never fail or delay it.
    pub fn index_note_best_effort(&self, note: IndexedNote) {
        let Some(sender) = &self.sender else {
            return;
        };
        let sender = sender.lock().unwrap_or_else(|poison| poison.into_inner());
        // `send` only fails if the worker thread is gone (shutdown) — the note
        // is dropped, which is fine for a best-effort cache.
        let _ = sender.send(note);
    }
}

/// The worker loop: reconcile once, then serve write/edit jobs in arrival order.
///
/// A single worker means phases of [`process_note`] never interleave with
/// another writer, so the lock can be released across the slow embed without
/// racing a concurrent write.
fn run_worker(
    index: Arc<Mutex<NoteIndex>>,
    embedder: Option<Arc<dyn Embedder>>,
    jobs: Receiver<IndexedNote>,
) {
    if let Some(embedder) = &embedder {
        reconcile_missing(&index, embedder.as_ref());
    }
    for note in jobs {
        process_note(&index, embedder.as_deref(), &note);
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
/// `app_data_dir()` rather than the KB root, because it is a machine-local
/// cache that should stay put even if a future vault-path setting relocates the
/// notes. Today the two directories coincide.
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
