//! Managed state for the SQLite note index and its embedder.
//!
//! The index is a **rebuildable cache** derived from the Markdown vault, so
//! nothing here is load-bearing for correctness: if the database can't be
//! opened the app still runs (notes just aren't searchable this session), and
//! indexing a note is best-effort — a failure is logged, never surfaced to the
//! command, because the `.md` file on disk is the source of truth.
//!
//! The embedder is present only in `embed`-feature builds; the default build
//! carries `None`, so `index_note` populates the note row and full-text index
//! but writes no vectors.

use std::sync::{Arc, Mutex};

use kodabi_core::embed::Embedder;
use kodabi_core::index::{IndexedNote, NoteIndex};
use tauri::{AppHandle, Manager};

/// The note index plus its optional embedder, held as Tauri managed state.
pub struct IndexState {
    /// `None` when the database failed to open (logged at startup).
    index: Option<Mutex<NoteIndex>>,
    /// `None` in default builds (no embedding backend compiled in).
    embedder: Option<Arc<dyn Embedder>>,
}

impl IndexState {
    /// Opens the index and builds the embedder. Never fails: an unopenable
    /// index degrades to "notes aren't indexed", not a launch failure.
    pub fn initialize(app: &AppHandle) -> Self {
        let index = open_index(app);
        if index.is_none() {
            eprintln!("note index unavailable — notes will not be searchable this session");
        }
        Self {
            index,
            embedder: build_embedder(),
        }
    }

    /// Upserts `note` into the index and refreshes its embeddings, logging and
    /// swallowing any error. Called after a note is written or edited on disk;
    /// the disk write is what mattered, so indexing must never fail the command.
    pub fn index_note_best_effort(&self, note: &IndexedNote) {
        let Some(index) = &self.index else {
            return;
        };
        // Recover a poisoned lock: the index is a cache, and a prior panic
        // mid-write leaves at worst a stale row that the next upsert overwrites.
        let mut index = index.lock().unwrap_or_else(|poison| poison.into_inner());
        if let Err(err) = kodabi_core::embed::index_note(&mut index, note, self.embedder.as_deref())
        {
            eprintln!("failed to index note {}: {err}", note.id);
        }
    }
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
