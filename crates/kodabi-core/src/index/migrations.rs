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

/// The ordered migrations, as lazy builders. Each is applied in its own
/// transaction, so a failure leaves the database at the last fully-applied
/// version. Builders are only invoked for versions that actually run, so a
/// no-op open never formats any migration SQL.
fn migrations() -> Vec<fn() -> String> {
    vec![
        migration_0001_initial_schema,
        migration_0002_chunked_embeddings,
    ]
}

/// Applies every migration newer than the database's current `user_version`.
/// Idempotent: on an up-to-date database it runs nothing.
pub(crate) fn apply(conn: &mut Connection) -> Result<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let current = usize::try_from(current).unwrap_or(0);

    for (idx, build) in migrations().iter().enumerate().skip(current) {
        let version = idx + 1;
        let sql = build();
        let tx = conn.transaction()?;
        tx.execute_batch(&sql)?;
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
    // Historical DDL — frozen. This ran in the field with `notes_vec` at 768
    // dimensions, so the literal must stay byte-stable (migrations are
    // append-only). `migration_0002_chunked_embeddings` recreates `notes_vec`
    // at the current `EMBEDDING_DIM` and re-keys it per chunk; do not thread
    // the constant back through here.
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

CREATE VIRTUAL TABLE notes_vec USING vec0(note_id TEXT PRIMARY KEY, embedding FLOAT[768]);
"#
    .to_string()
}

/// v2 — chunked embeddings. Recreates `notes_vec` at the settled
/// [`EMBEDDING_DIM`] (bge-small-en-v1.5, 384) with **one row per body chunk**
/// rather than one per note, and adds a `note_chunks` companion holding each
/// chunk's source text.
///
/// The vector row is keyed by a synthetic `chunk_id` (`"{note_id}#{seq:04}"`)
/// so a note contributes several vectors; `note_id` and `seq` are `vec0`
/// *metadata* columns, which lets the pipeline delete a note's vectors with a
/// plain `WHERE note_id = ?` and lets the search surface read `note_id, seq,
/// distance` straight out of a KNN without parsing the key. `note_chunks`
/// stores the exact text that was embedded, so the search surface can return a
/// snippet by joining on `(note_id, seq)` without re-reading the source file.
///
/// v1 stored no embeddings (nothing wrote them), so dropping and recreating
/// `notes_vec` loses nothing; the whole index is a rebuildable cache regardless
/// (FOUNDING_DOC §3.6).
fn migration_0002_chunked_embeddings() -> String {
    // Single source of truth for the vector dimension (see `super::EMBEDDING_DIM`).
    let dim = EMBEDDING_DIM;
    format!(
        r#"
DROP TABLE notes_vec;
CREATE VIRTUAL TABLE notes_vec USING vec0(
    chunk_id  TEXT PRIMARY KEY,
    note_id   TEXT,
    seq       INTEGER,
    embedding FLOAT[{dim}]
);
CREATE TABLE note_chunks (
    note_id TEXT NOT NULL,
    seq     INTEGER NOT NULL,
    text    TEXT NOT NULL,
    PRIMARY KEY (note_id, seq)
);
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

    fn user_version(index: &NoteIndex) -> i64 {
        index
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn migration_creates_every_table_and_sets_the_version() {
        let index = NoteIndex::open_in_memory().unwrap();

        for table in [
            "notes",
            "note_tags",
            "notes_fts",
            "notes_vec",
            "note_chunks",
        ] {
            assert!(table_exists(&index, table), "missing table {table}");
        }

        assert_eq!(user_version(&index), 2);
    }

    #[test]
    fn applying_migrations_twice_is_a_noop() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        // Re-running against an already-migrated database must not error (it
        // would if it tried to re-create existing tables).
        super::apply(&mut index.conn).unwrap();

        assert_eq!(user_version(&index), 2);
    }

    #[test]
    fn a_v1_database_upgrades_to_v2_on_open() {
        // Simulate a database left at v1 (the schema that shipped before the
        // embedding pipeline): apply only the first migration, then run the
        // full sequence and confirm it advances to v2 and adds `note_chunks`.
        let mut index = NoteIndex::open_in_memory().unwrap();
        index
            .conn
            .execute_batch("PRAGMA user_version = 1;")
            .unwrap();
        // `notes_vec` already exists from the full open; drop it so the v2
        // `DROP TABLE notes_vec` in the re-applied migration has its target and
        // the recreate lands, faithfully mimicking a real v1 → v2 upgrade.
        index.conn.execute_batch("DROP TABLE note_chunks;").unwrap();

        super::apply(&mut index.conn).unwrap();

        assert_eq!(user_version(&index), 2);
        assert!(table_exists(&index, "note_chunks"));
    }

    #[test]
    fn notes_vec_holds_the_current_embedding_dimension() {
        use super::super::EMBEDDING_DIM;

        let index = NoteIndex::open_in_memory().unwrap();
        let ok = format!("[{}]", vec!["0"; EMBEDDING_DIM].join(","));
        index
            .conn
            .execute(
                "INSERT INTO notes_vec (chunk_id, note_id, seq, embedding)
                 VALUES ('n_dim#0000', 'n_dim', 0, ?1)",
                [ok],
            )
            .expect("a correctly sized vector inserts");

        let wrong = format!("[{}]", vec!["0"; EMBEDDING_DIM + 1].join(","));
        let err = index.conn.execute(
            "INSERT INTO notes_vec (chunk_id, note_id, seq, embedding)
             VALUES ('n_dim#0001', 'n_dim', 1, ?1)",
            [wrong],
        );
        assert!(err.is_err(), "a wrong-length vector is rejected");
    }
}
