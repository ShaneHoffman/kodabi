//! The commitment ledger — durable state, evidence, and lifecycle over the
//! action items the distill pass already extracts.
//!
//! **This database is not a cache.** Unlike [`crate::index`], which is a
//! rebuildable derivation of the Markdown corpus and may be nuked at any time,
//! the ledger holds judgements a human made that exist nowhere else: a waiver, a
//! snooze, a closure with its provenance, the link that says two meetings named
//! one commitment. Deleting `ledger.db` loses those, so it lives beside
//! `settings.toml` in the config dir rather than inside the index, its
//! migrations must carry data forward (never drop-and-recreate), and every
//! change is mirrored into a per-project YAML snapshot in the vault
//! ([`snapshot`]) so "back up the vault" stays the whole backup story.
//!
//! **The note's checkbox stays the source of truth for done/not-done.** Nothing
//! here stores `done`, and nothing here derives it: a `- [x]` flip is invisible
//! to the ledger by design. What is stored is only what Markdown cannot carry —
//! identity across edits, evidence from outside the vault, and the states a
//! checkbox has no spelling for.
//!
//! ## Identity, and why entries have their own ids
//!
//! An extracted item's `a_` id is a hash of the *parsed* line
//! ([`crate::meeting::ActionItemFact`]), so it survives a checkbox flip but is
//! re-minted the moment the owner, description, or due date is edited — and the
//! per-line occurrence counter means deleting one of two identical lines hands
//! its id to the survivor. An `a_` id is therefore a **re-linkable reference**,
//! never a durable key. Entries carry their own random `le_` id and reference
//! items through [`ledger_item_refs`](migrations), whose rows retire rather than
//! disappear. [`sync`] is where a re-minted id is re-attached to its entry.

mod migrations;
pub mod snapshot;
mod store;
mod sync;

pub use snapshot::{ProjectSnapshot, RestoreReport, LEDGER_SNAPSHOT_FILE, LEDGER_SNAPSHOT_VERSION};
pub use store::{EntryDetail, EntryFilter, EntryLink, Evidence, ItemRef, LedgerEntry};
pub use sync::{NoteSync, SyncOutcome};

use std::collections::BTreeSet;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

/// Filename of the ledger database, resolved against the app config dir (the
/// shell's `sandbox::config_dir`, so `KODABI_SANDBOX` relocates it with the rest
/// of the config state).
pub const LEDGER_DB_FILE: &str = "ledger.db";

/// Base36 alphabet for minted ids (mirrors [`crate::note`]'s).
const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Random suffix length for a minted id. Longer than a note id's 8 because the
/// ledger mints ids far more often (one per commitment *and* one per evidence
/// claim) and, unlike notes, has no filesystem collision check behind it.
const ID_RANDOM_LEN: usize = 12;

/// Minimum random-suffix length a parsed id may carry, so a future shortening
/// never invalidates ids already in the field.
const ID_MIN_RANDOM_LEN: usize = 6;

/// Errors from opening, migrating, querying, or snapshotting the ledger.
#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    /// Any failure surfaced by SQLite (open, migrate, query, constraint).
    #[error("ledger database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A snapshot file could not be read, written, or renamed.
    #[error("ledger snapshot I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// A snapshot file's YAML could not be parsed or rendered.
    #[error("ledger snapshot YAML error at {path}: {source}")]
    Yaml {
        path: PathBuf,
        #[source]
        source: serde_yaml_ng::Error,
    },
    /// No entry with this id.
    #[error("no ledger entry {entry_id:?}")]
    EntryNotFound { entry_id: String },
    /// A lifecycle transition the state machine forbids (see
    /// [`store::LedgerEntry`]'s transition table).
    #[error("illegal ledger transition {from} -> {to} for {entry_id:?}")]
    IllegalTransition {
        entry_id: String,
        from: EntryState,
        to: EntryState,
    },
    /// A caller-supplied field failed validation before it reached SQLite.
    #[error("invalid {field}: {detail}")]
    InvalidField { field: &'static str, detail: String },
    /// The OS entropy source was unavailable while minting an id.
    #[error("OS RNG unavailable: {0}")]
    Rng(String),
}

/// `Result` specialised to [`LedgerError`].
pub type Result<T> = std::result::Result<T, LedgerError>;

/// Builds an [`LedgerError::InvalidField`].
pub(crate) fn invalid_field(field: &'static str, detail: impl Into<String>) -> LedgerError {
    LedgerError::InvalidField {
        field,
        detail: detail.into(),
    }
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Where an entry sits in its lifecycle.
///
/// Deliberately *not* a done/not-done axis: the note's checkbox owns that. These
/// are the states a checkbox cannot spell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryState {
    /// Live and tracked.
    Open,
    /// The entry lost its source line (edited beyond recognition, or the note
    /// was deleted) and needs a human to say what happened.
    NeedsReview,
    /// Resolved, with provenance in [`LedgerEntry::closed_via`]. A stronger
    /// claim than a checked box: it survives the line being edited away.
    Closed,
    /// Replaced by another entry; set only by [`Ledger::link_entries`] so the
    /// state and the link can never drift apart.
    Superseded,
    /// Deliberately not going to happen, and that is fine.
    Waived,
    /// Out of sight until [`LedgerEntry::snoozed_until`]. Expiry is evaluated at
    /// read time; nothing writes on the day it lapses.
    Snoozed,
}

impl EntryState {
    /// The stored spelling, shared by SQLite's `CHECK` and the YAML snapshot.
    pub fn as_str(self) -> &'static str {
        match self {
            EntryState::Open => "open",
            EntryState::NeedsReview => "needs_review",
            EntryState::Closed => "closed",
            EntryState::Superseded => "superseded",
            EntryState::Waived => "waived",
            EntryState::Snoozed => "snoozed",
        }
    }

    /// Parses the stored spelling.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "open" => Ok(EntryState::Open),
            "needs_review" => Ok(EntryState::NeedsReview),
            "closed" => Ok(EntryState::Closed),
            "superseded" => Ok(EntryState::Superseded),
            "waived" => Ok(EntryState::Waived),
            "snoozed" => Ok(EntryState::Snoozed),
            other => Err(invalid_field(
                "state",
                format!("unknown entry state {other:?}"),
            )),
        }
    }

    /// Whether this state is terminal — reachable back to `Open` only through
    /// [`Ledger::reopen`] or [`Ledger::unlink_entries`], never by sync.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            EntryState::Closed | EntryState::Superseded | EntryState::Waived
        )
    }
}

impl fmt::Display for EntryState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which way a commitment points. Both directions are first-class: "waiting on
/// them" is the half a checkbox list normally loses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// The local user took it on.
    Mine,
    /// Someone else did.
    Theirs,
    /// The line named no owner.
    Unassigned,
}

impl Direction {
    /// Derives the direction from the grammar's owner string.
    ///
    /// `"You"` is the distill prompt's convention for the local user
    /// ([`crate::distill`]), and [`crate::distill::UNASSIGNED_OWNER`] its
    /// sentinel for an unattributed line. Everything else is a named other.
    /// Case-insensitive: the owner is free text and only the renderer
    /// capitalizes it.
    pub fn from_owner(owner: &str) -> Direction {
        let owner = owner.trim();
        if owner.eq_ignore_ascii_case("you") {
            Direction::Mine
        } else if owner.eq_ignore_ascii_case(crate::distill::UNASSIGNED_OWNER) {
            Direction::Unassigned
        } else {
            Direction::Theirs
        }
    }

    /// The stored spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Mine => "mine",
            Direction::Theirs => "theirs",
            Direction::Unassigned => "unassigned",
        }
    }

    /// Parses the stored spelling.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "mine" => Ok(Direction::Mine),
            "theirs" => Ok(Direction::Theirs),
            "unassigned" => Ok(Direction::Unassigned),
            other => Err(invalid_field(
                "direction",
                format!("unknown direction {other:?}"),
            )),
        }
    }
}

/// How a closure was reached. Source-agnostic by design: GitHub and conversation
/// are simply the first two evidence providers, and adding one (ADO, Linear) is
/// a new variant plus a migration, not a new model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosedVia {
    /// A human said so.
    Manual,
    /// Inferred from a distilled conversation.
    Conversation,
    /// Observed in a GitHub artifact (a merged PR, a closed issue).
    Github,
}

impl ClosedVia {
    /// The stored spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            ClosedVia::Manual => "manual",
            ClosedVia::Conversation => "conversation",
            ClosedVia::Github => "github",
        }
    }

    /// Parses the stored spelling.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "manual" => Ok(ClosedVia::Manual),
            "conversation" => Ok(ClosedVia::Conversation),
            "github" => Ok(ClosedVia::Github),
            other => Err(invalid_field(
                "closed_via",
                format!("unknown closure provenance {other:?}"),
            )),
        }
    }
}

/// Where an evidence claim came from. Mirrors [`ClosedVia`] deliberately: a
/// closure's provenance is the source of the claim that closed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    Manual,
    Conversation,
    Github,
}

impl EvidenceSource {
    /// The stored spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            EvidenceSource::Manual => "manual",
            EvidenceSource::Conversation => "conversation",
            EvidenceSource::Github => "github",
        }
    }

    /// Parses the stored spelling.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "manual" => Ok(EvidenceSource::Manual),
            "conversation" => Ok(EvidenceSource::Conversation),
            "github" => Ok(EvidenceSource::Github),
            other => Err(invalid_field(
                "source",
                format!("unknown evidence source {other:?}"),
            )),
        }
    }

    /// The closure provenance this source implies, for a caller closing an entry
    /// on the strength of one claim.
    pub fn as_closed_via(self) -> ClosedVia {
        match self {
            EvidenceSource::Manual => ClosedVia::Manual,
            EvidenceSource::Conversation => ClosedVia::Conversation,
            EvidenceSource::Github => ClosedVia::Github,
        }
    }
}

/// How one entry relates to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    /// The commitment changed into a different one.
    Supersedes,
    /// The same commitment, re-stated later; the two are merged by hand.
    Refreshes,
}

impl LinkKind {
    /// The stored spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            LinkKind::Supersedes => "supersedes",
            LinkKind::Refreshes => "refreshes",
        }
    }

    /// Parses the stored spelling.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "supersedes" => Ok(LinkKind::Supersedes),
            "refreshes" => Ok(LinkKind::Refreshes),
            other => Err(invalid_field(
                "kind",
                format!("unknown link kind {other:?}"),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Id minting
// ---------------------------------------------------------------------------

/// Mints a random id with `prefix`, e.g. `le_9k2m4p7q1r3s`.
///
/// **Random, not a content hash** — which is the whole point of the ledger. The
/// `a_` item ids are hashes, so they churn exactly when durability matters most
/// (someone edits the line). An entry's identity must outlive its text.
///
/// Rejection-sampled so every base36 symbol is equally likely (mirrors
/// [`crate::note::NoteId::generate`]).
pub(crate) fn mint_id(prefix: &str) -> Result<String> {
    const REJECT_THRESHOLD: u8 = (256 / ALPHABET.len() * ALPHABET.len()) as u8;

    let mut random = String::with_capacity(ID_RANDOM_LEN);
    let mut buf = [0u8; ID_RANDOM_LEN];
    while random.len() < ID_RANDOM_LEN {
        getrandom::getrandom(&mut buf).map_err(|err| LedgerError::Rng(err.to_string()))?;
        for &byte in &buf {
            if byte < REJECT_THRESHOLD {
                random.push(ALPHABET[(byte % ALPHABET.len() as u8) as usize] as char);
                if random.len() == ID_RANDOM_LEN {
                    break;
                }
            }
        }
    }
    Ok(format!("{prefix}{random}"))
}

/// Whether `id` is a well-formed minted id with `prefix` — used when restoring a
/// snapshot written by another machine, where the ids are inputs rather than
/// something this process minted.
pub(crate) fn is_minted_id(id: &str, prefix: &str) -> bool {
    match id.strip_prefix(prefix) {
        Some(rest) => {
            rest.len() >= ID_MIN_RANDOM_LEN && rest.bytes().all(|b| ALPHABET.contains(&b))
        }
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Ledger
// ---------------------------------------------------------------------------

/// A handle to the commitment ledger, owning its connection.
///
/// Opening runs any pending migrations, so a returned `Ledger` is always at the
/// current schema version.
///
/// Timestamps are always **supplied by the caller** as RFC 3339 UTC with a `Z`
/// suffix and seconds precision (`.claude/rules/utc-timestamps.md`). kodabi-core
/// never reads the clock, matching the index's `today`-is-an-argument doctrine,
/// which is what makes every operation here deterministically testable.
pub struct Ledger {
    conn: Connection,
    /// Project slugs whose `_ledger.yml` no longer matches the database.
    ///
    /// In-memory and process-lifetime: the database is the truth and the
    /// snapshot is a backup, so losing this set to a crash costs only backup
    /// freshness. The shell drains it on a debounce and at exit
    /// ([`Ledger::flush_snapshots`]).
    dirty: BTreeSet<String>,
}

impl Ledger {
    /// Opens (creating if absent) the ledger database at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    /// Opens a fresh in-memory ledger — for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    /// Configures connection pragmas and migrates to the current schema.
    ///
    /// `foreign_keys` is load-bearing here rather than incidental: an entry's
    /// refs, evidence, and links cascade from it, so a delete that left them
    /// behind would resurrect as orphan rows in the next snapshot.
    fn init(mut conn: Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
        migrations::apply(&mut conn)?;
        Ok(Self {
            conn,
            dirty: BTreeSet::new(),
        })
    }

    /// Marks `project`'s snapshot as stale.
    pub(crate) fn mark_dirty(&mut self, project: &str) {
        if !project.is_empty() {
            self.dirty.insert(project.to_string());
        }
    }

    /// The project slugs whose snapshots are behind the database.
    pub fn dirty_projects(&self) -> Vec<String> {
        self.dirty.iter().cloned().collect()
    }

    /// Marks one project's snapshot as current again.
    pub(crate) fn clear_dirty(&mut self, project: &str) {
        self.dirty.remove(project);
    }

    /// Marks every snapshot current — the state after a restore, where the
    /// database was built *from* the files and rewriting them would be churn.
    pub(crate) fn clear_all_dirty(&mut self) {
        self.dirty.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn open_in_memory_is_ready_to_use() {
        let ledger = Ledger::open_in_memory().unwrap();
        let count: i64 = ledger
            .conn
            .query_row("SELECT count(*) FROM ledger_entries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn open_creates_the_file_and_survives_reopen() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(LEDGER_DB_FILE);
        {
            let ledger = Ledger::open(&path).unwrap();
            ledger
                .conn
                .execute("PRAGMA user_version", [])
                .ok()
                .or(Some(0));
        }
        assert!(path.is_file());
        // Reopening runs no migration and keeps the version.
        let ledger = Ledger::open(&path).unwrap();
        let version: i64 = ledger
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, migrations::CURRENT_VERSION);
    }

    #[test]
    fn minted_ids_are_well_formed_and_distinct() {
        let first = mint_id("le_").unwrap();
        let second = mint_id("le_").unwrap();
        assert_ne!(first, second);
        assert!(is_minted_id(&first, "le_"), "{first} should be well formed");
        assert_eq!(first.strip_prefix("le_").unwrap().len(), ID_RANDOM_LEN);
        assert!(!is_minted_id(&first, "ev_"));
        assert!(!is_minted_id("le_UPPER123456", "le_"));
        assert!(!is_minted_id("le_abc", "le_"), "too short");
    }

    #[test]
    fn direction_reads_the_grammar_conventions() {
        assert_eq!(Direction::from_owner("You"), Direction::Mine);
        assert_eq!(Direction::from_owner("you"), Direction::Mine);
        assert_eq!(
            Direction::from_owner(crate::distill::UNASSIGNED_OWNER),
            Direction::Unassigned
        );
        assert_eq!(Direction::from_owner("Priya"), Direction::Theirs);
        // "Yourself" is a named other, not the local user.
        assert_eq!(Direction::from_owner("Yourself"), Direction::Theirs);
    }

    #[test]
    fn entry_state_round_trips_and_flags_terminal() {
        for state in [
            EntryState::Open,
            EntryState::NeedsReview,
            EntryState::Closed,
            EntryState::Superseded,
            EntryState::Waived,
            EntryState::Snoozed,
        ] {
            assert_eq!(EntryState::parse(state.as_str()).unwrap(), state);
        }
        assert!(EntryState::Closed.is_terminal());
        assert!(EntryState::Superseded.is_terminal());
        assert!(EntryState::Waived.is_terminal());
        assert!(!EntryState::Open.is_terminal());
        assert!(!EntryState::NeedsReview.is_terminal());
        assert!(!EntryState::Snoozed.is_terminal());
        assert!(EntryState::parse("done").is_err());
    }
}
