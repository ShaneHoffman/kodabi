//! Schema migrations for the note index.
//!
//! Versioning is tracked in SQLite's `user_version` pragma: migration at
//! 0-based index `i` defines schema version `i + 1`, and `user_version` records
//! the highest version applied. The list is **append-only** — never edit or
//! reorder an existing entry, because databases in the field have already
//! applied it; add a new entry instead. The whole index is a rebuildable cache
//! (FOUNDING_DOC §3.6), so a breaking change can also just bump the version and
//! recreate affected tables.

use rusqlite::Connection;

use super::{Result, EMBEDDING_DIM};

/// The ordered migrations. Each is applied in its own transaction, so a failure
/// leaves the database at the last fully-applied version.
fn migrations() -> Vec<String> {
    vec![migration_0001_initial_schema()]
}

/// Applies every migration newer than the database's current `user_version`.
/// Idempotent: on an up-to-date database it runs nothing.
pub(crate) fn apply(conn: &mut Connection) -> Result<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let current = usize::try_from(current).unwrap_or(0);

    for (idx, sql) in migrations().iter().enumerate().skip(current) {
        let version = idx + 1;
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        // `user_version` takes a literal, not a bound parameter; `version` is a
        // trusted in-process integer, so interpolating it is safe. Set inside
        // the transaction so a failed migration rolls the version back too.
        tx.execute_batch(&format!("PRAGMA user_version = {version};"))?;
        tx.commit()?;
    }
    Ok(())
}

/// v1 — the initial schema: the `notes` table keyed by the stable `id`, a
/// normalized `note_tags` junction, an external-content FTS5 index over
/// title+body kept in sync by triggers, and a `sqlite-vec` table for embeddings.
fn migration_0001_initial_schema() -> String {
    // Single source of truth for the vector dimension (see `super::EMBEDDING_DIM`).
    let dim = EMBEDDING_DIM;
    format!(
        r#"
CREATE TABLE notes (
    pk         INTEGER PRIMARY KEY,
    id         TEXT NOT NULL UNIQUE,
    path       TEXT NOT NULL,
    title      TEXT NOT NULL,
    type       TEXT NOT NULL CHECK (type IN ('meeting', 'note', 'chat')),
    project    TEXT,
    date_raw   TEXT NOT NULL,
    date_utc   TEXT NOT NULL,
    source     TEXT NOT NULL,
    confidence REAL CHECK (confidence IS NULL OR (confidence >= 0.0 AND confidence <= 1.0)),
    body       TEXT NOT NULL
);
CREATE INDEX idx_notes_project ON notes (project);
CREATE INDEX idx_notes_date_utc ON notes (date_utc);
CREATE INDEX idx_notes_type ON notes (type);

CREATE TABLE note_tags (
    note_pk INTEGER NOT NULL REFERENCES notes (pk) ON DELETE CASCADE,
    tag     TEXT NOT NULL,
    PRIMARY KEY (note_pk, tag)
);
CREATE INDEX idx_note_tags_tag ON note_tags (tag);

CREATE VIRTUAL TABLE notes_fts USING fts5(title, body, content='notes', content_rowid='pk');

CREATE TRIGGER notes_ai AFTER INSERT ON notes BEGIN
    INSERT INTO notes_fts (rowid, title, body) VALUES (new.pk, new.title, new.body);
END;
CREATE TRIGGER notes_ad AFTER DELETE ON notes BEGIN
    INSERT INTO notes_fts (notes_fts, rowid, title, body)
        VALUES ('delete', old.pk, old.title, old.body);
END;
CREATE TRIGGER notes_au AFTER UPDATE ON notes BEGIN
    INSERT INTO notes_fts (notes_fts, rowid, title, body)
        VALUES ('delete', old.pk, old.title, old.body);
    INSERT INTO notes_fts (rowid, title, body) VALUES (new.pk, new.title, new.body);
END;

CREATE VIRTUAL TABLE notes_vec USING vec0(note_id TEXT PRIMARY KEY, embedding FLOAT[{dim}]);
"#
    )
}

#[cfg(test)]
mod tests {
    use super::super::NoteIndex;

    fn table_exists(index: &NoteIndex, name: &str) -> bool {
        index
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE name = ?1",
                [name],
                |_| Ok(()),
            )
            .is_ok()
    }

    #[test]
    fn migration_creates_every_table_and_sets_the_version() {
        let index = NoteIndex::open_in_memory().unwrap();

        for table in ["notes", "note_tags", "notes_fts", "notes_vec"] {
            assert!(table_exists(&index, table), "missing table {table}");
        }

        let version: i64 = index
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1);
    }

    #[test]
    fn applying_migrations_twice_is_a_noop() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        // Re-running against an already-migrated database must not error (it
        // would if it tried to re-create existing tables).
        super::apply(&mut index.conn).unwrap();

        let version: i64 = index
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1);
    }
}
