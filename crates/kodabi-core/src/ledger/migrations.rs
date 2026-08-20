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
pub(crate) const CURRENT_VERSION: i64 = 3;

/// The ordered migrations, as lazy builders. Each is applied in its own
/// transaction, so a failure leaves the database at the last fully-applied
/// version. Builders are only invoked for versions that actually run, so a
/// no-op open never formats any migration SQL.
fn migrations() -> Vec<fn() -> String> {
    vec![
        migration_0001_initial_schema,
        migration_0002_enrollment,
        migration_0003_category_provenance,
    ]
}

/// Applies every migration newer than the database's current `user_version`.
/// Idempotent: on an up-to-date database it runs nothing.
///
/// **Foreign keys are disabled for the duration, and that is not an
/// optimization.** SQLite cannot alter a `CHECK` constraint, so widening one
/// (as v2 does, to admit the `untracked` state) means the create-copy-drop-rename
/// dance. Every child table references `ledger_entries` with `ON DELETE
/// CASCADE`, so dropping the old table under `foreign_keys = ON` would take
/// every item ref, evidence claim, and link with it — the exact data-loss this
/// module's doctrine forbids. The pragma is a no-op inside a transaction, hence
/// the toggle outside the loop; `foreign_key_check` afterwards proves nothing
/// was left dangling, and turns a silent orphan into a loud failure.
pub(crate) fn apply(conn: &mut Connection) -> Result<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let current = usize::try_from(current).unwrap_or(0);
    let pending = migrations();
    if current >= pending.len() {
        return Ok(());
    }

    let restore_foreign_keys: bool =
        conn.pragma_query_value(None, "foreign_keys", |row| row.get::<_, i64>(0))? == 1;
    if restore_foreign_keys {
        conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
    }

    let outcome = (|| -> Result<()> {
        for (idx, build) in pending.iter().enumerate().skip(current) {
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
    })();

    if restore_foreign_keys {
        let violations = outcome.is_ok().then(|| foreign_key_violations(conn));
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        outcome?;
        if violations.transpose()?.unwrap_or(0) > 0 {
            return Err(super::invalid_field(
                "foreign_key_check",
                "a migration left dangling rows behind",
            ));
        }
    } else {
        outcome?;
    }
    Ok(())
}

/// How many rows `PRAGMA foreign_key_check` objects to.
fn foreign_key_violations(conn: &Connection) -> Result<i64> {
    let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
    let count = stmt.query_map([], |_| Ok(()))?.count();
    Ok(i64::try_from(count).unwrap_or(i64::MAX))
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

/// v2 — the enrollment gate: the `untracked` state, enrollment provenance, the
/// "a person has touched this" flag, and per-meeting tracking overrides.
///
/// **This is a table rebuild, not an `ALTER`,** and the reason is narrow:
/// `state`'s `CHECK` is an inline column constraint, which SQLite offers no way
/// to alter. Admitting one more state therefore means create-copy-drop-rename.
/// The copy carries every v1 column across verbatim and lets the three new
/// columns take their defaults, so an existing row's meaning is unchanged: it
/// was enrolled because nothing said otherwise (`enrolled_via = 'default'`) and
/// nobody has acted on it yet (`touched = 0`). [`apply`] holds foreign keys off
/// around the drop — see its doc for why that is load-bearing rather than tidy.
///
/// Two constraint decisions worth stating here rather than only in the DDL:
///
/// * **`untracked_via` is present exactly when the state is `untracked`,**
///   spelled as the same boolean equality v1 used for `closed_via`. An untrack
///   with no provenance would break retro-application, which revives what an
///   override untracked and must leave a person's own untrack alone.
/// * **`ledger_note_overrides` stores `mode` as text rather than a boolean,**
///   even though only `context_only` was ever reachable through it (a plain
///   `tracked` meeting simply had no row).
///
///   **This table is now legacy and nothing writes it.** The override moved
///   into each note's frontmatter `tracking:` key when meeting categories
///   landed, which is where the "tracked, and I mean it, whatever my category
///   defaults to" value this text anticipated actually lives — frontmatter can
///   hold it, and it travels with the note through a re-route or a rebuild,
///   which a row keyed on a note id could only imitate with bookkeeping.
///   Remaining rows are drained into their notes at startup
///   (`ledger_state::drain_legacy_overrides`) and deleted as they graduate; a
///   pre-graduation `_ledger.yml` restores into this table and drains the same
///   way. The table itself stays until a later migration can prove every field
///   database has emptied it — this store carries rows forward, it does not
///   drop them.
fn migration_0002_enrollment() -> String {
    r#"
CREATE TABLE ledger_entries_new (
    entry_id            TEXT PRIMARY KEY,
    state               TEXT NOT NULL CHECK (state IN
                          ('open', 'needs_review', 'closed', 'superseded', 'waived',
                           'snoozed', 'untracked')),
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
    enrolled_via        TEXT NOT NULL DEFAULT 'default'
                          CHECK (enrolled_via IN ('default', 'override', 'manual')),
    untracked_via       TEXT CHECK (untracked_via IN ('manual', 'override')),
    touched             INTEGER NOT NULL DEFAULT 0 CHECK (touched IN (0, 1)),
    CHECK ((state = 'closed') = (closed_via IS NOT NULL)),
    CHECK ((state = 'untracked') = (untracked_via IS NOT NULL))
);

INSERT INTO ledger_entries_new
    (entry_id, state, direction, owner, description, owner_norm, description_norm,
     project, created_at, updated_at, last_mention, last_evidence_check,
     snoozed_until, closed_via, review_reason)
SELECT
     entry_id, state, direction, owner, description, owner_norm, description_norm,
     project, created_at, updated_at, last_mention, last_evidence_check,
     snoozed_until, closed_via, review_reason
FROM ledger_entries;

DROP TABLE ledger_entries;
ALTER TABLE ledger_entries_new RENAME TO ledger_entries;

CREATE INDEX idx_ledger_entries_state ON ledger_entries (state);
CREATE INDEX idx_ledger_entries_project ON ledger_entries (project);
CREATE INDEX idx_ledger_entries_match ON ledger_entries (owner_norm, description_norm);
CREATE INDEX idx_ledger_entries_mention ON ledger_entries (last_mention);

CREATE TABLE ledger_note_overrides (
    note_id TEXT PRIMARY KEY,
    project TEXT NOT NULL,
    mode    TEXT NOT NULL CHECK (mode IN ('tracked', 'context_only')),
    set_at  TEXT NOT NULL
);
CREATE INDEX idx_ledger_note_overrides_project ON ledger_note_overrides (project);
"#
    .to_string()
}

/// v3: admits `category` as a reason an entry was enrolled or untracked.
///
/// A meeting's *category* now carries an enrollment default
/// ([`crate::ledger::builtin_category_default`]), so the two provenance columns
/// gain a fourth and third value respectively: an entry can be in the working
/// set because its genre gates and it was addressed to you, and it can have left
/// the working set because the meeting was recategorized into such a genre.
/// Both are machine judgements, which is what keeps them revivable where a
/// person's `manual` is not.
///
/// **Another table rebuild, for the same narrow reason as v2:** both columns'
/// `CHECK`s are inline column constraints, and SQLite offers no way to alter
/// one. Unlike v2 this copy carries *every* column across verbatim, the three
/// v2 additions included — there is no new column to default, only a wider set
/// of values each may hold, so no existing row's meaning changes at all.
/// [`apply`] holds foreign keys off around the drop; see its doc.
fn migration_0003_category_provenance() -> String {
    r#"
CREATE TABLE ledger_entries_new (
    entry_id            TEXT PRIMARY KEY,
    state               TEXT NOT NULL CHECK (state IN
                          ('open', 'needs_review', 'closed', 'superseded', 'waived',
                           'snoozed', 'untracked')),
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
    enrolled_via        TEXT NOT NULL DEFAULT 'default'
                          CHECK (enrolled_via IN ('default', 'override', 'manual', 'category')),
    untracked_via       TEXT CHECK (untracked_via IN ('manual', 'override', 'category')),
    touched             INTEGER NOT NULL DEFAULT 0 CHECK (touched IN (0, 1)),
    CHECK ((state = 'closed') = (closed_via IS NOT NULL)),
    CHECK ((state = 'untracked') = (untracked_via IS NOT NULL))
);

INSERT INTO ledger_entries_new
    (entry_id, state, direction, owner, description, owner_norm, description_norm,
     project, created_at, updated_at, last_mention, last_evidence_check,
     snoozed_until, closed_via, review_reason, enrolled_via, untracked_via, touched)
SELECT
     entry_id, state, direction, owner, description, owner_norm, description_norm,
     project, created_at, updated_at, last_mention, last_evidence_check,
     snoozed_until, closed_via, review_reason, enrolled_via, untracked_via, touched
FROM ledger_entries;

DROP TABLE ledger_entries;
ALTER TABLE ledger_entries_new RENAME TO ledger_entries;

CREATE INDEX idx_ledger_entries_state ON ledger_entries (state);
CREATE INDEX idx_ledger_entries_project ON ledger_entries (project);
CREATE INDEX idx_ledger_entries_match ON ledger_entries (owner_norm, description_norm);
CREATE INDEX idx_ledger_entries_mention ON ledger_entries (last_mention);
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

    /// Opens a connection migrated only as far as v1, with foreign keys on —
    /// the state a database in the field is in before this build touches it.
    fn migrated_to_v1() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute_batch(&super::migration_0001_initial_schema())
            .unwrap();
        tx.execute_batch("PRAGMA user_version = 1;").unwrap();
        tx.commit().unwrap();
        conn
    }

    #[test]
    fn v2_upgrade_carries_every_row_forward_and_leaves_no_orphans() {
        let mut conn = migrated_to_v1();
        seed_full_row_set(&conn);

        super::apply(&mut conn).unwrap();

        // The rebuild's whole risk is the cascade: dropping the old table under
        // foreign_keys = ON would silently take the children with it.
        for (table, expected) in [
            ("ledger_entries", 2),
            ("ledger_item_refs", 1),
            ("ledger_evidence", 1),
            ("ledger_entry_links", 1),
        ] {
            let count: i64 = conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, expected, "{table} must survive the v2 rebuild");
        }

        let violations: i64 = super::foreign_key_violations(&conn).unwrap();
        assert_eq!(violations, 0, "the rebuild must leave no dangling rows");

        // Existing rows mean what they meant: enrolled because nothing said
        // otherwise, and untouched by a person.
        let (via, touched, untracked_via): (String, i64, Option<String>) = conn
            .query_row(
                "SELECT enrolled_via, touched, untracked_via FROM ledger_entries
                 WHERE entry_id = 'le_aaaaaaaaaaaa'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(via, "default");
        assert_eq!(touched, 0);
        assert_eq!(untracked_via, None);

        // The closure's provenance survived the copy verbatim.
        let closed_via: Option<String> = conn
            .query_row(
                "SELECT closed_via FROM ledger_entries WHERE entry_id = 'le_bbbbbbbbbbbb'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(closed_via.as_deref(), Some("github"));

        assert!(table_exists(&conn, "ledger_note_overrides"));
        // Rebuilt tables lose their indexes with them; the live-line guarantee
        // and the entry indexes have to be recreated by hand.
        for index in [
            "idx_ledger_item_refs_live",
            "idx_ledger_entries_state",
            "idx_ledger_entries_project",
            "idx_ledger_entries_match",
            "idx_ledger_entries_mention",
        ] {
            assert!(index_exists(&conn, index), "{index} should exist");
        }

        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::CURRENT_VERSION);
    }

    #[test]
    fn v2_cascades_still_fire_after_the_rebuild() {
        let mut conn = migrated_to_v1();
        seed_full_row_set(&conn);
        super::apply(&mut conn).unwrap();

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
        assert_eq!(refs, 0, "the recreated table must keep its cascades");
    }

    /// Opens a connection migrated only as far as v2 — the state a database in
    /// the field is in before this build touches it.
    fn migrated_to_v2() -> Connection {
        let mut conn = migrated_to_v1();
        conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute_batch(&super::migration_0002_enrollment())
            .unwrap();
        tx.execute_batch("PRAGMA user_version = 2;").unwrap();
        tx.commit().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn
    }

    #[test]
    fn v3_upgrade_carries_every_row_forward_including_the_v2_columns() {
        let mut conn = migrated_to_v2();
        seed_full_row_set(&conn);
        // The columns v2 added, carrying non-default values this time: v2's own
        // copy could let them default because they were new, and v3's must not.
        conn.execute(
            "UPDATE ledger_entries SET enrolled_via = 'manual', touched = 1
             WHERE entry_id = 'le_aaaaaaaaaaaa'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ledger_entries
                 (entry_id, state, direction, owner, description, owner_norm,
                  description_norm, project, created_at, updated_at, last_mention,
                  untracked_via, enrolled_via)
             VALUES ('le_cccccccccccc', 'untracked', 'theirs', 'Priya', 'x', 'priya', 'x', 'Ops',
                     '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z',
                     'override', 'default')",
            [],
        )
        .unwrap();

        super::apply(&mut conn).unwrap();

        for (table, expected) in [
            ("ledger_entries", 3),
            ("ledger_item_refs", 1),
            ("ledger_evidence", 1),
            ("ledger_entry_links", 1),
        ] {
            let count: i64 = conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, expected, "{table} must survive the v3 rebuild");
        }
        assert_eq!(super::foreign_key_violations(&conn).unwrap(), 0);

        // Verbatim, not re-defaulted: this is the failure v2's shape invites.
        let (via, touched): (String, i64) = conn
            .query_row(
                "SELECT enrolled_via, touched FROM ledger_entries
                 WHERE entry_id = 'le_aaaaaaaaaaaa'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(via, "manual");
        assert_eq!(touched, 1);
        let untracked_via: Option<String> = conn
            .query_row(
                "SELECT untracked_via FROM ledger_entries WHERE entry_id = 'le_cccccccccccc'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(untracked_via.as_deref(), Some("override"));

        for index in [
            "idx_ledger_item_refs_live",
            "idx_ledger_entries_state",
            "idx_ledger_entries_project",
            "idx_ledger_entries_match",
            "idx_ledger_entries_mention",
        ] {
            assert!(index_exists(&conn, index), "{index} should exist");
        }
        assert!(table_exists(&conn, "ledger_note_overrides"));
    }

    #[test]
    fn v3_admits_category_as_a_provenance_and_still_closes_the_set() {
        let conn = migrated();

        conn.execute(
            "INSERT INTO ledger_entries
                 (entry_id, state, direction, owner, description, owner_norm,
                  description_norm, project, created_at, updated_at, last_mention,
                  enrolled_via)
             VALUES ('le_111111111111', 'open', 'mine', 'You', 'x', 'you', 'x', 'Ops',
                     '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z',
                     'category')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ledger_entries
                 (entry_id, state, direction, owner, description, owner_norm,
                  description_norm, project, created_at, updated_at, last_mention,
                  untracked_via)
             VALUES ('le_222222222222', 'untracked', 'theirs', 'Priya', 'x', 'priya', 'x', 'Ops',
                     '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z',
                     'category')",
            [],
        )
        .unwrap();

        // Still closed sets, one value wider.
        for (column, value) in [("enrolled_via", "'genre'"), ("untracked_via", "'genre'")] {
            let err = conn.execute(
                &format!(
                    "INSERT INTO ledger_entries
                         (entry_id, state, direction, owner, description, owner_norm,
                          description_norm, project, created_at, updated_at, last_mention,
                          {column})
                     VALUES ('le_333333333333', 'untracked', 'theirs', 'Priya', 'x', 'priya', 'x',
                             'Ops', '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z',
                             '2026-08-01T00:00:00Z', {value})"
                ),
                [],
            );
            assert!(err.is_err(), "{column} must stay a closed set");
        }
    }

    #[test]
    fn v2_enforces_the_untracked_provenance_invariants() {
        let conn = migrated();

        // An untrack with no provenance is unrepresentable, mirroring closures.
        let err = conn.execute(
            "INSERT INTO ledger_entries
                 (entry_id, state, direction, owner, description, owner_norm,
                  description_norm, project, created_at, updated_at, last_mention)
             VALUES ('le_dddddddddddd', 'untracked', 'theirs', 'Priya', 'x', 'priya', 'x', 'Ops',
                     '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z')",
            [],
        );
        assert!(err.is_err(), "untracked without untracked_via is rejected");

        // So is provenance on an entry that is not untracked.
        let err = conn.execute(
            "INSERT INTO ledger_entries
                 (entry_id, state, direction, owner, description, owner_norm,
                  description_norm, project, created_at, updated_at, last_mention, untracked_via)
             VALUES ('le_eeeeeeeeeeee', 'open', 'theirs', 'Priya', 'x', 'priya', 'x', 'Ops',
                     '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z',
                     'manual')",
            [],
        );
        assert!(err.is_err(), "untracked_via on an open entry is rejected");

        // The new state itself is admitted, with its provenance.
        conn.execute(
            "INSERT INTO ledger_entries
                 (entry_id, state, direction, owner, description, owner_norm,
                  description_norm, project, created_at, updated_at, last_mention, untracked_via)
             VALUES ('le_ffffffffffff', 'untracked', 'theirs', 'Priya', 'x', 'priya', 'x', 'Ops',
                     '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z',
                     'override')",
            [],
        )
        .unwrap();

        // Enrollment provenance and the touched flag are closed sets.
        for (column, value) in [("enrolled_via", "'whenever'"), ("touched", "2")] {
            let err = conn.execute(
                &format!(
                    "UPDATE ledger_entries SET {column} = {value} WHERE entry_id = 'le_ffffffffffff'"
                ),
                [],
            );
            assert!(err.is_err(), "{column} must reject {value}");
        }
    }

    #[test]
    fn note_overrides_are_one_per_note_and_a_closed_mode_set() {
        let conn = migrated();
        conn.execute(
            "INSERT INTO ledger_note_overrides (note_id, project, mode, set_at)
             VALUES ('n_a1b2c3', 'Briarwood Golf', 'context_only', '2026-08-18T09:00:00Z')",
            [],
        )
        .unwrap();

        let err = conn.execute(
            "INSERT INTO ledger_note_overrides (note_id, project, mode, set_at)
             VALUES ('n_a1b2c3', 'Ops', 'tracked', '2026-08-18T10:00:00Z')",
            [],
        );
        assert!(err.is_err(), "a note has at most one override");

        let err = conn.execute(
            "INSERT INTO ledger_note_overrides (note_id, project, mode, set_at)
             VALUES ('n_d4e5f6', 'Ops', 'sometimes', '2026-08-18T10:00:00Z')",
            [],
        );
        assert!(err.is_err(), "mode is a closed set");
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
