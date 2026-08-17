//! Schema migrations for the commitment ledger.
//!
//! Versioning matches [`crate::index`]'s: SQLite's `user_version` pragma,
//! migration at 0-based index `i` defines schema version `i + 1`, and the list is
//! **append-only** — never edit or reorder an existing entry, because databases
//! in the field have already applied it.
//!
//! **One doctrine differs, and it is the important one.** The index is a
//! rebuildable cache, so a breaking change there may bump the version and
//! recreate the affected tables. The ledger holds judgements that exist nowhere
//! else, so **drop-and-recreate is never an option here**: a migration that
//! cannot carry the existing rows forward is a migration that destroys user data.
//! Write the `ALTER`/backfill, or write a new table and copy into it.

use rusqlite::Connection;

use super::Result;

/// The schema version this build writes, for the tests that assert a freshly
/// opened database lands on it. Not read at runtime: [`apply`] derives the
/// version it writes from the list's length, so the two can never disagree.
#[cfg(test)]
pub(crate) const CURRENT_VERSION: i64 = 1;

/// The ordered migrations, as lazy builders. Each is applied in its own
/// transaction, so a failure leaves the database at the last fully-applied
/// version. Builders are only invoked for versions that actually run, so a
/// no-op open never formats any migration SQL.
fn migrations() -> Vec<fn() -> String> {
    vec![migration_0001_initial_schema]
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

/// v1 — the initial ledger schema: entries keyed by their own durable id, the
/// item references that re-link them to extracted lines as those lines churn,
/// evidence claims, and the supersede/refresh graph between entries.
///
/// Three constraint decisions are load-bearing and stated here rather than only
/// in the DDL comments:
///
/// * **`closed_via` is present exactly when the state is `closed`.** Expressed as
///   a boolean equality so neither half can drift; a closure with no provenance
///   is unrepresentable rather than merely discouraged.
/// * **`ledger_item_refs` is unique on `(note_id, item_id)` only where
///   `active = 1`.** A live extracted line belongs to at most one entry, but the
///   retired history of that same line is unbounded — a description edited three
///   times leaves three retired rows.
/// * **No `done` column, anywhere.** The checkbox in the note is the source of
///   truth for done/not-done; a copy here would be a second, staler truth.
fn migration_0001_initial_schema() -> String {
    r#"
CREATE TABLE ledger_entries (
    entry_id            TEXT PRIMARY KEY,
    state               TEXT NOT NULL CHECK (state IN
                          ('open', 'needs_review', 'closed', 'superseded', 'waived', 'snoozed')),
    direction           TEXT NOT NULL CHECK (direction IN ('mine', 'theirs', 'unassigned')),
    owner               TEXT NOT NULL,
    description         TEXT NOT NULL,
    owner_norm          TEXT NOT NULL,
    description_norm    TEXT NOT NULL,
    project             TEXT NOT NULL,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    last_mention        TEXT NOT NULL,
    last_evidence_check TEXT,
    snoozed_until       TEXT CHECK (snoozed_until IS NULL OR state = 'snoozed'),
    closed_via          TEXT CHECK (closed_via IN ('manual', 'conversation', 'github')),
    review_reason       TEXT CHECK (review_reason IS NULL OR state = 'needs_review'),
    CHECK ((state = 'closed') = (closed_via IS NOT NULL))
);
CREATE INDEX idx_ledger_entries_state ON ledger_entries (state);
CREATE INDEX idx_ledger_entries_project ON ledger_entries (project);
CREATE INDEX idx_ledger_entries_match ON ledger_entries (owner_norm, description_norm);
CREATE INDEX idx_ledger_entries_mention ON ledger_entries (last_mention);

CREATE TABLE ledger_item_refs (
    entry_id   TEXT NOT NULL REFERENCES ledger_entries (entry_id) ON DELETE CASCADE,
    item_id    TEXT NOT NULL,
    note_id    TEXT NOT NULL,
    active     INTEGER NOT NULL CHECK (active IN (0, 1)),
    linked_at  TEXT NOT NULL,
    retired_at TEXT,
    PRIMARY KEY (entry_id, note_id, item_id),
    CHECK ((active = 0) = (retired_at IS NOT NULL))
);
CREATE UNIQUE INDEX idx_ledger_item_refs_live
    ON ledger_item_refs (note_id, item_id) WHERE active = 1;
CREATE INDEX idx_ledger_item_refs_item ON ledger_item_refs (item_id);
CREATE INDEX idx_ledger_item_refs_note ON ledger_item_refs (note_id);

CREATE TABLE ledger_evidence (
    evidence_id TEXT PRIMARY KEY,
    entry_id    TEXT NOT NULL REFERENCES ledger_entries (entry_id) ON DELETE CASCADE,
    source      TEXT NOT NULL CHECK (source IN ('manual', 'conversation', 'github')),
    reference   TEXT,
    confidence  REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    observed_at TEXT NOT NULL
);
CREATE INDEX idx_ledger_evidence_entry ON ledger_evidence (entry_id);

CREATE TABLE ledger_entry_links (
    from_entry TEXT NOT NULL REFERENCES ledger_entries (entry_id) ON DELETE CASCADE,
    to_entry   TEXT NOT NULL REFERENCES ledger_entries (entry_id) ON DELETE CASCADE,
    kind       TEXT NOT NULL CHECK (kind IN ('supersedes', 'refreshes')),
    created_at TEXT NOT NULL,
    PRIMARY KEY (from_entry, to_entry),
    CHECK (from_entry <> to_entry)
);
CREATE INDEX idx_ledger_entry_links_to ON ledger_entry_links (to_entry);
"#
    .to_string()
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use tempfile::tempdir;

    /// Opens a migrated connection the way `Ledger::init` does.
    fn migrated() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        super::apply(&mut conn).unwrap();
        conn
    }

    fn table_exists(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
            == 1
    }

    fn index_exists(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
            [name],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
            == 1
    }

    /// Seeds one entry with a ref, an evidence claim, and a link to a second
    /// entry — the full row set, so the first real upgrade test has a data
    /// fixture ready and this test proves v1 accepts every shape.
    fn seed_full_row_set(conn: &Connection) {
        for (id, state, via) in [
            ("le_aaaaaaaaaaaa", "open", None::<&str>),
            ("le_bbbbbbbbbbbb", "closed", Some("github")),
        ] {
            conn.execute(
                "INSERT INTO ledger_entries
                     (entry_id, state, direction, owner, description, owner_norm,
                      description_norm, project, created_at, updated_at, last_mention, closed_via)
                 VALUES (?1, ?2, 'theirs', 'Priya', 'send the revised deck', 'priya',
                         'send the revised deck', 'Briarwood Golf',
                         '2026-08-01T17:03:00Z', '2026-08-01T17:03:00Z', '2026-08-01T00:00:00Z', ?3)",
                rusqlite::params![id, state, via],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO ledger_item_refs (entry_id, item_id, note_id, active, linked_at)
             VALUES ('le_aaaaaaaaaaaa', 'a_111111', 'n_a1b2c3', 1, '2026-08-01T17:03:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ledger_evidence
                 (evidence_id, entry_id, source, reference, confidence, observed_at)
             VALUES ('ev_cccccccccccc', 'le_aaaaaaaaaaaa', 'github',
                     'https://example.com/pull/42', 0.8, '2026-08-15T02:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ledger_entry_links (from_entry, to_entry, kind, created_at)
             VALUES ('le_bbbbbbbbbbbb', 'le_aaaaaaaaaaaa', 'supersedes', '2026-08-10T11:00:00Z')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn migration_creates_every_table_and_sets_the_version() {
        let conn = migrated();
        for table in [
            "ledger_entries",
            "ledger_item_refs",
            "ledger_evidence",
            "ledger_entry_links",
        ] {
            assert!(table_exists(&conn, table), "{table} should exist");
        }
        // The partial unique index is the "one live entry per line" guarantee,
        // not an optimization, so it is asserted by name.
        assert!(index_exists(&conn, "idx_ledger_item_refs_live"));

        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::CURRENT_VERSION);
    }

    #[test]
    fn applying_migrations_twice_is_a_noop() {
        let mut conn = migrated();
        seed_full_row_set(&conn);
        super::apply(&mut conn).unwrap();

        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::CURRENT_VERSION);
        let entries: i64 = conn
            .query_row("SELECT count(*) FROM ledger_entries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(entries, 2, "re-applying must not touch data");
    }

    #[test]
    fn v1_accepts_a_full_row_set_and_enforces_its_invariants() {
        let conn = migrated();
        seed_full_row_set(&conn);

        // A closure with no provenance is unrepresentable.
        let err = conn.execute(
            "INSERT INTO ledger_entries
                 (entry_id, state, direction, owner, description, owner_norm,
                  description_norm, project, created_at, updated_at, last_mention)
             VALUES ('le_dddddddddddd', 'closed', 'mine', 'You', 'x', 'you', 'x', 'Ops',
                     '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z')",
            [],
        );
        assert!(err.is_err(), "closed without closed_via must be rejected");

        // So is provenance on an open entry.
        let err = conn.execute(
            "INSERT INTO ledger_entries
                 (entry_id, state, direction, owner, description, owner_norm,
                  description_norm, project, created_at, updated_at, last_mention, closed_via)
             VALUES ('le_eeeeeeeeeeee', 'open', 'mine', 'You', 'x', 'you', 'x', 'Ops',
                     '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z',
                     'manual')",
            [],
        );
        assert!(err.is_err(), "open with closed_via must be rejected");

        // Two live entries cannot claim the same extracted line.
        let err = conn.execute(
            "INSERT INTO ledger_item_refs (entry_id, item_id, note_id, active, linked_at)
             VALUES ('le_bbbbbbbbbbbb', 'a_111111', 'n_a1b2c3', 1, '2026-08-02T00:00:00Z')",
            [],
        );
        assert!(err.is_err(), "a live line belongs to one entry");

        // But the retired history of that line is unbounded.
        conn.execute(
            "INSERT INTO ledger_item_refs
                 (entry_id, item_id, note_id, active, linked_at, retired_at)
             VALUES ('le_bbbbbbbbbbbb', 'a_111111', 'n_a1b2c3', 0, '2026-08-02T00:00:00Z',
                     '2026-08-03T00:00:00Z')",
            [],
        )
        .unwrap();

        // Deleting an entry cascades to everything hanging off it.
        conn.execute(
            "DELETE FROM ledger_entries WHERE entry_id = 'le_aaaaaaaaaaaa'",
            [],
        )
        .unwrap();
        let refs: i64 = conn
            .query_row(
                "SELECT count(*) FROM ledger_item_refs WHERE entry_id = 'le_aaaaaaaaaaaa'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(refs, 0);
        let evidence: i64 = conn
            .query_row("SELECT count(*) FROM ledger_evidence", [], |row| row.get(0))
            .unwrap();
        assert_eq!(evidence, 0);
        let links: i64 = conn
            .query_row("SELECT count(*) FROM ledger_entry_links", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(links, 0);
    }

    #[test]
    fn a_migrated_database_survives_reopen_on_disk() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("ledger.db");
        {
            let mut conn = Connection::open(&path).unwrap();
            super::apply(&mut conn).unwrap();
            seed_full_row_set(&conn);
        }
        let mut conn = Connection::open(&path).unwrap();
        super::apply(&mut conn).unwrap();
        let entries: i64 = conn
            .query_row("SELECT count(*) FROM ledger_entries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(entries, 2, "durable state survives a reopen");
    }
}
