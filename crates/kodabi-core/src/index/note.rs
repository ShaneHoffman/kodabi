//! The note record the index ingests and returns, plus the `date` → UTC
//! normalization the schema requires.
//!
//! Fields mirror `docs/FRONTMATTER_SCHEMA.md` (what the watcher/rebuild parses
//! out of frontmatter) and `docs/MCP_TOOL_SURFACE.md`'s `NoteSummary` (what the
//! query surface returns), so an indexed row is a complete note summary without
//! re-reading the source file.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};

use super::{IndexError, Result};

/// A note's `type` frontmatter field — a closed enum (FRONTMATTER_SCHEMA
/// "type"). Stored in the index as its lowercase string, guarded by a `CHECK`
/// constraint in the schema.
///
/// The serde impls render/parse the same lowercase spellings as [`as_str`], so
/// the `search_notes` DTOs (`SearchParams.type`, `SearchHit.type`) match the
/// `NoteType` `$def` in `docs/MCP_TOOL_SURFACE.md` on the wire.
///
/// [`as_str`]: NoteType::as_str
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteType {
    Meeting,
    Note,
    Chat,
}

impl NoteType {
    /// The canonical lowercase spelling stored in the `type` column.
    pub fn as_str(&self) -> &'static str {
        match self {
            NoteType::Meeting => "meeting",
            NoteType::Note => "note",
            NoteType::Chat => "chat",
        }
    }
}

impl fmt::Display for NoteType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for NoteType {
    type Err = UnknownNoteType;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "meeting" => Ok(NoteType::Meeting),
            "note" => Ok(NoteType::Note),
            "chat" => Ok(NoteType::Chat),
            other => Err(UnknownNoteType(other.to_string())),
        }
    }
}

impl From<crate::note::NoteType> for NoteType {
    fn from(ty: crate::note::NoteType) -> Self {
        match ty {
            crate::note::NoteType::Meeting => NoteType::Meeting,
            crate::note::NoteType::Note => NoteType::Note,
            crate::note::NoteType::Chat => NoteType::Chat,
        }
    }
}

/// A string that isn't one of the three allowed [`NoteType`] values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownNoteType(pub String);

impl fmt::Display for UnknownNoteType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown note type {:?} (expected meeting|note|chat)",
            self.0
        )
    }
}

impl std::error::Error for UnknownNoteType {}

impl ToSql for NoteType {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.as_str()))
    }
}

impl FromSql for NoteType {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        value
            .as_str()?
            .parse()
            .map_err(|err: UnknownNoteType| FromSqlError::Other(Box::new(err)))
    }
}

/// A note to insert or update in the index — the parsed frontmatter (`id`,
/// `type`, `project`, `date`, `tags`, `source`, `confidence`) plus the derived
/// fields the query surface needs (`path`, `title`, `body`).
///
/// `project` is `None` for the `Inbox`/unfiled sentinel: callers (the Phase 2
/// watcher) map the frontmatter `Inbox` value to `None`, matching how
/// `NoteSummary` represents an unfiled note as `null`.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedNote {
    /// Stable note id, `^n_[0-9a-z]{6,}$`. The logical key; never the path.
    pub id: String,
    /// Current KB-relative path. Informational — changes when the note moves.
    pub path: String,
    /// Effective display title: the note's stored frontmatter `title`, or the
    /// de-slugged filename stem for a legacy/hand-made note without one.
    pub title: String,
    pub note_type: NoteType,
    /// Owning project, or `None` when unfiled (Inbox).
    pub project: Option<String>,
    /// Frontmatter `date`, stored verbatim (offset preserved).
    pub date: String,
    /// Frontmatter tags; empty when the key is absent.
    pub tags: Vec<String>,
    pub source: String,
    /// Routing confidence 0.0–1.0, or `None` when hand-filed/imported.
    pub confidence: Option<f64>,
    /// Note body (frontmatter stripped) — the full-text search content.
    pub body: String,
    /// Structured meeting facts (decisions, action items, duration, speaker
    /// count) for a meeting note, or `None` for a non-meeting note (or when the
    /// caller has not derived them). Populated by the write path via
    /// [`crate::meeting::meeting_facts_for`]; [`from_note`](IndexedNote::from_note)
    /// leaves it `None`, since deriving it reads the session file the note's
    /// `source` points at, which the plain frontmatter/body mapping does not do.
    pub meeting: Option<crate::meeting::MeetingFacts>,
}

impl IndexedNote {
    /// Builds an index record from a parsed [`Note`](crate::note::Note), its
    /// KB-relative path, and a display title.
    ///
    /// The caller passes the effective title (via [`crate::vault::effective_title`]):
    /// the note's stored frontmatter `title`, or the de-slugged filename stem as
    /// a fallback. `path` should already be KB-relative with forward slashes. The
    /// `Inbox` sentinel project collapses to `None`, matching how `NoteSummary`
    /// represents an unfiled note.
    pub fn from_note(note: &crate::note::Note, title: &str, path: &str) -> Self {
        let project = note.routing.project();
        IndexedNote {
            id: note.id.as_str().to_string(),
            path: path.to_string(),
            title: title.to_string(),
            note_type: note.note_type.into(),
            project: (project != crate::note::INBOX).then(|| project.to_string()),
            date: note.date.clone(),
            tags: note.tags.iter().map(|t| t.as_str().to_string()).collect(),
            source: note.source.as_yaml().to_string(),
            confidence: note.routing.confidence(),
            body: note.body.clone(),
            meeting: None,
        }
    }
}

/// The meeting-scalar row read back for a meeting note: the two session-derived
/// counts (nullable) and the ordered decisions. The action items are a separate
/// read ([`NoteRow`]'s query surface exposes `get_action_items`), keeping the
/// `search`/`notes_by_project` paths free of any meeting joins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingFactsRow {
    pub duration_seconds: Option<u32>,
    pub speaker_count: Option<u32>,
    pub decisions: Vec<String>,
}

/// One stored action item, read back in body order. Mirrors
/// [`crate::meeting::ActionItemFact`] (the derivation-side shape) the way
/// [`NoteRow`] mirrors [`IndexedNote`]: a distinct read type for the query
/// surface. `overdue` is not stored — it is derived server-side at read time
/// from `done` + `due_date`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionItemRow {
    pub id: String,
    pub description: String,
    pub owner: String,
    pub due_date: Option<String>,
    pub done: bool,
    pub extracted_date: Option<String>,
}

/// Note counts split by [`NoteType`] — the `notes_by_type` block of
/// `get_project_context`'s `counts`. Field names match that schema, so it
/// serializes straight out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct NoteTypeCounts {
    pub meeting: u32,
    pub note: u32,
    pub chat: u32,
}

impl NoteTypeCounts {
    /// Total notes across every type.
    pub fn total(&self) -> u32 {
        self.meeting + self.note + self.chat
    }
}

/// A minimal stable reference to a note — the `NoteRef` `$def` of
/// `docs/MCP_TOOL_SURFACE.md`. Carried by a cross-note read (an action item's
/// `source`) so the caller can follow the row back to `get_note` without a
/// second lookup. `path` is informational and changes on move; `id` is the
/// handle.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct NoteRef {
    pub id: String,
    pub path: String,
}

/// An action item's read-time status — the `ActionItemStatus` `$def` of
/// `docs/MCP_TOOL_SURFACE.md`. The serde spellings match that enum
/// (`open | overdue | done`) so it both deserializes the `status` filter of
/// `list_outstanding_items` and serializes into an `ActionItem` on the wire.
///
/// Never stored: `note_action_items` persists only the `done` checkbox and the
/// `due_date`, and `Overdue` is derived against the caller's `today` so it stays
/// correct as the clock advances without a reindex.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionItemStatus {
    Open,
    Overdue,
    Done,
}

impl ActionItemStatus {
    /// A checked item is [`Done`]; otherwise it is [`Overdue`] once its due date
    /// is strictly before `today`, else [`Open`]. An item with no due date is
    /// never overdue.
    ///
    /// `today` is passed in rather than read here: kodabi-core never reads the
    /// clock (`.claude/rules/utc-timestamps.md`), and the shell supplies the
    /// device's local date because due dates are local calendar dates.
    ///
    /// [`Done`]: ActionItemStatus::Done
    /// [`Overdue`]: ActionItemStatus::Overdue
    /// [`Open`]: ActionItemStatus::Open
    pub fn derive(done: bool, due_date: Option<&str>, today: NaiveDate) -> Self {
        if done {
            return ActionItemStatus::Done;
        }
        match due_date.and_then(|due| NaiveDate::parse_from_str(due, "%Y-%m-%d").ok()) {
            Some(due) if due < today => ActionItemStatus::Overdue,
            _ => ActionItemStatus::Open,
        }
    }
}

/// A note row read back from the index. Carries both the verbatim `date` and
/// the derived `date_utc` ordering key (see [`normalize_date_to_utc`]).
#[derive(Debug, Clone, PartialEq)]
pub struct NoteRow {
    pub id: String,
    pub path: String,
    pub title: String,
    pub note_type: NoteType,
    pub project: Option<String>,
    /// Verbatim frontmatter `date` (the `date_raw` column).
    pub date: String,
    /// UTC-normalized ordering key (the `date_utc` column).
    pub date_utc: String,
    pub tags: Vec<String>,
    pub source: String,
    pub confidence: Option<f64>,
    pub body: String,
}

/// Collapses a frontmatter `date` — a full RFC 3339 timestamp with offset, or a
/// date-only `YYYY-MM-DD` — to a UTC `YYYY-MM-DDTHH:MM:SSZ` string.
///
/// Because every value ends on the same `Z` offset, a lexical `ORDER BY
/// date_utc` is also chronological. Raw frontmatter strings are not: a
/// `-07:00` wall clock sorts *before* a `+00:00` one that is actually earlier
/// in absolute time (FRONTMATTER_SCHEMA "date"). The verbatim string is kept
/// separately as `date_raw`, so nothing is lost.
pub fn normalize_date_to_utc(raw: &str) -> Result<String> {
    // A full timestamp with offset (`Z` or numeric) parses here; a date-only
    // value does not, and falls through to the date-only branch below.
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Ok(dt
            .with_timezone(&Utc)
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string());
    }

    let date = NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|source| IndexError::Date {
        value: raw.to_string(),
        source,
    })?;
    Ok(format!("{}T00:00:00Z", date.format("%Y-%m-%d")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_note_maps_fields_and_collapses_inbox_to_none() {
        use crate::note::{Note, NoteId, Routing, Source, Tag};

        let routed = Note::new(
            NoteId::parse("n_a1b2c3").unwrap(),
            crate::note::NoteType::Meeting,
            Routing::Routed {
                project: "Briarwood Golf".to_string(),
                confidence: 0.94,
            },
            "2026-07-09T14:00:00-07:00",
            vec![Tag::parse("budgeting").unwrap()],
            Source::parse("transcript").unwrap(),
            "the body",
        )
        .unwrap();
        let indexed =
            IndexedNote::from_note(&routed, "Weekly Sync", "Briarwood Golf/weekly-sync.md");
        assert_eq!(indexed.id, "n_a1b2c3");
        assert_eq!(indexed.title, "Weekly Sync");
        assert_eq!(indexed.path, "Briarwood Golf/weekly-sync.md");
        assert_eq!(indexed.note_type, NoteType::Meeting);
        assert_eq!(indexed.project.as_deref(), Some("Briarwood Golf"));
        assert_eq!(indexed.date, "2026-07-09T14:00:00-07:00");
        assert_eq!(indexed.tags, vec!["budgeting".to_string()]);
        assert_eq!(indexed.source, "transcript");
        assert_eq!(indexed.confidence, Some(0.94));
        assert_eq!(indexed.body, "the body");

        // An Inbox note collapses to a null project and keeps its score.
        let inbox = Note::new(
            NoteId::parse("n_inbox1").unwrap(),
            crate::note::NoteType::Note,
            Routing::Routed {
                project: crate::note::INBOX.to_string(),
                confidence: 0.30,
            },
            "2026-07-11",
            vec![],
            Source::parse("quick-capture").unwrap(),
            "",
        )
        .unwrap();
        let indexed = IndexedNote::from_note(&inbox, "idea", "Inbox/idea.md");
        assert_eq!(indexed.project, None);
        assert_eq!(indexed.confidence, Some(0.30));
    }

    #[test]
    fn note_type_round_trips_through_its_string() {
        for ty in [NoteType::Meeting, NoteType::Note, NoteType::Chat] {
            assert_eq!(ty.as_str().parse::<NoteType>().unwrap(), ty);
        }
        assert_eq!(NoteType::Meeting.as_str(), "meeting");
        assert!("transcript".parse::<NoteType>().is_err());
    }

    #[test]
    fn note_type_serde_uses_the_lowercase_spelling() {
        for ty in [NoteType::Meeting, NoteType::Note, NoteType::Chat] {
            let json = serde_json::to_string(&ty).unwrap();
            assert_eq!(json, format!("\"{}\"", ty.as_str()));
            assert_eq!(serde_json::from_str::<NoteType>(&json).unwrap(), ty);
        }
        assert!(serde_json::from_str::<NoteType>("\"transcript\"").is_err());
    }

    #[test]
    fn action_item_status_is_derived_against_today() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 24).unwrap();

        // A checked box is done whatever the due date says.
        assert_eq!(
            ActionItemStatus::derive(true, Some("2020-01-01"), today),
            ActionItemStatus::Done
        );
        assert_eq!(
            ActionItemStatus::derive(true, None, today),
            ActionItemStatus::Done
        );

        // Strictly before today is overdue; today itself is not yet.
        assert_eq!(
            ActionItemStatus::derive(false, Some("2026-07-23"), today),
            ActionItemStatus::Overdue
        );
        assert_eq!(
            ActionItemStatus::derive(false, Some("2026-07-24"), today),
            ActionItemStatus::Open
        );
        assert_eq!(
            ActionItemStatus::derive(false, Some("2026-07-25"), today),
            ActionItemStatus::Open
        );

        // An undated item is never overdue, however far the clock advances.
        assert_eq!(
            ActionItemStatus::derive(false, None, today),
            ActionItemStatus::Open
        );
        assert_eq!(
            ActionItemStatus::derive(false, None, NaiveDate::from_ymd_opt(2099, 1, 1).unwrap()),
            ActionItemStatus::Open
        );

        // An unparseable due date degrades to open rather than erroring.
        assert_eq!(
            ActionItemStatus::derive(false, Some("not-a-date"), today),
            ActionItemStatus::Open
        );
    }

    #[test]
    fn action_item_status_serde_matches_the_schema_spelling() {
        for (status, spelling) in [
            (ActionItemStatus::Open, "\"open\""),
            (ActionItemStatus::Overdue, "\"overdue\""),
            (ActionItemStatus::Done, "\"done\""),
        ] {
            assert_eq!(serde_json::to_string(&status).unwrap(), spelling);
            assert_eq!(
                serde_json::from_str::<ActionItemStatus>(spelling).unwrap(),
                status
            );
        }
        assert!(serde_json::from_str::<ActionItemStatus>("\"pending\"").is_err());
    }

    #[test]
    fn full_timestamp_is_converted_to_utc() {
        // 14:00 at -07:00 is 21:00Z.
        assert_eq!(
            normalize_date_to_utc("2026-07-09T14:00:00-07:00").unwrap(),
            "2026-07-09T21:00:00Z"
        );
        // A `Z` timestamp is preserved as-is.
        assert_eq!(
            normalize_date_to_utc("2026-07-11T14:03:00Z").unwrap(),
            "2026-07-11T14:03:00Z"
        );
    }

    #[test]
    fn date_only_normalizes_to_utc_midnight() {
        assert_eq!(
            normalize_date_to_utc("2026-07-11").unwrap(),
            "2026-07-11T00:00:00Z"
        );
    }

    #[test]
    fn normalization_fixes_the_cross_offset_sort_hazard() {
        // The FRONTMATTER_SCHEMA example: raw strings sort these the wrong way
        // (…T14…-07:00 < …T15…+00:00 lexically), but -07:00 14:00 is 21:00Z,
        // which is later than 15:00Z. After normalization the UTC keys order
        // them correctly.
        // Names describe wall-clock digits; the -07:00 one is the later instant.
        let earlier_digits = normalize_date_to_utc("2026-07-09T14:00:00-07:00").unwrap(); // 21:00Z
        let later_digits = normalize_date_to_utc("2026-07-09T15:00:00+00:00").unwrap(); // 15:00Z
        assert!(earlier_digits > later_digits);
    }

    #[test]
    fn a_non_date_string_is_an_error() {
        assert!(matches!(
            normalize_date_to_utc("not-a-date"),
            Err(IndexError::Date { .. })
        ));
    }
}
