//! The cross-note action-item query behind the `list_outstanding_items` MCP
//! tool (`docs/MCP_TOOL_SURFACE.md` §4) — the read the Phase 3 milestone
//! ("what's outstanding on <project>?") actually runs.
//!
//! `note_action_items` already holds every item parsed out of a distilled note
//! body (see [`crate::meeting`] and `migration_0003_meeting_facts`); this module
//! is purely the read side, joining those rows to `notes` for the project scope
//! and the source `NoteRef`.
//!
//! The join carries **no `notes.type` predicate**, deliberately: a commitment is
//! a commitment whether it was made in a meeting, in a chat, or written by hand,
//! so all three surface here together (FOUNDING_DOC §3.6, "chats are documents
//! too"). Which types produce rows at all is decided once, upstream, by
//! [`crate::meeting::derives_facts`] — not a second time here.
//!
//! # Status is filtered in SQL and rendered in Rust
//!
//! `open`/`overdue` are derived, never stored, so both the `WHERE` clause and
//! the emitted [`ActionItemStatus`] must agree about the same item on the same
//! day — otherwise a page could serve a row the filter excluded, or the
//! cross-page `summary` could disagree with the items beneath it. The SQL
//! predicates below are written to be provably equivalent to
//! [`ActionItemStatus::derive`]: the `GLOB` guard accepts exactly the shapes
//! `NaiveDate::parse_from_str("%Y-%m-%d")` accepts, so a malformed due date is
//! "not dated" on both sides rather than sorting or comparing as garbage.
//!
//! # No index was added for this
//!
//! The sort key is a computed expression, so no plain B-tree could serve the
//! `ORDER BY` anyway; the join side is already covered (`note_action_items`'
//! primary key leads with `note_id`, and `notes.id` is `UNIQUE`). At personal-KB
//! scale — order 10^4 rows for thousands of meetings — the scan-and-sort is
//! sub-millisecond, and a migration is permanent while the index is a
//! rebuildable cache (FOUNDING_DOC §3.6), so the cheap decision is reversible
//! and the expensive one is not. Revisit if rows pass ~100k or a page measures
//! past ~50 ms, when a profile can justify the index's exact shape.

use chrono::NaiveDate;
use rusqlite::params_from_iter;
use rusqlite::types::Value;

use super::note::{ActionItemStatus, NoteRef};
use super::scope::ProjectScope;
use super::search::PageInfo;
use super::{IndexError, NoteIndex, Result};

/// Bounds mirroring the `list_outstanding_items` `inputSchema`.
const MIN_LIMIT: u32 = 1;
const MAX_LIMIT: u32 = 100;
const DEFAULT_LIMIT: u32 = 50;

fn default_true() -> bool {
    true
}

fn default_limit() -> u32 {
    DEFAULT_LIMIT
}

/// The schema default: the not-done set.
fn default_status() -> Vec<ActionItemStatus> {
    vec![ActionItemStatus::Open, ActionItemStatus::Overdue]
}

/// The `list_outstanding_items` inputs — mirrors the tool's `inputSchema`
/// field-for-field, including its defaults, so an MCP wrapper can deserialize
/// tool arguments straight into it. `deny_unknown_fields` matches the schema's
/// `additionalProperties: false`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutstandingParams {
    /// Restrict to items whose source note is in this project. The reserved
    /// value `Inbox` (any casing) matches unfiled notes.
    #[serde(default)]
    pub project: Option<String>,
    /// When `project` is set, also include nested sub-projects. Default `true`.
    #[serde(default = "default_true")]
    pub include_descendants: bool,
    /// Restrict to this owner. Matched case-insensitively (ASCII), because the
    /// distill grammar stores capitalized owners (`You`, `Priya`,
    /// `Unassigned`) while a caller naturally writes `you`. `Unassigned` is how
    /// you ask for items nobody was attributed.
    #[serde(default)]
    pub owner: Option<String>,
    /// Statuses to include. Defaults to the not-done set (open + overdue). An
    /// explicitly empty list matches nothing.
    #[serde(default = "default_status")]
    pub status: Vec<ActionItemStatus>,
    /// Only items with a due date strictly before this date. Items with no due
    /// date are excluded when this is set.
    #[serde(default)]
    pub due_before: Option<String>,
    /// Restrict to items extracted from this specific meeting note.
    #[serde(default)]
    pub source_note_id: Option<String>,
    /// Max items per page, clamped to `1..=100`. Default `50`.
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// Opaque pagination token from a prior response's `page.next_cursor`.
    #[serde(default)]
    pub cursor: Option<String>,
}

impl Default for OutstandingParams {
    fn default() -> Self {
        Self {
            project: None,
            include_descendants: true,
            owner: None,
            status: default_status(),
            due_before: None,
            source_note_id: None,
            limit: DEFAULT_LIMIT,
            cursor: None,
        }
    }
}

/// One outstanding item — the `ActionItem` `$def` of
/// `docs/MCP_TOOL_SURFACE.md`, field names and order matching it so the MCP
/// wrapper serializes it straight out. `extracted_date` is omitted rather than
/// null when absent (the `$def` does not require it), matching `get_note`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OutstandingItem {
    pub id: String,
    pub description: String,
    pub owner: String,
    pub due_date: Option<String>,
    pub status: ActionItemStatus,
    pub source: NoteRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extracted_date: Option<String>,
}

/// Totals across **all** pages of the same filter, so a caller that pages can
/// still report "12 outstanding" without walking. `done` is 0 unless `done` was
/// among the requested statuses, exactly as the schema describes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct OutstandingSummary {
    pub open: u32,
    pub overdue: u32,
    pub done: u32,
}

/// The `list_outstanding_items` output — the page's `items`, the cross-page
/// `summary`, and the pagination envelope.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OutstandingResults {
    pub items: Vec<OutstandingItem>,
    pub summary: OutstandingSummary,
    pub page: PageInfo,
}

/// A due date this server will parse — exactly the values
/// [`ActionItemStatus::derive`] accepts, so SQL and Rust classify every row
/// identically. Unreachable today (the distill grammar only writes valid dates),
/// but it makes the equivalence a property of this query rather than of a
/// distant writer.
///
/// Both halves are load-bearing. The `GLOB` pins the literal `YYYY-MM-DD` shape,
/// which `date()` alone would not (it also reads `2026-7-4` and a `T`-suffixed
/// timestamp). The `date(x) = x` round-trip then rejects a shape-valid non-day:
/// SQLite normalizes `2026-02-30` to `2026-03-02` and returns NULL for
/// `2026-13-01`, so neither compares equal to the stored text — matching
/// `NaiveDate::parse_from_str`, which rejects both. Without it, `2026-02-30`
/// would be *overdue* to this filter and *open* to the rendered status.
const DATED: &str = "(note_action_items.due_date IS NOT NULL \
     AND note_action_items.due_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]' \
     AND date(note_action_items.due_date) = note_action_items.due_date)";

/// The per-status SQL predicates. `?` binds `today`.
fn status_predicate(status: ActionItemStatus) -> String {
    match status {
        ActionItemStatus::Done => "note_action_items.done = 1".to_string(),
        ActionItemStatus::Overdue => {
            format!("(note_action_items.done = 0 AND {DATED} AND note_action_items.due_date < ?)")
        }
        ActionItemStatus::Open => format!(
            "(note_action_items.done = 0 AND (NOT {DATED} OR note_action_items.due_date >= ?))"
        ),
    }
}

/// Whether a status predicate binds `today`.
fn status_binds_today(status: ActionItemStatus) -> bool {
    !matches!(status, ActionItemStatus::Done)
}

/// The two computed sort columns, projected so the `ORDER BY`, the keyset
/// predicate, and the cursor all name the same expressions.
///
/// SQLite sorts `NULL` first under `ASC`, but the spec orders undated items
/// **last**, so the bucket needs an explicit leading term. `due_key` collapses
/// the whole undated bucket to `''` rather than leaving a malformed date to sort
/// among real ones.
fn sort_columns() -> String {
    format!(
        "CASE WHEN {DATED} THEN 0 ELSE 1 END AS undated, \
         CASE WHEN {DATED} THEN note_action_items.due_date ELSE '' END AS due_key"
    )
}

/// A decoded pagination cursor: the boundary row's sort key plus its primary
/// key.
struct OutstandingCursor {
    undated: i64,
    due_key: String,
    note_id: String,
    seq: i64,
}

/// The filter clauses and their bound values, shared by the page query and the
/// summary so the two can never disagree about what matched.
struct Filters {
    clauses: Vec<String>,
    values: Vec<Value>,
}

impl NoteIndex {
    /// Lists not-done (or explicitly filtered) action items across notes,
    /// ordered by due date ascending with undated items last.
    ///
    /// `today` is supplied by the caller rather than read here: kodabi-core
    /// never reads the clock (`.claude/rules/utc-timestamps.md`), and the shell
    /// passes the device's local date because due dates are local calendar
    /// dates.
    ///
    /// A filter that matches nothing is an empty page, never an error — absence
    /// is a valid answer (the cross-cutting contract's not-found-vs-empty rule).
    pub fn list_outstanding_items(
        &self,
        params: &OutstandingParams,
        today: NaiveDate,
    ) -> Result<OutstandingResults> {
        // Validate the cursor before anything else, so a malformed token is
        // rejected the same way whatever the index holds.
        let cursor = params
            .cursor
            .as_deref()
            .map(decode_outstanding_cursor)
            .transpose()?;
        let limit = params.limit.clamp(MIN_LIMIT, MAX_LIMIT) as usize;
        let today_date = today;
        let today = today.format("%Y-%m-%d").to_string();

        // An explicitly empty status list is a request for nothing, not a
        // request for everything.
        if params.status.is_empty() {
            return Ok(OutstandingResults {
                items: Vec::new(),
                summary: OutstandingSummary::default(),
                page: PageInfo {
                    has_more: false,
                    next_cursor: None,
                    total_estimate: Some(0),
                },
            });
        }

        let filters = build_filters(params, &today)?;
        let summary = self.outstanding_summary_with(&filters, &today)?;
        let items = self.outstanding_page(&filters, cursor.as_ref(), limit, today_date)?;

        // One extra row tells us whether another page exists without a count.
        let has_more = items.len() > limit;
        let mut items = items;
        items.truncate(limit);
        let next_cursor = has_more
            .then(|| items.last().map(|(_, key)| encode_outstanding_cursor(key)))
            .flatten();

        let items = items
            .into_iter()
            .map(|(item, _)| item)
            .collect::<Vec<OutstandingItem>>();

        Ok(OutstandingResults {
            items,
            summary,
            page: PageInfo {
                has_more,
                next_cursor,
                // Exact, unlike `search_notes`: there is no truncated candidate
                // pool here, so the summary counted every match.
                total_estimate: Some(u64::from(summary.open + summary.overdue + summary.done)),
            },
        })
    }

    /// The cross-page totals for a filter, without fetching any items. Used on
    /// its own by `get_project_context`, whose `counts` must stay true even when
    /// its `outstanding` section is switched off or capped at zero.
    pub fn outstanding_summary(
        &self,
        params: &OutstandingParams,
        today: NaiveDate,
    ) -> Result<OutstandingSummary> {
        if params.status.is_empty() {
            return Ok(OutstandingSummary::default());
        }
        let today = today.format("%Y-%m-%d").to_string();
        let filters = build_filters(params, &today)?;
        self.outstanding_summary_with(&filters, &today)
    }

    fn outstanding_summary_with(
        &self,
        filters: &Filters,
        today: &str,
    ) -> Result<OutstandingSummary> {
        // `SUM` over zero rows is NULL, hence the COALESCE.
        let sql = format!(
            "SELECT \
               COALESCE(SUM(CASE WHEN {open} THEN 1 ELSE 0 END), 0), \
               COALESCE(SUM(CASE WHEN {overdue} THEN 1 ELSE 0 END), 0), \
               COALESCE(SUM(CASE WHEN {done} THEN 1 ELSE 0 END), 0) \
             FROM note_action_items \
             JOIN notes ON notes.id = note_action_items.note_id \
             WHERE 1 = 1{filters}",
            open = status_predicate(ActionItemStatus::Open),
            overdue = status_predicate(ActionItemStatus::Overdue),
            done = status_predicate(ActionItemStatus::Done),
            filters = filters.where_and(),
        );

        // The three CASE arms bind `today` twice (open, overdue) before the
        // filter values, in SQL order.
        let mut values = vec![
            Value::Text(today.to_string()),
            Value::Text(today.to_string()),
        ];
        values.extend(filters.values.iter().cloned());

        let (open, overdue, done) = self.conn.query_row(&sql, params_from_iter(values), |row| {
            Ok((
                row.get::<_, i64>(0)? as u32,
                row.get::<_, i64>(1)? as u32,
                row.get::<_, i64>(2)? as u32,
            ))
        })?;
        Ok(OutstandingSummary {
            open,
            overdue,
            done,
        })
    }

    /// Fetches up to `limit + 1` rows past the cursor, each paired with its
    /// sort key so the caller can mint the next cursor.
    fn outstanding_page(
        &self,
        filters: &Filters,
        cursor: Option<&OutstandingCursor>,
        limit: usize,
        today: NaiveDate,
    ) -> Result<Vec<(OutstandingItem, CursorKey)>> {
        let mut values = filters.values.clone();

        // Keyset written lexicographically rather than as a row-value
        // comparison: `search.rs` records that row values need a newer SQLite
        // than the bundled floor guarantees.
        let keyset = match cursor {
            Some(cursor) => {
                values.push(Value::Integer(cursor.undated));
                values.push(Value::Integer(cursor.undated));
                values.push(Value::Text(cursor.due_key.clone()));
                values.push(Value::Integer(cursor.undated));
                values.push(Value::Text(cursor.due_key.clone()));
                values.push(Value::Text(cursor.note_id.clone()));
                values.push(Value::Integer(cursor.undated));
                values.push(Value::Text(cursor.due_key.clone()));
                values.push(Value::Text(cursor.note_id.clone()));
                values.push(Value::Integer(cursor.seq));
                " WHERE (undated > ? \
                    OR (undated = ? AND due_key > ?) \
                    OR (undated = ? AND due_key = ? AND note_id > ?) \
                    OR (undated = ? AND due_key = ? AND note_id = ? AND seq > ?))"
            }
            None => "",
        };

        let sql = format!(
            "SELECT item_id, description, owner, due_date, done, extracted_date, \
                    note_id, note_path, undated, due_key, seq \
             FROM ( \
               SELECT note_action_items.item_id AS item_id, \
                      note_action_items.description AS description, \
                      note_action_items.owner AS owner, \
                      note_action_items.due_date AS due_date, \
                      note_action_items.done AS done, \
                      note_action_items.extracted_date AS extracted_date, \
                      note_action_items.note_id AS note_id, \
                      note_action_items.seq AS seq, \
                      notes.path AS note_path, \
                      {sort} \
               FROM note_action_items \
               JOIN notes ON notes.id = note_action_items.note_id \
               WHERE 1 = 1{filters} \
             ) AS scoped{keyset} \
             ORDER BY undated, due_key, note_id, seq \
             LIMIT {fetch}",
            sort = sort_columns(),
            filters = filters.where_and(),
            keyset = keyset,
            fetch = limit + 1,
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(values), |row| {
                let done: i64 = row.get("done")?;
                let due_date: Option<String> = row.get("due_date")?;
                Ok((
                    OutstandingItem {
                        id: row.get("item_id")?,
                        description: row.get("description")?,
                        owner: row.get("owner")?,
                        // Re-derived in Rust against the same `today` the SQL
                        // filter bound; `DATED` guarantees the two agree on
                        // every row, so a served item always carries a status
                        // the caller actually asked for.
                        status: ActionItemStatus::derive(done != 0, due_date.as_deref(), today),
                        due_date,
                        source: NoteRef {
                            id: row.get("note_id")?,
                            path: row.get("note_path")?,
                        },
                        extracted_date: row.get("extracted_date")?,
                    },
                    CursorKey {
                        undated: row.get("undated")?,
                        due_key: row.get("due_key")?,
                        note_id: row.get("note_id")?,
                        seq: row.get("seq")?,
                    },
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

/// The boundary row's ordering identity.
struct CursorKey {
    undated: i64,
    due_key: String,
    note_id: String,
    seq: i64,
}

impl Filters {
    /// The clauses as ` AND (…)` suffixes to splice after `WHERE 1 = 1`.
    fn where_and(&self) -> String {
        self.clauses.iter().map(|c| format!(" AND {c}")).collect()
    }
}

/// Builds the filter clauses and bound values, in SQL order.
fn build_filters(params: &OutstandingParams, today: &str) -> Result<Filters> {
    let mut clauses = Vec::new();
    let mut values = Vec::new();

    let scope = ProjectScope::resolve(params.project.as_deref(), params.include_descendants);
    if let Some((clause, mut scope_values)) = scope.predicate() {
        clauses.push(clause);
        values.append(&mut scope_values);
    }

    if let Some(owner) = &params.owner {
        clauses.push("note_action_items.owner = ? COLLATE NOCASE".to_string());
        values.push(Value::Text(owner.trim().to_string()));
    }

    // The requested statuses, OR-ed. Deduplicated first: `uniqueItems` is
    // unenforced on the wire, and a repeat would bind `today` twice for no gain.
    let mut statuses: Vec<ActionItemStatus> = Vec::new();
    for status in &params.status {
        if !statuses.contains(status) {
            statuses.push(*status);
        }
    }
    let predicates: Vec<String> = statuses.iter().map(|s| status_predicate(*s)).collect();
    clauses.push(format!("({})", predicates.join(" OR ")));
    for status in &statuses {
        if status_binds_today(*status) {
            values.push(Value::Text(today.to_string()));
        }
    }

    if let Some(due_before) = &params.due_before {
        let due_before = parse_iso_date(due_before)?;
        // `DATED` is what makes "items with no due date are excluded when this
        // is set" fall out, for malformed dates as well as absent ones.
        clauses.push(format!("({DATED} AND note_action_items.due_date < ?)"));
        values.push(Value::Text(due_before));
    }

    if let Some(source_note_id) = &params.source_note_id {
        clauses.push("note_action_items.note_id = ?".to_string());
        values.push(Value::Text(source_note_id.clone()));
    }

    Ok(Filters { clauses, values })
}

/// Validates an `IsoDate` bound and returns its canonical `YYYY-MM-DD` form.
fn parse_iso_date(raw: &str) -> Result<String> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .map(|date| date.format("%Y-%m-%d").to_string())
        .map_err(|source| IndexError::Date {
            value: raw.to_string(),
            source,
        })
}

/// Encodes a cursor naming the boundary row: its two sort-key columns plus the
/// `(note_id, seq)` primary key that totally orders ties.
///
/// The tiebreak is deliberately the primary key rather than `item_id`:
/// `item_id` is an FNV hash of the note id plus the line's content, so two
/// identical action lines could collide and stall the walk.
fn encode_outstanding_cursor(key: &CursorKey) -> String {
    // `due_key` is a fixed-shape `YYYY-MM-DD` or empty and a note id matches
    // `^n_[0-9a-z]{6,}$`, so neither can contain `:` — the fields stay
    // unambiguous with a plain separator.
    format!(
        "v1:{}:{}:{}:{}",
        key.undated, key.seq, key.due_key, key.note_id
    )
}

/// Decodes a cursor, rejecting anything [`encode_outstanding_cursor`] did not
/// produce.
fn decode_outstanding_cursor(raw: &str) -> Result<OutstandingCursor> {
    let bad = || IndexError::Cursor {
        value: raw.to_string(),
    };
    let rest = raw.strip_prefix("v1:").ok_or_else(bad)?;
    let mut parts = rest.splitn(4, ':');
    let undated: i64 = parts.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
    let seq: i64 = parts.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
    let due_key = parts.next().ok_or_else(bad)?.to_string();
    let note_id = parts.next().ok_or_else(bad)?.to_string();
    if !(0..=1).contains(&undated) || seq < 0 || note_id.is_empty() {
        return Err(bad());
    }
    Ok(OutstandingCursor {
        undated,
        due_key,
        note_id,
        seq,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{IndexedNote, NoteType};
    use crate::meeting::{ActionItemFact, MeetingFacts};

    /// A fixed "today" so no test depends on the wall clock.
    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 7, 24).unwrap()
    }

    fn item(id: &str, owner: &str, due: Option<&str>, done: bool) -> ActionItemFact {
        ActionItemFact {
            id: id.to_string(),
            description: format!("do {id}"),
            owner: owner.to_string(),
            due_date: due.map(str::to_string),
            done,
            extracted_date: Some("2026-07-10".to_string()),
        }
    }

    fn meeting(id: &str, project: Option<&str>, items: Vec<ActionItemFact>) -> IndexedNote {
        IndexedNote {
            id: id.to_string(),
            path: format!("{}/{id}.md", project.unwrap_or("Inbox")),
            title: format!("Title {id}"),
            note_type: NoteType::Meeting,
            project: project.map(str::to_string),
            date: "2026-07-10".to_string(),
            tags: vec![],
            source: "transcript".to_string(),
            confidence: Some(0.9),
            category: None,
            category_confidence: None,
            tracking: None,
            body: format!("body of {id}"),
            meeting: Some(MeetingFacts {
                duration_seconds: Some(600),
                speaker_count: Some(2),
                decisions: vec![],
                action_items: items,
            }),
        }
    }

    /// A chat note carrying items. Both session scalars are `None`, as
    /// `meeting::derive_meeting_facts` guarantees for a chat.
    fn chat(id: &str, project: Option<&str>, items: Vec<ActionItemFact>) -> IndexedNote {
        IndexedNote {
            note_type: NoteType::Chat,
            source: format!("chats/{id}.jsonl"),
            meeting: Some(MeetingFacts {
                duration_seconds: None,
                speaker_count: None,
                decisions: vec![],
                action_items: items,
            }),
            ..meeting(id, project, vec![])
        }
    }

    /// Three notes across a project subtree, the Inbox, and a sibling project.
    fn seeded() -> NoteIndex {
        let mut index = NoteIndex::open_in_memory().unwrap();
        index
            .upsert_note(&meeting(
                "n_growth",
                Some("Growth"),
                vec![
                    // Overdue: due before `today()`.
                    item("a_past01", "You", Some("2026-07-01"), false),
                    // Open: due after `today()`.
                    item("a_soon01", "Priya", Some("2026-08-01"), false),
                    // Open: undated, so never overdue.
                    item("a_none01", "Unassigned", None, false),
                    // Done: excluded from the default status set.
                    item("a_done01", "You", Some("2026-07-01"), true),
                ],
            ))
            .unwrap();
        index
            .upsert_note(&meeting(
                "n_nested",
                Some("Growth/Q3"),
                vec![item("a_nest01", "You", Some("2026-07-02"), false)],
            ))
            .unwrap();
        index
            .upsert_note(&meeting(
                "n_sibling",
                Some("Growthx"),
                vec![item("a_sib001", "You", Some("2026-07-03"), false)],
            ))
            .unwrap();
        index
            .upsert_note(&meeting(
                "n_inbox0",
                None,
                vec![item("a_inbox1", "You", Some("2026-07-04"), false)],
            ))
            .unwrap();
        index
    }

    fn ids(results: &OutstandingResults) -> Vec<&str> {
        results.items.iter().map(|i| i.id.as_str()).collect()
    }

    #[test]
    fn an_empty_index_is_an_empty_page_not_an_error() {
        let index = NoteIndex::open_in_memory().unwrap();
        let results = index
            .list_outstanding_items(&OutstandingParams::default(), today())
            .unwrap();

        assert!(results.items.is_empty());
        assert_eq!(results.summary, OutstandingSummary::default());
        assert!(!results.page.has_more);
        assert_eq!(results.page.total_estimate, Some(0));
    }

    #[test]
    fn the_default_status_set_is_not_done_ordered_by_due_date_undated_last() {
        let index = seeded();
        let results = index
            .list_outstanding_items(&OutstandingParams::default(), today())
            .unwrap();

        // Dated ascending across every project, then the undated item last.
        // `a_done01` is absent: done is not in the default set.
        assert_eq!(
            ids(&results),
            [
                "a_past01", // 2026-07-01
                "a_nest01", // 2026-07-02
                "a_sib001", // 2026-07-03
                "a_inbox1", // 2026-07-04
                "a_soon01", // 2026-08-01
                "a_none01", // undated
            ]
        );
        // Four dated items sit before 2026-07-24; the future-dated and the
        // undated one are open. The done item is counted only when requested.
        assert_eq!(results.summary.open, 2);
        assert_eq!(results.summary.overdue, 4);
        assert_eq!(results.summary.done, 0);
        assert_eq!(results.page.total_estimate, Some(6));
    }

    /// The join deliberately carries no `notes.type` predicate: a commitment made
    /// in a chat is as real as one made in a meeting (FOUNDING_DOC §3.6). This is
    /// the guard against someone "tidying" the read side by adding one.
    #[test]
    fn chat_sourced_items_surface_beside_meeting_sourced_ones() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        index
            .upsert_note(&meeting(
                "n_mtg001",
                Some("Growth"),
                vec![item("a_mtg001", "You", Some("2026-07-02"), false)],
            ))
            .unwrap();
        index
            .upsert_note(&chat(
                "n_cht001",
                Some("Growth"),
                vec![item("a_cht001", "Jane", Some("2026-07-01"), false)],
            ))
            .unwrap();

        let results = index
            .list_outstanding_items(&OutstandingParams::default(), today())
            .unwrap();

        // Interleaved by due date, not segregated by note type: the chat's
        // earlier item leads.
        assert_eq!(ids(&results), ["a_cht001", "a_mtg001"]);
        assert_eq!(results.items[0].source.id, "n_cht001");
        assert_eq!(results.summary.overdue, 2);
    }

    #[test]
    fn status_is_derived_against_today_on_both_sides_of_the_boundary() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        index
            .upsert_note(&meeting(
                "n_edge00",
                Some("Growth"),
                vec![item("a_edge01", "You", Some("2026-07-15"), false)],
            ))
            .unwrap();

        // On the due date itself the item is open, not yet overdue.
        let on_the_day = NaiveDate::from_ymd_opt(2026, 7, 15).unwrap();
        let results = index
            .list_outstanding_items(&OutstandingParams::default(), on_the_day)
            .unwrap();
        assert_eq!(results.items[0].status, ActionItemStatus::Open);
        assert_eq!(results.summary.open, 1);
        assert_eq!(results.summary.overdue, 0);

        // One day later it is overdue — and the SQL filter agrees with the
        // rendered status, which is the invariant that matters.
        let next_day = NaiveDate::from_ymd_opt(2026, 7, 16).unwrap();
        let results = index
            .list_outstanding_items(&OutstandingParams::default(), next_day)
            .unwrap();
        assert_eq!(results.items[0].status, ActionItemStatus::Overdue);
        assert_eq!(results.summary.open, 0);
        assert_eq!(results.summary.overdue, 1);

        // Asking for only `overdue` finds it on the later day and not the earlier.
        let overdue_only = OutstandingParams {
            status: vec![ActionItemStatus::Overdue],
            ..OutstandingParams::default()
        };
        assert!(index
            .list_outstanding_items(&overdue_only, on_the_day)
            .unwrap()
            .items
            .is_empty());
        assert_eq!(
            index
                .list_outstanding_items(&overdue_only, next_day)
                .unwrap()
                .items
                .len(),
            1
        );
    }

    #[test]
    fn a_malformed_due_date_is_treated_as_undated_by_sql_and_rust_alike() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        index
            .upsert_note(&meeting(
                "n_bad000",
                Some("Growth"),
                vec![item("a_bad001", "You", Some("07/15/2026"), false)],
            ))
            .unwrap();

        let results = index
            .list_outstanding_items(&OutstandingParams::default(), today())
            .unwrap();

        // The `DATED` GLOB guard keeps an unparseable date out of the overdue
        // bucket, matching `ActionItemStatus::derive`'s parse-or-open rule.
        assert_eq!(results.items.len(), 1);
        assert_eq!(results.items[0].status, ActionItemStatus::Open);
        assert_eq!(results.summary.overdue, 0);
        assert_eq!(results.summary.open, 1);

        // And `due_before` excludes it, like any other undated item.
        let filtered = index
            .list_outstanding_items(
                &OutstandingParams {
                    due_before: Some("2030-01-01".to_string()),
                    ..OutstandingParams::default()
                },
                today(),
            )
            .unwrap();
        assert!(filtered.items.is_empty());
    }

    #[test]
    fn a_calendar_invalid_due_date_is_undated_on_both_sides_too() {
        // `2026-02-30` has the right *shape* but is not a real day, so
        // `NaiveDate::parse_from_str` rejects it. A shape-only SQL guard would
        // call it dated and — being lexically before `today()` — overdue, which
        // would serve an item under `status: ["overdue"]` that renders `open`.
        let mut index = NoteIndex::open_in_memory().unwrap();
        index
            .upsert_note(&meeting(
                "n_feb300",
                Some("Growth"),
                vec![item("a_feb301", "You", Some("2026-02-30"), false)],
            ))
            .unwrap();

        let results = index
            .list_outstanding_items(&OutstandingParams::default(), today())
            .unwrap();
        assert_eq!(results.items.len(), 1);
        assert_eq!(results.items[0].status, ActionItemStatus::Open);
        assert_eq!(results.summary.open, 1);
        assert_eq!(results.summary.overdue, 0);

        // The invariant that matters: a status filter never serves a row whose
        // rendered status the caller did not ask for.
        let overdue_only = OutstandingParams {
            status: vec![ActionItemStatus::Overdue],
            ..OutstandingParams::default()
        };
        assert!(index
            .list_outstanding_items(&overdue_only, today())
            .unwrap()
            .items
            .is_empty());

        // And it drops out of `due_before`, like any other undated item.
        let filtered = index
            .list_outstanding_items(
                &OutstandingParams {
                    due_before: Some("2030-01-01".to_string()),
                    ..OutstandingParams::default()
                },
                today(),
            )
            .unwrap();
        assert!(filtered.items.is_empty());
    }

    #[test]
    fn a_cursor_walk_serves_every_item_exactly_once() {
        let index = seeded();

        let mut seen = Vec::new();
        let mut params = OutstandingParams {
            limit: 2,
            ..OutstandingParams::default()
        };
        loop {
            let page = index.list_outstanding_items(&params, today()).unwrap();
            // The cross-page totals are the same on every page.
            assert_eq!(page.summary.open + page.summary.overdue, 6);
            assert_eq!(page.page.total_estimate, Some(6));
            seen.extend(page.items.iter().map(|i| i.id.clone()));
            match page.page.next_cursor {
                Some(cursor) => params.cursor = Some(cursor),
                None => {
                    assert!(!page.page.has_more);
                    break;
                }
            }
        }

        assert_eq!(
            seen,
            ["a_past01", "a_nest01", "a_sib001", "a_inbox1", "a_soon01", "a_none01"]
        );
    }

    #[test]
    fn the_cursor_orders_ties_by_the_primary_key_not_the_hashed_item_id() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        // Four items sharing one due date, so only the `(note_id, seq)`
        // tiebreak separates them.
        index
            .upsert_note(&meeting(
                "n_tied00",
                Some("Growth"),
                vec![
                    item("a_tie001", "You", Some("2026-07-01"), false),
                    item("a_tie002", "You", Some("2026-07-01"), false),
                    item("a_tie003", "You", Some("2026-07-01"), false),
                    item("a_tie004", "You", Some("2026-07-01"), false),
                ],
            ))
            .unwrap();

        let mut seen = Vec::new();
        let mut params = OutstandingParams {
            limit: 1,
            ..OutstandingParams::default()
        };
        loop {
            let page = index.list_outstanding_items(&params, today()).unwrap();
            seen.extend(page.items.iter().map(|i| i.id.clone()));
            match page.page.next_cursor {
                Some(cursor) => params.cursor = Some(cursor),
                None => break,
            }
        }

        assert_eq!(seen, ["a_tie001", "a_tie002", "a_tie003", "a_tie004"]);
    }

    #[test]
    fn a_tampered_cursor_is_rejected() {
        let index = seeded();
        for bad in [
            "",
            "v1:",
            "v2:0:0:2026-07-01:n_growth",
            "v1:0:0:2026-07-01",
            "v1:9:0:2026-07-01:n_growth", // undated out of range
            "v1:0:-1:2026-07-01:n_growth",
            "v1:x:0:2026-07-01:n_growth",
            "v1:0:0:2026-07-01:", // empty note id
        ] {
            let params = OutstandingParams {
                cursor: Some(bad.to_string()),
                ..OutstandingParams::default()
            };
            assert!(
                matches!(
                    index.list_outstanding_items(&params, today()),
                    Err(IndexError::Cursor { .. })
                ),
                "cursor {bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn project_scope_honors_the_subtree_and_never_matches_a_sibling() {
        let index = seeded();

        // Subtree: Growth plus Growth/Q3, never the sibling `Growthx`.
        let subtree = index
            .list_outstanding_items(
                &OutstandingParams {
                    project: Some("Growth".to_string()),
                    include_descendants: true,
                    ..OutstandingParams::default()
                },
                today(),
            )
            .unwrap();
        assert_eq!(
            ids(&subtree),
            ["a_past01", "a_nest01", "a_soon01", "a_none01"]
        );

        // Exact: Growth only.
        let exact = index
            .list_outstanding_items(
                &OutstandingParams {
                    project: Some("Growth".to_string()),
                    include_descendants: false,
                    ..OutstandingParams::default()
                },
                today(),
            )
            .unwrap();
        assert_eq!(ids(&exact), ["a_past01", "a_soon01", "a_none01"]);
    }

    #[test]
    fn the_inbox_sentinel_selects_unfiled_notes_items() {
        let index = seeded();
        let results = index
            .list_outstanding_items(
                &OutstandingParams {
                    project: Some("inbox".to_string()),
                    ..OutstandingParams::default()
                },
                today(),
            )
            .unwrap();

        assert_eq!(ids(&results), ["a_inbox1"]);
        assert_eq!(results.items[0].source.id, "n_inbox0");
    }

    #[test]
    fn an_unknown_project_is_an_empty_page_not_an_error() {
        let index = seeded();
        let results = index
            .list_outstanding_items(
                &OutstandingParams {
                    project: Some("Nope".to_string()),
                    ..OutstandingParams::default()
                },
                today(),
            )
            .unwrap();

        assert!(results.items.is_empty());
        assert!(!results.page.has_more);
        assert_eq!(results.summary, OutstandingSummary::default());
    }

    #[test]
    fn owner_matches_case_insensitively() {
        let index = seeded();
        for spelling in ["You", "you", "YOU", " you "] {
            let results = index
                .list_outstanding_items(
                    &OutstandingParams {
                        owner: Some(spelling.to_string()),
                        ..OutstandingParams::default()
                    },
                    today(),
                )
                .unwrap();
            // The stored owner is `You`; the done item is still excluded.
            assert_eq!(
                ids(&results),
                ["a_past01", "a_nest01", "a_sib001", "a_inbox1"],
                "{spelling:?}"
            );
        }

        // `Unassigned` is how you ask for items nobody was attributed.
        let unowned = index
            .list_outstanding_items(
                &OutstandingParams {
                    owner: Some("unassigned".to_string()),
                    ..OutstandingParams::default()
                },
                today(),
            )
            .unwrap();
        assert_eq!(ids(&unowned), ["a_none01"]);
    }

    #[test]
    fn due_before_is_strict_and_drops_undated_items() {
        let index = seeded();
        let results = index
            .list_outstanding_items(
                &OutstandingParams {
                    due_before: Some("2026-07-03".to_string()),
                    ..OutstandingParams::default()
                },
                today(),
            )
            .unwrap();

        // 07-01 and 07-02 qualify; 07-03 does not (strictly before), and the
        // undated item is excluded outright.
        assert_eq!(ids(&results), ["a_past01", "a_nest01"]);
    }

    #[test]
    fn a_malformed_due_before_is_a_date_error() {
        let index = seeded();
        let params = OutstandingParams {
            due_before: Some("next tuesday".to_string()),
            ..OutstandingParams::default()
        };
        assert!(matches!(
            index.list_outstanding_items(&params, today()),
            Err(IndexError::Date { .. })
        ));
    }

    #[test]
    fn source_note_id_narrows_to_one_meeting() {
        let index = seeded();
        let results = index
            .list_outstanding_items(
                &OutstandingParams {
                    source_note_id: Some("n_nested".to_string()),
                    ..OutstandingParams::default()
                },
                today(),
            )
            .unwrap();

        assert_eq!(ids(&results), ["a_nest01"]);
        assert_eq!(results.items[0].source.id, "n_nested");
        assert_eq!(results.items[0].source.path, "Growth/Q3/n_nested.md");
    }

    #[test]
    fn requesting_done_reports_it_in_the_summary() {
        let index = seeded();
        let results = index
            .list_outstanding_items(
                &OutstandingParams {
                    status: vec![ActionItemStatus::Done],
                    ..OutstandingParams::default()
                },
                today(),
            )
            .unwrap();

        assert_eq!(ids(&results), ["a_done01"]);
        assert_eq!(results.summary.done, 1);
        assert_eq!(results.summary.open, 0);
        assert_eq!(results.summary.overdue, 0);
    }

    #[test]
    fn an_explicitly_empty_status_list_matches_nothing() {
        let index = seeded();
        let results = index
            .list_outstanding_items(
                &OutstandingParams {
                    status: vec![],
                    ..OutstandingParams::default()
                },
                today(),
            )
            .unwrap();

        // Not "every status" — the caller asked for none.
        assert!(results.items.is_empty());
        assert_eq!(results.summary, OutstandingSummary::default());
    }

    #[test]
    fn a_repeated_status_does_not_double_count() {
        let index = seeded();
        let results = index
            .list_outstanding_items(
                &OutstandingParams {
                    status: vec![
                        ActionItemStatus::Open,
                        ActionItemStatus::Open,
                        ActionItemStatus::Overdue,
                    ],
                    ..OutstandingParams::default()
                },
                today(),
            )
            .unwrap();

        assert_eq!(results.items.len(), 6);
        assert_eq!(results.summary.open, 2);
        assert_eq!(results.summary.overdue, 4);
    }

    #[test]
    fn an_item_whose_note_was_deleted_is_not_served() {
        let mut index = seeded();
        index.delete_note("n_growth").unwrap();

        let results = index
            .list_outstanding_items(&OutstandingParams::default(), today())
            .unwrap();

        // `delete_note` clears the item rows alongside the note.
        assert_eq!(ids(&results), ["a_nest01", "a_sib001", "a_inbox1"]);
    }

    #[test]
    fn an_orphaned_item_row_is_dropped_by_the_join() {
        let index = seeded();
        // `note_action_items` carries no foreign key to `notes` (see
        // `migration_0003_meeting_facts`), so an orphan is representable if a
        // future writer ever misses a cleanup. The inner join must not surface
        // it — an item with no source note has no `NoteRef` to return.
        index
            .conn
            .execute(
                "INSERT INTO note_action_items \
                   (note_id, seq, item_id, description, owner, due_date, done, extracted_date) \
                 VALUES ('n_ghost0', 0, 'a_ghost1', 'haunt', 'You', '2026-07-01', 0, NULL)",
                [],
            )
            .unwrap();

        let results = index
            .list_outstanding_items(&OutstandingParams::default(), today())
            .unwrap();

        assert!(!ids(&results).contains(&"a_ghost1"));
        // And it is absent from the totals, not merely from the page.
        assert_eq!(results.summary.open + results.summary.overdue, 6);
    }

    #[test]
    fn the_summary_alone_skips_the_page_query() {
        let index = seeded();
        let summary = index
            .outstanding_summary(&OutstandingParams::default(), today())
            .unwrap();

        assert_eq!(summary.open, 2);
        assert_eq!(summary.overdue, 4);
        assert_eq!(summary.done, 0);
    }

    #[test]
    fn limit_is_clamped_to_the_schema_bounds() {
        let index = seeded();

        // 0 clamps up to 1 rather than paging forever on empty responses.
        let page = index
            .list_outstanding_items(
                &OutstandingParams {
                    limit: 0,
                    ..OutstandingParams::default()
                },
                today(),
            )
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert!(page.page.has_more);

        // An over-large limit clamps down but still serves everything here.
        let page = index
            .list_outstanding_items(
                &OutstandingParams {
                    limit: u32::MAX,
                    ..OutstandingParams::default()
                },
                today(),
            )
            .unwrap();
        assert_eq!(page.items.len(), 6);
        assert!(!page.page.has_more);
    }

    #[test]
    fn an_item_serializes_to_the_action_item_def() {
        let index = seeded();
        let results = index
            .list_outstanding_items(
                &OutstandingParams {
                    source_note_id: Some("n_nested".to_string()),
                    ..OutstandingParams::default()
                },
                today(),
            )
            .unwrap();

        let value = serde_json::to_value(&results.items[0]).unwrap();
        assert_eq!(value["id"], "a_nest01");
        assert_eq!(value["description"], "do a_nest01");
        assert_eq!(value["owner"], "You");
        assert_eq!(value["due_date"], "2026-07-02");
        assert_eq!(value["status"], "overdue");
        assert_eq!(value["source"]["id"], "n_nested");
        assert_eq!(value["source"]["path"], "Growth/Q3/n_nested.md");
        assert_eq!(value["extracted_date"], "2026-07-10");

        // An absent `extracted_date` is omitted, not null — the `$def` does not
        // require it, and this matches `get_note`.
        let mut bare = results.items[0].clone();
        bare.extracted_date = None;
        let value = serde_json::to_value(&bare).unwrap();
        assert!(value.get("extracted_date").is_none());
    }
}
