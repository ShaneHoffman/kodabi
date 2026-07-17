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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// Display title (derived; not a frontmatter field).
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
    fn note_type_round_trips_through_its_string() {
        for ty in [NoteType::Meeting, NoteType::Note, NoteType::Chat] {
            assert_eq!(ty.as_str().parse::<NoteType>().unwrap(), ty);
        }
        assert_eq!(NoteType::Meeting.as_str(), "meeting");
        assert!("transcript".parse::<NoteType>().is_err());
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
