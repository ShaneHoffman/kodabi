//! The SQLite index over the notes corpus — FTS5 full-text + `sqlite-vec`
//! vectors.
//!
//! Plain Markdown + YAML frontmatter is the source of truth; this index is a
//! **rebuildable cache** derived from it, never the other way around
//! (FOUNDING_DOC §3.6, FRONTMATTER_SCHEMA "storage"). It can be nuked and
//! reconstructed from the `.md` files at any time, so nothing is stored here
//! that isn't re-derivable from frontmatter + bodies.
//!
//! A row keyed by the stable frontmatter `id` caches the fields the watcher and
//! query surface need — `project`, `date` (normalized to UTC for ordering,
//! alongside the verbatim value), `tags`, `type`, `confidence`, plus `path`,
//! `title`, `source`, and the `body` that backs full-text search. The
//! `sqlite-vec` table holds one L2-normalized embedding per body chunk, keyed by
//! the note's `id`; the embedding pipeline (`crate::embed`) owns writing it.

mod embed;
mod migrations;
mod note;
mod outstanding;
mod query;
mod scope;
mod search;

pub use embed::{ChunkHit, EmbeddedChunk};

pub use note::{
    normalize_date_to_utc, ActionItemRow, ActionItemStatus, IndexedNote, MeetingFactsRow, NoteRef,
    NoteRow, NoteType, NoteTypeCounts, UnknownNoteType,
};

pub use outstanding::{OutstandingItem, OutstandingParams, OutstandingResults, OutstandingSummary};

pub use scope::ProjectScope;

pub use search::{PageInfo, SearchHit, SearchParams, SearchResults, TagMatch};

use std::path::Path;
use std::sync::Once;

use rusqlite::Connection;

/// Embedding dimension for the `sqlite-vec` table.
///
/// Settled on **bge-small-en-v1.5** (FOUNDING_DOC's "Embedding model" open
/// question, resolved in Phase 2): a 384-dimensional CLS-pooled sentence
/// encoder. Stored vectors are L2-normalized, so `vec0`'s default L2 distance
/// rank-orders identically to cosine similarity. Passages are embedded bare and
/// queries carry the model's instruction prefix — that asymmetry lives in the
/// `Embedder` impl (`crate::embed`), not here. The index is rebuildable, so
/// changing models is a one-line bump plus a new migration that recreates
/// `notes_vec` (see `migration_0002_chunked_embeddings`).
pub const EMBEDDING_DIM: usize = 384;

/// Errors from opening, migrating, or querying the note index.
#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    /// Any failure surfaced by SQLite (open, migrate, query, constraint).
    #[error("index database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A note's frontmatter `date` could not be parsed for UTC normalization.
    #[error("invalid note date {value:?}: {source}")]
    Date {
        value: String,
        #[source]
        source: chrono::ParseError,
    },
    /// An embedding handed to the vector store was not [`EMBEDDING_DIM`] long.
    #[error("embedding dimension mismatch: expected {expected}, got {got}")]
    EmbeddingDim { expected: usize, got: usize },
    /// A pagination `cursor` was malformed — not one this index produced (wrong
    /// version prefix, bad sort-key encoding, or an empty boundary id). Shared
    /// by every paginated index query, each of which owns its own codec.
    #[error("invalid pagination cursor {value:?}")]
    Cursor { value: String },
    /// A `search_notes` filter array carried more values than the query planner
    /// will bind. Rejected rather than truncated: silently dropping a value
    /// would widen an `all`-style filter into a different question.
    #[error("too many {field} filter values: {got} (maximum {max})")]
    FilterTooLarge {
        field: &'static str,
        max: usize,
        got: usize,
    },
}

/// `Result` specialised to [`IndexError`].
pub type Result<T> = std::result::Result<T, IndexError>;

/// One-time, process-wide registration of the `sqlite-vec` extension.
static VEC_INIT: Once = Once::new();

/// Registers `sqlite-vec` so every connection opened afterward exposes the
/// `vec0` virtual table module. `sqlite3_auto_extension` installs it
/// process-wide; the [`Once`] guard makes repeated [`NoteIndex`] opens (e.g. in
/// tests) register it exactly once.
fn register_vec_extension() {
    // The entry-point signature `sqlite3_auto_extension` expects. `sqlite-vec`
    // declares `sqlite3_vec_init` against its own opaque `sqlite3` type, so we
    // erase to `*const ()` and transmute to this (rusqlite's) matching type —
    // the registration idiom from the crate's own examples. Portable
    // `c_char`/`c_int` keep the pointer types correct across platforms.
    type EntryPoint = unsafe extern "C" fn(
        *mut rusqlite::ffi::sqlite3,
        *mut *mut std::os::raw::c_char,
        *const rusqlite::ffi::sqlite3_api_routines,
    ) -> std::os::raw::c_int;

    VEC_INIT.call_once(|| {
        // SAFETY: `sqlite3_vec_init` is the extension entry point that
        // `sqlite-vec` documents for this exact call; `EntryPoint` is its true
        // ABI. This is the crate's only FFI.
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(
                std::mem::transmute::<*const (), EntryPoint>(
                    sqlite_vec::sqlite3_vec_init as *const (),
                ),
            ));
        }
    });
}

/// A handle to the SQLite note index, owning its connection.
///
/// Opening runs any pending schema migrations, so a returned `NoteIndex` is
/// always at the current schema version.
pub struct NoteIndex {
    conn: Connection,
}

impl NoteIndex {
    /// Opens (creating if absent) the index database at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        register_vec_extension();
        Self::init(Connection::open(path)?)
    }

    /// Opens a fresh in-memory index — for tests and ephemeral rebuilds.
    pub fn open_in_memory() -> Result<Self> {
        register_vec_extension();
        Self::init(Connection::open_in_memory()?)
    }

    /// Configures connection pragmas and migrates to the current schema. WAL
    /// suits a desktop reader/writer pair (the watcher writes, MCP reads); it is
    /// harmless on an in-memory database. `foreign_keys` must be enabled per
    /// connection for the `note_tags` cascade, and is set before the first
    /// transaction so it takes effect.
    fn init(mut conn: Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
        migrations::apply(&mut conn)?;
        Ok(Self { conn })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Builds a `sqlite-vec` JSON vector literal of the configured dimension,
    /// with `value` in the first slot and zeros elsewhere.
    fn vector_json(value: f32) -> String {
        let mut parts = vec![value.to_string()];
        parts.extend(std::iter::repeat_n("0".to_string(), EMBEDDING_DIM - 1));
        format!("[{}]", parts.join(","))
    }

    #[test]
    fn open_in_memory_is_ready_to_use() {
        let index = NoteIndex::open_in_memory().unwrap();
        let count: i64 = index
            .conn
            .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn vec_table_accepts_and_knn_searches_embeddings() {
        // Proves the sqlite-vec extension actually loaded and the vec0 table is
        // usable end-to-end — insert a chunk vector, then find it by
        // nearest-neighbour and read back its `note_id` metadata column.
        let index = NoteIndex::open_in_memory().unwrap();
        index
            .conn
            .execute(
                "INSERT INTO notes_vec (chunk_id, note_id, seq, embedding)
                 VALUES ('n_abc123#0000', 'n_abc123', 0, ?1)",
                [vector_json(1.0)],
            )
            .unwrap();

        let hit: String = index
            .conn
            .query_row(
                "SELECT note_id FROM notes_vec
                 WHERE embedding MATCH ?1 ORDER BY distance LIMIT 1",
                [vector_json(1.0)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hit, "n_abc123");
    }

    #[test]
    fn a_file_backed_index_persists_across_reopen() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("index.sqlite");

        {
            let mut index = NoteIndex::open(&path).unwrap();
            index
                .upsert_note(&crate::index::IndexedNote {
                    id: "n_persist".to_string(),
                    path: "Inbox/n.md".to_string(),
                    title: "Persisted".to_string(),
                    note_type: NoteType::Note,
                    project: None,
                    date: "2026-07-11".to_string(),
                    tags: vec![],
                    source: "manual".to_string(),
                    confidence: None,
                    body: "durable body".to_string(),
                    meeting: None,
                })
                .unwrap();
        }

        // A fresh handle over the same file sees the persisted row, and opening
        // it does not re-run migrations destructively.
        let reopened = NoteIndex::open(&path).unwrap();
        let note = reopened.get_note("n_persist").unwrap().unwrap();
        assert_eq!(note.title, "Persisted");
    }
}
