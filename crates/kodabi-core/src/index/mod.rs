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
//! `sqlite-vec` table is created here; embeddings are populated later (Phase 2
//! embedding pipeline).

mod migrations;
mod note;
mod query;

pub use note::{normalize_date_to_utc, IndexedNote, NoteRow, NoteType, UnknownNoteType};

use std::path::Path;
use std::sync::Once;

use rusqlite::Connection;

/// Provisional embedding dimension for the `sqlite-vec` table.
///
/// The embedding model is still an open Phase 2 decision (FOUNDING_DOC's
/// "Embedding model" open question — bge-small=384 / nomic-embed=768 / other);
/// 768 tracks nomic-embed as a placeholder. No embeddings are written by this
/// crate yet (the embedding pipeline is a separate task), and the index is
/// rebuildable, so settling on a different model is a one-line bump plus a new
/// migration that recreates `notes_vec`.
pub const EMBEDDING_DIM: usize = 768;

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
        // usable end-to-end — insert a vector, then find it by nearest-neighbour.
        let index = NoteIndex::open_in_memory().unwrap();
        index
            .conn
            .execute(
                "INSERT INTO notes_vec (note_id, embedding) VALUES ('n_abc123', ?1)",
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
