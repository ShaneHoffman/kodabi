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
        migration_0003_meeting_facts,
        migration_0004_note_classification,
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

/// v3 — structured meeting facts for the MCP `get_note` tool. Adds three tables
/// holding what a distilled note's Markdown body (decisions, action items) and
/// its raw session transcript (duration, distinct speaker channels) contain, so
/// `get_note` can serve `MeetingMeta` + `ActionItem` without re-parsing the body
/// or re-reading the JSONL on every call.
///
/// **These tables serve meeting *and* chat notes** (`meeting::derives_facts`);
/// the `note_meetings` name is historical, predating the chat leg, and was kept
/// because renaming it would cost a migration for no behavior change while the
/// MCP wire object is still `meeting`/`MeetingMeta`. Nothing here is type-aware:
/// `note_id` is a bare key and no column records the note type.
///
/// - `note_meetings` — one row per meeting or chat note: the two session-derived
///   scalars (`duration_seconds`, `speaker_count`), both nullable because a note
///   whose `source` is a keyword (e.g. `manual`) or whose transcript was pruned
///   by retention has no transcript to measure. **Both are always `NULL` for a
///   chat**, whose `source` is a chat transcript, not a session recording — that
///   pre-existing nullability is why adding chats needed no migration at all.
///   A note with zero decisions and zero action items still gets a row, and that
///   row is deliberate: it is the sentinel that makes
///   `note_ids_missing_meeting_facts` converge instead of re-deriving the same
///   empty note on every backfill pass.
/// - `note_decisions` — the ordered `## Decisions` bullets.
/// - `note_action_items` — the ordered `## Action items` lines, parsed into the
///   grammar's fields; `item_id` is a deterministic hash of the note id + the
///   line's content, so it is stable across reindexes (see `crate::meeting`).
///   `done` is the checkbox state; `overdue` is derived server-side and not
///   stored.
///
/// All three are keyed by `note_id TEXT` (not the `notes.pk` foreign key), so
/// they sit beside `notes_vec`/`note_chunks` as a body/session-derived cache and
/// are cleared explicitly in `delete_note`/`clear`. Because the whole index is a
/// rebuildable cache (FOUNDING_DOC §3.6), a field database gains three empty
/// tables on upgrade and repopulates them via the meeting-facts backfill pass
/// (`reconcile::reconcile_missing_meeting_facts`) — nothing is lost. That pass
/// covers every chat note too, so a database indexed before chats carried facts
/// converges on the next launch with no schema change.
fn migration_0003_meeting_facts() -> String {
    r#"
CREATE TABLE note_meetings (
    note_id          TEXT PRIMARY KEY,
    duration_seconds INTEGER CHECK (duration_seconds IS NULL OR duration_seconds >= 0),
    speaker_count    INTEGER CHECK (speaker_count IS NULL OR speaker_count >= 0)
);
CREATE TABLE note_decisions (
    note_id TEXT NOT NULL,
    seq     INTEGER NOT NULL,
    text    TEXT NOT NULL,
    PRIMARY KEY (note_id, seq)
);
CREATE TABLE note_action_items (
    note_id        TEXT NOT NULL,
    seq            INTEGER NOT NULL,
    item_id        TEXT NOT NULL,
    description    TEXT NOT NULL,
    owner          TEXT NOT NULL,
    due_date       TEXT,
    done           INTEGER NOT NULL CHECK (done IN (0, 1)),
    extracted_date TEXT,
    PRIMARY KEY (note_id, seq)
);
CREATE INDEX idx_note_action_items_item_id ON note_action_items (item_id);
"#
    .to_string()
}

/// v4: the two classification facets a note's frontmatter now carries — the
/// meeting's genre (`category`, with the classifier's `category_confidence` in
/// its own guess) and the per-meeting commitment-tracking override
/// (`tracking`).
///
/// Purely additive: three nullable columns on `notes` plus an index on
/// `category` for filtering. Nothing is lost on upgrade — a field database
/// gains three `NULL` columns and backfills them the moment the startup
/// reconcile re-reads each note, because frontmatter is the source of truth and
/// the index is a rebuildable cache (FOUNDING_DOC §3.6). Notes written before
/// this shipped carry neither key and correctly stay `NULL`.
///
/// `tracking` is the column the enrollment gate reads (`ledger::sync`), which
/// is why it is here rather than in `ledger.db`: the gate resolves a note's mode
/// from the facts the index already hands it, so the override now travels with
/// the note file the way a re-route or a vault rebuild always assumed it would.
/// It stores the **frontmatter** (kebab-case) spelling, like `source`, not the
/// snake_case one `ledger.db`'s own `CHECK` uses.
///
/// The `CHECK` constraints restate closed sets that Rust already validates
/// ([`crate::note::MeetingCategory`], [`crate::ledger::EnrollmentMode`]); they
/// are cheap insurance against a hand-edited database, and match how
/// `notes.type` has always been guarded.
fn migration_0004_note_classification() -> String {
    r#"
ALTER TABLE notes ADD COLUMN category TEXT
    CHECK (category IS NULL OR category IN (
        'standup', 'one-on-one', 'client', 'working-session',
        'review', 'all-hands', 'observer'
    ));
ALTER TABLE notes ADD COLUMN category_confidence REAL
    CHECK (category_confidence IS NULL
        OR (category_confidence >= 0.0 AND category_confidence <= 1.0));
ALTER TABLE notes ADD COLUMN tracking TEXT
    CHECK (tracking IS NULL OR tracking IN ('tracked', 'context-only'));
CREATE INDEX idx_notes_category ON notes (category);
"#
    .to_string()
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
            "note_meetings",
            "note_decisions",
            "note_action_items",
        ] {
            assert!(table_exists(&index, table), "missing table {table}");
        }

        assert_eq!(user_version(&index), 4);
    }

    #[test]
    fn applying_migrations_twice_is_a_noop() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        // Re-running against an already-migrated database must not error (it
        // would if it tried to re-create existing tables).
        super::apply(&mut index.conn).unwrap();

        assert_eq!(user_version(&index), 4);
    }

    #[test]
    fn a_v1_database_upgrades_to_the_latest_schema_on_open() {
        use super::super::EMBEDDING_DIM;

        // Reconstruct a genuine v1 database. The v1 `notes_vec` was a
        // single-row-per-note vec0 table at 768 dimensions; `note_chunks` (v2) and
        // the meeting-facts tables (v3) did not exist. Roll the freshly-opened
        // (latest) database back to exactly that shape so `apply` runs the real
        // chain — including the 768 -> EMBEDDING_DIM recreate — a field upgrade
        // would, not a same-dimension no-op.
        let mut index = NoteIndex::open_in_memory().unwrap();
        index
            .conn
            .execute_batch(
                "DROP TABLE note_action_items;
                 DROP TABLE note_decisions;
                 DROP TABLE note_meetings;
                 DROP TABLE note_chunks;
                 DROP TABLE notes_vec;
                 CREATE VIRTUAL TABLE notes_vec USING vec0(note_id TEXT PRIMARY KEY, embedding FLOAT[768]);
                 DROP INDEX idx_notes_category;
                 ALTER TABLE notes DROP COLUMN category;
                 ALTER TABLE notes DROP COLUMN category_confidence;
                 ALTER TABLE notes DROP COLUMN tracking;
                 PRAGMA user_version = 1;",
            )
            .unwrap();

        // Sanity: the reconstructed v1 table really is 768-wide, and holds a row
        // (the DROP in the migration must cope with data present).
        let v1_vec = format!("[{}]", vec!["0"; 768].join(","));
        index
            .conn
            .execute(
                "INSERT INTO notes_vec (note_id, embedding) VALUES ('n_old', ?1)",
                [&v1_vec],
            )
            .expect("the v1 table holds 768-dim vectors");

        super::apply(&mut index.conn).unwrap();

        assert_eq!(user_version(&index), 4);
        for table in [
            "note_chunks",
            "note_meetings",
            "note_decisions",
            "note_action_items",
        ] {
            assert!(table_exists(&index, table), "missing table {table}");
        }

        // The recreate actually changed the dimension: the upgraded table takes
        // an EMBEDDING_DIM (384) chunk vector and rejects the old 768-dim width.
        let new_vec = format!("[{}]", vec!["0"; EMBEDDING_DIM].join(","));
        index
            .conn
            .execute(
                "INSERT INTO notes_vec (chunk_id, note_id, seq, embedding)
                 VALUES ('n_new#0000', 'n_new', 0, ?1)",
                [&new_vec],
            )
            .expect("the upgraded table holds EMBEDDING_DIM vectors");
        let old_width = index.conn.execute(
            "INSERT INTO notes_vec (chunk_id, note_id, seq, embedding)
             VALUES ('n_bad#0000', 'n_bad', 0, ?1)",
            [&v1_vec],
        );
        assert!(
            old_width.is_err(),
            "the upgraded table must reject the old 768-dim width"
        );
    }

    #[test]
    fn a_v2_database_upgrades_to_the_latest_schema_on_open() {
        // Reconstruct a genuine v2 database: the meeting-facts tables did not
        // exist yet. Roll the freshly-opened (v3) database back to exactly that
        // shape so `apply` performs the real additive upgrade a field database
        // would, with a note already present (the migration must cope with data).
        let mut index = NoteIndex::open_in_memory().unwrap();
        index
            .conn
            .execute_batch(
                "DROP TABLE note_action_items;
                 DROP TABLE note_decisions;
                 DROP TABLE note_meetings;
                 DROP INDEX idx_notes_category;
                 ALTER TABLE notes DROP COLUMN category;
                 ALTER TABLE notes DROP COLUMN category_confidence;
                 ALTER TABLE notes DROP COLUMN tracking;
                 PRAGMA user_version = 2;",
            )
            .unwrap();
        index
            .conn
            .execute_batch(
                "INSERT INTO notes (id, path, title, type, project, date_raw, date_utc, source, body)
                 VALUES ('n_old', 'n_old.md', 'Old', 'meeting', NULL, '2026-01-01', '2026-01-01T00:00:00Z', 'manual', 'body');",
            )
            .expect("a v2 note row inserts");

        super::apply(&mut index.conn).unwrap();

        assert_eq!(user_version(&index), 4);
        for table in ["note_meetings", "note_decisions", "note_action_items"] {
            assert!(table_exists(&index, table), "missing table {table}");
        }
        // The upgraded tables accept rows keyed by the note id.
        index
            .conn
            .execute_batch(
                "INSERT INTO note_meetings (note_id, duration_seconds, speaker_count)
                     VALUES ('n_old', 900, 2);
                 INSERT INTO note_decisions (note_id, seq, text) VALUES ('n_old', 0, 'Ship it');
                 INSERT INTO note_action_items
                     (note_id, seq, item_id, description, owner, due_date, done, extracted_date)
                     VALUES ('n_old', 0, 'a_abc123', 'send the deck', 'You', NULL, 0, '2026-01-01');",
            )
            .expect("the upgraded meeting-facts tables accept rows");
    }

    #[test]
    fn a_v3_database_upgrades_to_v4_on_open() {
        // Reconstruct a genuine v3 database: the three classification columns
        // did not exist. Roll a freshly-opened (v4) database back to exactly
        // that shape so `apply` performs the real additive upgrade a field
        // database would, with a note already present.
        let mut index = NoteIndex::open_in_memory().unwrap();
        index
            .conn
            .execute_batch(
                "DROP INDEX idx_notes_category;
                 ALTER TABLE notes DROP COLUMN category;
                 ALTER TABLE notes DROP COLUMN category_confidence;
                 ALTER TABLE notes DROP COLUMN tracking;
                 PRAGMA user_version = 3;",
            )
            .unwrap();
        index
            .conn
            .execute_batch(
                "INSERT INTO notes (id, path, title, type, project, date_raw, date_utc, source, body)
                 VALUES ('n_old', 'n_old.md', 'Old', 'meeting', NULL, '2026-01-01', '2026-01-01T00:00:00Z', 'manual', 'body');",
            )
            .expect("a v3 note row inserts");

        super::apply(&mut index.conn).unwrap();

        assert_eq!(user_version(&index), 4);
        // The pre-existing row survives with NULL facets — nothing is lost, and
        // the reconcile pass backfills it from frontmatter.
        let (category, tracking): (Option<String>, Option<String>) = index
            .conn
            .query_row(
                "SELECT category, tracking FROM notes WHERE id = 'n_old'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("the upgraded row reads back");
        assert_eq!(category, None);
        assert_eq!(tracking, None);

        index
            .conn
            .execute_batch(
                "UPDATE notes SET category = 'one-on-one', category_confidence = 0.8,
                                  tracking = 'context-only' WHERE id = 'n_old';",
            )
            .expect("the upgraded columns accept valid values");
    }

    /// The `CHECK` constraints restate closed sets Rust owns, and SQL cannot
    /// see the enums. Driving this from `MeetingCategory::ALL` (and both
    /// `EnrollmentMode` variants) is what makes an added or renamed genre fail
    /// here rather than as a constraint violation on a user's machine.
    #[test]
    fn every_enum_spelling_is_accepted_by_its_check_constraint() {
        use crate::ledger::EnrollmentMode;
        use crate::note::MeetingCategory;

        let index = NoteIndex::open_in_memory().unwrap();
        index
            .conn
            .execute_batch(
                "INSERT INTO notes (id, path, title, type, project, date_raw, date_utc, source, body)
                 VALUES ('n_enum1', 'n_enum1.md', 'Enum', 'meeting', NULL, '2026-01-01', '2026-01-01T00:00:00Z', 'manual', 'body');",
            )
            .unwrap();

        for category in MeetingCategory::ALL {
            index
                .conn
                .execute(
                    "UPDATE notes SET category = ?1 WHERE id = 'n_enum1'",
                    [category.as_str()],
                )
                .unwrap_or_else(|err| {
                    panic!(
                        "the CHECK rejects {:?}, which MeetingCategory::ALL offers: {err}",
                        category.as_str()
                    )
                });
        }

        for mode in [EnrollmentMode::Tracked, EnrollmentMode::ContextOnly] {
            index
                .conn
                .execute(
                    "UPDATE notes SET tracking = ?1 WHERE id = 'n_enum1'",
                    [mode.as_frontmatter_str()],
                )
                .unwrap_or_else(|err| {
                    panic!(
                        "the CHECK rejects {:?}, which EnrollmentMode writes to frontmatter: {err}",
                        mode.as_frontmatter_str()
                    )
                });
        }
    }

    #[test]
    fn the_classification_columns_reject_values_outside_their_closed_sets() {
        let index = NoteIndex::open_in_memory().unwrap();
        index
            .conn
            .execute_batch(
                "INSERT INTO notes (id, path, title, type, project, date_raw, date_utc, source, body)
                 VALUES ('n_new', 'n_new.md', 'New', 'meeting', NULL, '2026-01-01', '2026-01-01T00:00:00Z', 'manual', 'body');",
            )
            .unwrap();

        for bad in [
            "UPDATE notes SET category = 'retro' WHERE id = 'n_new'",
            "UPDATE notes SET category_confidence = 1.5 WHERE id = 'n_new'",
            "UPDATE notes SET tracking = 'context_only' WHERE id = 'n_new'",
        ] {
            assert!(
                index.conn.execute_batch(bad).is_err(),
                "the CHECK constraint let this through: {bad}"
            );
        }
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
