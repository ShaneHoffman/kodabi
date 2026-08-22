//! The MCP-facing read over the commitment ledger: `list_commitments`.
//!
//! Three surfaces answer three different questions about the same words, and
//! keeping them distinct is the whole point of this module:
//!
//! - a note answers *what was said*,
//! - [`crate::index::NoteIndex::list_outstanding_items`] answers *what did they
//!   commit to* — raw extraction, every unchecked line in the vault,
//! - this answers *what am I tracking* — the ledger's working set, with the
//!   items it never enrolled, deliberately untracked, superseded or waived
//!   already filtered out.
//!
//! Chat used to have access only to the middle one, so "what's outstanding on
//! X?" could list commitments the Commitments view had settled — the flagship
//! answer contradicting the flagship view. This module is the third answer,
//! assembled through the *same* [`super::view::assemble`] join the desktop view
//! renders, so the two cannot drift apart: a difference would have to be a bug
//! in one shared function rather than a divergence between two.
//!
//! Shaped after [`crate::index::outstanding`] — same params/results/cursor
//! trio, same "an explicitly empty filter list matches nothing" rule, same
//! clock discipline (`today` and the aging thresholds arrive from the shell;
//! nothing here reads a clock or a settings file).

use std::collections::{BTreeSet, HashMap};

use chrono::NaiveDate;

use super::store::{EntryDetail, EntryFilter};
use super::view::{self, AgingConfig, Commitment, NoteContext};
use super::{Direction, EntryState, Ledger, LedgerError};
use crate::index::{NoteIndex, PageInfo};
use crate::note::INBOX;

/// Schema default for `limit`.
const DEFAULT_LIMIT: u32 = 50;
const MIN_LIMIT: u32 = 1;
const MAX_LIMIT: u32 = 100;

fn default_limit() -> u32 {
    DEFAULT_LIMIT
}

fn default_true() -> bool {
    true
}

/// The schema default for `state`: the live working set.
///
/// The same three states the Commitments view's own list shows
/// (`ledger_cmds::LIVE_STATES`), and the reason this tool exists — a caller who
/// asks nothing in particular gets what is actually being tracked, not the
/// audit trail.
fn default_states() -> Vec<EntryState> {
    vec![
        EntryState::Open,
        EntryState::NeedsReview,
        EntryState::Snoozed,
    ]
}

/// The `list_commitments` inputs — mirrors the tool's `inputSchema`
/// field-for-field, including its defaults, so the MCP wrapper can deserialize
/// tool arguments straight into it. `deny_unknown_fields` matches the schema's
/// `additionalProperties: false`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitmentsParams {
    /// Restrict to commitments filed under this project. The reserved value
    /// `Inbox` (any casing) matches unfiled ones.
    #[serde(default)]
    pub project: Option<String>,
    /// When `project` is set, also include nested sub-projects. Default `true`.
    #[serde(default = "default_true")]
    pub include_descendants: bool,
    /// Restrict to commitments the user owes (`mine`), is waiting on
    /// (`theirs`), or that nobody was attributed (`unassigned`).
    #[serde(default)]
    pub direction: Option<Direction>,
    /// Entry states to include. Defaults to the live set (open, needs_review,
    /// snoozed). An explicitly empty list matches nothing.
    #[serde(default = "default_states")]
    pub state: Vec<EntryState>,
    /// Restrict to commitments whose live source line is in this note.
    #[serde(default)]
    pub source_note_id: Option<String>,
    /// Max commitments per page, clamped to `1..=100`. Default `50`.
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// Opaque pagination token from a prior response's `page.next_cursor`.
    #[serde(default)]
    pub cursor: Option<String>,
}

impl Default for CommitmentsParams {
    fn default() -> Self {
        Self {
            project: None,
            include_descendants: true,
            direction: None,
            state: default_states(),
            source_note_id: None,
            limit: DEFAULT_LIMIT,
            cursor: None,
        }
    }
}

/// One commitment on the wire — the `Commitment` `$def` of
/// `docs/MCP_TOOL_SURFACE.md`.
///
/// Mirrors `ledger_cmds`' `CommitmentDto` field-for-field on purpose: the
/// desktop view and chat describe a commitment with the same words, so a reader
/// moving between them recognizes the same thing.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CommitmentWire {
    pub entry_id: String,
    pub state: EntryState,
    pub direction: Direction,
    /// The ledger's cached copy of who owes it and what it is — what survives
    /// the source line being edited away, and the fallback when `item` is null.
    pub owner: String,
    pub description: String,
    /// `null` for the Inbox sentinel, matching every other tool's projection.
    pub project: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_mention: String,
    pub last_evidence_check: Option<String>,
    /// How long this has gone untouched, derived against the caller's `today`
    /// and the user's own thresholds.
    pub tier: String,
    pub snoozed_until: Option<String>,
    /// A snooze whose day has arrived: still `snoozed`, but back in the work.
    pub snooze_lapsed: bool,
    pub closed_via: Option<String>,
    pub review_reason: Option<String>,
    pub untracked_via: Option<String>,
    /// The live source line, or `null` when the entry has no active ref or the
    /// index has no row for its note.
    pub item: Option<CommitmentItemWire>,
    /// The note the active ref points into, when the index knows it.
    pub source: Option<CommitmentSourceWire>,
    pub evidence: Vec<CommitmentEvidenceWire>,
}

/// The live source line behind a commitment: the note's current text and the
/// checkbox that owns done/not-done.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CommitmentItemWire {
    pub note_id: String,
    pub item_id: String,
    pub description: String,
    pub owner: String,
    pub due_date: Option<String>,
    pub done: bool,
    pub status: crate::index::ActionItemStatus,
}

/// Where a commitment's source line lives, for a click-through.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CommitmentSourceWire {
    pub note_id: String,
    pub title: String,
    pub project: Option<String>,
    pub path: String,
    pub category: Option<String>,
}

/// One claim that a commitment may already be settled.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CommitmentEvidenceWire {
    pub evidence_id: String,
    pub source: String,
    pub reference: Option<String>,
    pub confidence: f64,
    pub observed_at: String,
}

/// Totals across **all** pages of the same filter, so a caller that pages can
/// still say "9 tracked, 4 of them mine" without walking.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct CommitmentsSummary {
    pub total: u32,
    pub mine: u32,
    pub theirs: u32,
    pub unassigned: u32,
    /// Entries that lost their source line and want a person's eye.
    pub needs_review: u32,
    /// Snoozes whose day has arrived.
    pub snooze_lapsed: u32,
}

/// The `list_commitments` output — the page's `commitments`, the cross-page
/// `summary`, and the pagination envelope.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CommitmentsResults {
    pub commitments: Vec<CommitmentWire>,
    pub summary: CommitmentsSummary,
    pub page: PageInfo,
}

/// Why a commitments read could not be served.
#[derive(Debug, thiserror::Error)]
pub enum CommitmentsError {
    /// The pagination token was not one this module minted.
    #[error("invalid cursor: {value:?}")]
    Cursor { value: String },
    /// The ledger itself failed.
    #[error(transparent)]
    Ledger(#[from] LedgerError),
}

/// A decoded pagination cursor: the boundary row's sort key plus its primary
/// key.
struct CommitmentsCursor {
    last_mention: String,
    entry_id: String,
}

/// Lists the ledger's tracked commitments, joined to their live source lines.
///
/// `today` and `aging` are supplied by the caller for the same reason the rest
/// of kodabi-core takes them: nothing here reads the clock
/// (`.claude/rules/utc-timestamps.md`), and the aging thresholds live in a
/// settings file this crate does not own.
///
/// A filter that matches nothing is an empty page, never an error — absence is
/// a valid answer (the cross-cutting contract's not-found-vs-empty rule).
///
/// Paginated in Rust rather than in SQL, unlike the index's outstanding query.
/// The ledger holds one row per human commitment and
/// [`Ledger::list_details`] already fetches the whole filtered set in one pass
/// (it is what the desktop view does on every refresh), so sorting and slicing
/// here costs nothing and buys the summary its exact cross-page totals.
pub fn list_commitments(
    ledger: &Ledger,
    index: &NoteIndex,
    params: &CommitmentsParams,
    today: NaiveDate,
    aging: AgingConfig,
) -> Result<CommitmentsResults, CommitmentsError> {
    // Validate the cursor before touching the ledger, so a tampered token is
    // rejected the same way whatever the database holds.
    let cursor = params
        .cursor
        .as_deref()
        .map(decode_commitments_cursor)
        .transpose()?;
    let limit = params.limit.clamp(MIN_LIMIT, MAX_LIMIT) as usize;

    // An explicitly empty state list is a request for nothing, not a request
    // for everything — the rule `EntryFilter` and the outstanding query share.
    if params.state.is_empty() {
        return Ok(empty_page());
    }

    let details = ledger.list_details(&filter_for(params))?;
    if details.is_empty() {
        return Ok(empty_page());
    }

    let notes = note_contexts(index, &referenced_notes(&details));
    let mut commitments = view::assemble(details, &notes, today, aging);

    // Newest mention first: the ledger's timestamps are RFC 3339 UTC, so a
    // lexical compare is a chronological one. `entry_id` breaks ties, which is
    // what makes the keyset cursor below a total order rather than a guess.
    commitments.sort_by(|left, right| {
        right
            .detail
            .entry
            .last_mention
            .cmp(&left.detail.entry.last_mention)
            .then_with(|| left.detail.entry.entry_id.cmp(&right.detail.entry.entry_id))
    });

    let summary = summarize(&commitments);

    // Compared by key rather than by position, so a boundary entry deleted
    // between two pages can neither stall the walk nor repeat a row.
    if let Some(cursor) = cursor.as_ref() {
        commitments.retain(|commitment| after_cursor(commitment, cursor));
    }

    let has_more = commitments.len() > limit;
    commitments.truncate(limit);
    let next_cursor = has_more
        .then(|| {
            commitments.last().map(|commitment| {
                encode_commitments_cursor(
                    &commitment.detail.entry.last_mention,
                    &commitment.detail.entry.entry_id,
                )
            })
        })
        .flatten();

    Ok(CommitmentsResults {
        commitments: commitments.into_iter().map(commitment_wire).collect(),
        summary,
        page: PageInfo {
            has_more,
            next_cursor,
            // Exact: the summary counted every match, not a truncated pool.
            total_estimate: Some(u64::from(summary.total)),
        },
    })
}

/// The cross-page totals for a filter, without assembling any rows.
///
/// Used on its own by `get_project_context`, whose briefing carries the ledger's
/// counts while the rows come from `list_commitments` — the same division the
/// index's [`crate::index::NoteIndex::outstanding_summary`] draws.
pub fn commitments_summary(
    ledger: &Ledger,
    params: &CommitmentsParams,
    today: NaiveDate,
) -> Result<CommitmentsSummary, CommitmentsError> {
    if params.state.is_empty() {
        return Ok(CommitmentsSummary::default());
    }
    let details = ledger.list_details(&filter_for(params))?;
    // Assembled against an empty note map: with no context to join, `item` and
    // `source` degrade to `None` — exactly what a count needs, and it costs no
    // index reads. The tier goes unused, so the thresholds are the defaults.
    let counted = view::assemble(details, &HashMap::new(), today, AgingConfig::default());
    Ok(summarize(&counted))
}

/// Assembles what the index knows about each note a commitment points at.
///
/// Best-effort per note: a missing row or a read error omits that id rather
/// than failing the batch, because a commitment whose note the index has lost
/// still renders from the ledger's cached text.
///
/// Lives here rather than in the shell so both readers of the ledger — the
/// desktop app and the MCP server — assemble the join the same way.
pub fn note_contexts(
    index: &NoteIndex,
    note_ids: &BTreeSet<String>,
) -> HashMap<String, NoteContext> {
    if note_ids.is_empty() {
        return HashMap::new();
    }
    let mut contexts = HashMap::with_capacity(note_ids.len());
    for id in note_ids {
        let Ok(Some(row)) = index.get_note(id) else {
            continue;
        };
        let Ok(items) = index.get_action_items(id) else {
            continue;
        };
        contexts.insert(
            id.clone(),
            NoteContext {
                note_id: row.id,
                title: row.title,
                project: row.project,
                path: row.path,
                category: row.category,
                items,
            },
        );
    }
    contexts
}

/// The notes a set of entries still points into — active refs only, since a
/// retired ref is history and names a line that has moved on.
fn referenced_notes(details: &[EntryDetail]) -> BTreeSet<String> {
    details
        .iter()
        .flat_map(|detail| detail.item_refs.iter())
        .filter(|item_ref| item_ref.active)
        .map(|item_ref| item_ref.note_id.clone())
        .collect()
}

fn filter_for(params: &CommitmentsParams) -> EntryFilter {
    EntryFilter {
        states: Some(params.state.clone()),
        // The Inbox sentinel is spelled one way in the ledger and any way by a
        // caller, matching how the other project filters read it.
        project: params.project.as_deref().map(canonical_project),
        include_descendants: params.include_descendants,
        direction: params.direction,
        note_id: params.source_note_id.clone(),
        stale_before: None,
        unchecked_since: None,
    }
}

fn canonical_project(project: &str) -> String {
    if project.eq_ignore_ascii_case(INBOX) {
        INBOX.to_string()
    } else {
        project.to_string()
    }
}

fn empty_page() -> CommitmentsResults {
    CommitmentsResults {
        commitments: Vec::new(),
        summary: CommitmentsSummary::default(),
        page: PageInfo {
            has_more: false,
            next_cursor: None,
            total_estimate: Some(0),
        },
    }
}

fn summarize(commitments: &[Commitment]) -> CommitmentsSummary {
    let mut summary = CommitmentsSummary {
        total: commitments.len() as u32,
        ..CommitmentsSummary::default()
    };
    for commitment in commitments {
        match commitment.detail.entry.direction {
            Direction::Mine => summary.mine += 1,
            Direction::Theirs => summary.theirs += 1,
            Direction::Unassigned => summary.unassigned += 1,
        }
        if commitment.detail.entry.state == EntryState::NeedsReview {
            summary.needs_review += 1;
        }
        if commitment.snooze_lapsed {
            summary.snooze_lapsed += 1;
        }
    }
    summary
}

/// Whether a commitment sorts strictly after the cursor's boundary row, under
/// the same (`last_mention` DESC, `entry_id` ASC) order the page uses.
fn after_cursor(commitment: &Commitment, cursor: &CommitmentsCursor) -> bool {
    let entry = &commitment.detail.entry;
    match entry.last_mention.cmp(&cursor.last_mention) {
        std::cmp::Ordering::Greater => false,
        std::cmp::Ordering::Less => true,
        std::cmp::Ordering::Equal => entry.entry_id > cursor.entry_id,
    }
}

fn encode_commitments_cursor(last_mention: &str, entry_id: &str) -> String {
    // An entry id matches `^le_[0-9a-z]{6,}$` and so cannot contain `:`, while
    // an RFC 3339 timestamp can — so the id goes first and the timestamp takes
    // the unbounded tail.
    format!("v1:{entry_id}:{last_mention}")
}

/// Decodes a cursor, rejecting anything [`encode_commitments_cursor`] did not
/// produce.
fn decode_commitments_cursor(raw: &str) -> Result<CommitmentsCursor, CommitmentsError> {
    let bad = || CommitmentsError::Cursor {
        value: raw.to_string(),
    };
    let rest = raw.strip_prefix("v1:").ok_or_else(bad)?;
    let mut parts = rest.splitn(2, ':');
    let entry_id = parts.next().ok_or_else(bad)?.to_string();
    let last_mention = parts.next().ok_or_else(bad)?.to_string();
    if entry_id.is_empty() || last_mention.is_empty() {
        return Err(bad());
    }
    Ok(CommitmentsCursor {
        last_mention,
        entry_id,
    })
}

fn commitment_wire(commitment: Commitment) -> CommitmentWire {
    let Commitment {
        detail,
        item,
        source,
        snooze_lapsed,
        tier,
    } = commitment;
    let entry = detail.entry;
    CommitmentWire {
        entry_id: entry.entry_id,
        state: entry.state,
        direction: entry.direction,
        owner: entry.owner,
        description: entry.description,
        project: (entry.project != INBOX).then_some(entry.project),
        created_at: entry.created_at,
        updated_at: entry.updated_at,
        last_mention: entry.last_mention,
        last_evidence_check: entry.last_evidence_check,
        tier: tier.as_str().to_string(),
        snoozed_until: entry.snoozed_until,
        snooze_lapsed,
        closed_via: entry.closed_via.map(|via| via.as_str().to_string()),
        review_reason: entry.review_reason,
        untracked_via: entry.untracked_via.map(|via| via.as_str().to_string()),
        item: item.map(|item| CommitmentItemWire {
            note_id: item.note_id,
            item_id: item.item_id,
            description: item.description,
            owner: item.owner,
            due_date: item.due_date,
            done: item.done,
            status: item.status,
        }),
        source: source.map(|source| CommitmentSourceWire {
            note_id: source.note_id,
            title: source.title,
            project: source.project,
            path: source.path,
            category: source
                .category
                .map(|category| category.as_str().to_string()),
        }),
        evidence: detail
            .evidence
            .into_iter()
            .map(|claim| CommitmentEvidenceWire {
                evidence_id: claim.evidence_id,
                source: claim.source.as_str().to_string(),
                reference: claim.reference,
                confidence: claim.confidence,
                observed_at: claim.observed_at,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{IndexedNote, NoteType};
    use crate::ledger::{ClosedVia, NoteSync, OwnerIdentity, UntrackedVia};
    use crate::meeting::{ActionItemFact, MeetingFacts};

    const NOW: &str = "2026-08-17T12:00:00Z";

    /// A fixed "today" so no test depends on the wall clock.
    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 8, 21).unwrap()
    }

    fn fact(id: &str, owner: &str, description: &str) -> ActionItemFact {
        ActionItemFact {
            id: id.to_string(),
            description: description.to_string(),
            owner: owner.to_string(),
            due_date: None,
            done: false,
            firm: true,
            extracted_date: Some("2026-08-01".to_string()),
        }
    }

    fn note(id: &str, project: Option<&str>, items: Vec<ActionItemFact>) -> IndexedNote {
        IndexedNote {
            id: id.to_string(),
            path: format!("{}/{id}.md", project.unwrap_or("Inbox")),
            title: format!("Title {id}"),
            note_type: NoteType::Meeting,
            project: project.map(str::to_string),
            date: "2026-08-01".to_string(),
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

    /// Syncs one note's items into the ledger, the way the index worker does.
    fn sync(
        ledger: &mut Ledger,
        note_id: &str,
        project: &str,
        date: &str,
        items: &[ActionItemFact],
    ) -> Vec<String> {
        ledger
            .sync_note_items(&NoteSync {
                note_id,
                project,
                note_date_utc: date,
                items,
                link_hints: &[],
                note_override: None,
                category_default: None,
                identity: &OwnerIdentity::default(),
                now: NOW,
            })
            .unwrap()
            .created
    }

    fn all_entry_ids(ledger: &Ledger) -> Vec<String> {
        ledger
            .list_entries(&EntryFilter {
                states: None,
                project: None,
                include_descendants: false,
                direction: None,
                note_id: None,
                stale_before: None,
                unchecked_since: None,
            })
            .unwrap()
            .into_iter()
            .map(|entry| entry.entry_id)
            .collect()
    }

    /// One project's worth of commitments in both stores, mentioned on three
    /// distinct days so the sort order is unambiguous.
    fn seeded() -> (Ledger, NoteIndex) {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let mut index = NoteIndex::open_in_memory().unwrap();

        let growth = vec![
            fact("a_one001", "You", "send the deck"),
            fact("a_two001", "Priya", "review the budget"),
        ];
        index
            .upsert_note(&note("n_growth", Some("Growth"), growth.clone()))
            .unwrap();
        sync(
            &mut ledger,
            "n_growth",
            "Growth",
            "2026-08-01T00:00:00Z",
            &growth,
        );

        let nested = vec![fact("a_three1", "You", "book the venue")];
        index
            .upsert_note(&note("n_nested", Some("Growth/Q3"), nested.clone()))
            .unwrap();
        sync(
            &mut ledger,
            "n_nested",
            "Growth/Q3",
            "2026-08-05T00:00:00Z",
            &nested,
        );

        let other = vec![fact("a_four01", "You", "chase the invoice")];
        index
            .upsert_note(&note("n_other", Some("Briarwood"), other.clone()))
            .unwrap();
        sync(
            &mut ledger,
            "n_other",
            "Briarwood",
            "2026-08-10T00:00:00Z",
            &other,
        );

        (ledger, index)
    }

    fn list(ledger: &Ledger, index: &NoteIndex, params: &CommitmentsParams) -> CommitmentsResults {
        list_commitments(ledger, index, params, today(), AgingConfig::default()).unwrap()
    }

    fn descriptions(results: &CommitmentsResults) -> Vec<&str> {
        results
            .commitments
            .iter()
            .map(|commitment| commitment.description.as_str())
            .collect()
    }

    #[test]
    fn the_default_filter_is_the_live_working_set() {
        let (ledger, index) = seeded();
        let results = list(&ledger, &index, &CommitmentsParams::default());
        assert_eq!(results.commitments.len(), 4);
        assert_eq!(results.summary.total, 4);
    }

    /// The whole point of the tool: what the ledger has settled must not come
    /// back as outstanding, which is what raw extraction would still report.
    #[test]
    fn settled_and_untracked_entries_are_absent_by_default() {
        let (mut ledger, index) = seeded();
        let ids = all_entry_ids(&ledger);

        ledger.close(&ids[0], ClosedVia::Manual, NOW).unwrap();
        ledger.waive(&ids[1], NOW).unwrap();
        ledger.untrack(&ids[2], UntrackedVia::Manual, NOW).unwrap();

        let results = list(&ledger, &index, &CommitmentsParams::default());
        assert_eq!(
            results.commitments.len(),
            1,
            "only the untouched entry is live"
        );
        assert_eq!(results.summary.total, 1);

        // Each is still reachable by asking for it, so nothing is lost.
        let settled = list(
            &ledger,
            &index,
            &CommitmentsParams {
                state: vec![
                    EntryState::Closed,
                    EntryState::Waived,
                    EntryState::Untracked,
                ],
                ..CommitmentsParams::default()
            },
        );
        assert_eq!(settled.commitments.len(), 3);
    }

    #[test]
    fn an_explicitly_empty_state_list_matches_nothing() {
        let (ledger, index) = seeded();
        let results = list(
            &ledger,
            &index,
            &CommitmentsParams {
                state: Vec::new(),
                ..CommitmentsParams::default()
            },
        );
        assert!(results.commitments.is_empty());
        assert_eq!(results.summary.total, 0);
        assert_eq!(results.page.total_estimate, Some(0));
        assert!(!results.page.has_more);
    }

    #[test]
    fn a_project_filter_includes_descendants_but_not_siblings() {
        let (ledger, index) = seeded();
        let nested = list(
            &ledger,
            &index,
            &CommitmentsParams {
                project: Some("Growth".to_string()),
                ..CommitmentsParams::default()
            },
        );
        assert_eq!(nested.commitments.len(), 3, "Growth plus Growth/Q3");

        let shallow = list(
            &ledger,
            &index,
            &CommitmentsParams {
                project: Some("Growth".to_string()),
                include_descendants: false,
                ..CommitmentsParams::default()
            },
        );
        assert_eq!(shallow.commitments.len(), 2);
    }

    #[test]
    fn direction_and_source_note_narrow_the_set() {
        let (ledger, index) = seeded();
        let mine = list(
            &ledger,
            &index,
            &CommitmentsParams {
                direction: Some(Direction::Mine),
                ..CommitmentsParams::default()
            },
        );
        assert!(
            mine.commitments
                .iter()
                .all(|commitment| commitment.direction == Direction::Mine),
            "every row is the user's own"
        );

        let one_note = list(
            &ledger,
            &index,
            &CommitmentsParams {
                source_note_id: Some("n_nested".to_string()),
                ..CommitmentsParams::default()
            },
        );
        assert_eq!(descriptions(&one_note), vec!["book the venue"]);
    }

    #[test]
    fn rows_carry_their_live_source_line_and_note() {
        let (ledger, index) = seeded();
        let results = list(
            &ledger,
            &index,
            &CommitmentsParams {
                source_note_id: Some("n_other".to_string()),
                ..CommitmentsParams::default()
            },
        );
        let commitment = &results.commitments[0];
        let item = commitment.item.as_ref().expect("the ref is live");
        assert_eq!(item.item_id, "a_four01");
        assert!(!item.done);
        let source = commitment
            .source
            .as_ref()
            .expect("the index knows the note");
        assert_eq!(source.note_id, "n_other");
        assert_eq!(source.project.as_deref(), Some("Briarwood"));
    }

    /// A commitment whose note the index has lost still renders, from the
    /// ledger's own cached copy of the words.
    #[test]
    fn a_missing_note_degrades_to_cached_text_rather_than_an_error() {
        let (ledger, _) = seeded();
        let empty_index = NoteIndex::open_in_memory().unwrap();
        let results = list(&ledger, &empty_index, &CommitmentsParams::default());
        assert_eq!(results.commitments.len(), 4);
        assert!(results
            .commitments
            .iter()
            .all(|commitment| commitment.item.is_none() && commitment.source.is_none()));
        assert!(results
            .commitments
            .iter()
            .all(|commitment| !commitment.description.is_empty()));
    }

    #[test]
    fn paging_walks_every_row_exactly_once() {
        let (ledger, index) = seeded();
        let mut seen: Vec<String> = Vec::new();
        let mut cursor = None;
        for _ in 0..10 {
            let results = list(
                &ledger,
                &index,
                &CommitmentsParams {
                    limit: 1,
                    cursor: cursor.clone(),
                    ..CommitmentsParams::default()
                },
            );
            assert_eq!(results.commitments.len(), 1);
            // The cross-page total is the same on every page.
            assert_eq!(results.summary.total, 4);
            seen.push(results.commitments[0].entry_id.clone());
            cursor = results.page.next_cursor.clone();
            if cursor.is_none() {
                assert!(!results.page.has_more);
                break;
            }
        }
        assert_eq!(seen.len(), 4, "one page per row, then the walk ends");
        let unique: BTreeSet<&String> = seen.iter().collect();
        assert_eq!(unique.len(), 4, "no row repeated across pages");
    }

    #[test]
    fn rows_come_back_newest_mention_first() {
        let (ledger, index) = seeded();
        let results = list(&ledger, &index, &CommitmentsParams::default());
        let mentions: Vec<&str> = results
            .commitments
            .iter()
            .map(|commitment| commitment.last_mention.as_str())
            .collect();
        let mut sorted = mentions.clone();
        sorted.sort_unstable();
        sorted.reverse();
        assert_eq!(mentions, sorted);
    }

    #[test]
    fn a_tampered_cursor_is_rejected_rather_than_ignored() {
        let (ledger, index) = seeded();
        for raw in [
            "nonsense",
            "v2:le_abc:2026-08-01T00:00:00Z",
            "v1:",
            "v1:le_abc",
        ] {
            let error = list_commitments(
                &ledger,
                &index,
                &CommitmentsParams {
                    cursor: Some(raw.to_string()),
                    ..CommitmentsParams::default()
                },
                today(),
                AgingConfig::default(),
            )
            .expect_err("should refuse");
            assert!(matches!(error, CommitmentsError::Cursor { .. }), "{raw}");
        }
    }

    #[test]
    fn an_empty_ledger_is_an_empty_page_not_an_error() {
        let ledger = Ledger::open_in_memory().unwrap();
        let index = NoteIndex::open_in_memory().unwrap();
        let results = list(&ledger, &index, &CommitmentsParams::default());
        assert!(results.commitments.is_empty());
        assert!(!results.page.has_more);
        assert_eq!(results.page.total_estimate, Some(0));
    }

    #[test]
    fn the_inbox_sentinel_is_matched_any_casing_and_projects_as_null() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let mut index = NoteIndex::open_in_memory().unwrap();
        let items = vec![fact("a_inbox1", "You", "file the receipt")];
        index
            .upsert_note(&note("n_inbox", None, items.clone()))
            .unwrap();
        sync(
            &mut ledger,
            "n_inbox",
            INBOX,
            "2026-08-02T00:00:00Z",
            &items,
        );

        for spelling in ["Inbox", "inbox", "INBOX"] {
            let results = list(
                &ledger,
                &index,
                &CommitmentsParams {
                    project: Some(spelling.to_string()),
                    ..CommitmentsParams::default()
                },
            );
            assert_eq!(results.commitments.len(), 1, "{spelling}");
            assert_eq!(
                results.commitments[0].project, None,
                "the sentinel is null on the wire"
            );
        }
    }

    #[test]
    fn the_summary_counts_directions_across_every_page() {
        let (ledger, index) = seeded();
        let results = list(
            &ledger,
            &index,
            &CommitmentsParams {
                limit: 1,
                ..CommitmentsParams::default()
            },
        );
        let summary = results.summary;
        assert_eq!(summary.total, 4);
        assert_eq!(
            summary.mine + summary.theirs + summary.unassigned,
            summary.total,
            "every row lands in exactly one direction bucket"
        );
    }

    #[test]
    fn the_standalone_summary_agrees_with_the_paged_one() {
        let (ledger, index) = seeded();
        let params = CommitmentsParams {
            project: Some("Growth".to_string()),
            ..CommitmentsParams::default()
        };
        let paged = list(&ledger, &index, &params);
        let counted = commitments_summary(&ledger, &params, today()).unwrap();
        assert_eq!(counted, paged.summary);
    }

    #[test]
    fn a_needs_review_entry_is_counted_and_still_live() {
        let (mut ledger, index) = seeded();
        // Re-syncing the note with the line gone retires its ref, which is what
        // sends the entry to review.
        sync(
            &mut ledger,
            "n_nested",
            "Growth/Q3",
            "2026-08-05T00:00:00Z",
            &[],
        );
        let results = list(&ledger, &index, &CommitmentsParams::default());
        assert_eq!(results.summary.needs_review, 1);
        assert!(results
            .commitments
            .iter()
            .any(|commitment| commitment.state == EntryState::NeedsReview));
    }

    #[test]
    fn a_lapsed_snooze_is_flagged_and_counted() {
        let (mut ledger, index) = seeded();
        let entry_id = all_entry_ids(&ledger)[0].clone();
        // Yesterday, relative to the fixed `today()`.
        ledger.snooze(&entry_id, "2026-08-20", NOW).unwrap();

        let results = list(&ledger, &index, &CommitmentsParams::default());
        let snoozed = results
            .commitments
            .iter()
            .find(|commitment| commitment.entry_id == entry_id)
            .expect("a lapsed snooze is still live work");
        assert!(snoozed.snooze_lapsed);
        assert_eq!(snoozed.snoozed_until.as_deref(), Some("2026-08-20"));
        assert_eq!(results.summary.snooze_lapsed, 1);
    }

    #[test]
    fn the_aging_tier_follows_the_callers_thresholds() {
        let (ledger, index) = seeded();
        // Every fixture was last mentioned 11-20 days before `today()`, so a
        // one-day threshold makes them all stale and a one-year one keeps them
        // all fresh. The tier is the caller's judgement, not this module's.
        let tight = list_commitments(
            &ledger,
            &index,
            &CommitmentsParams::default(),
            today(),
            AgingConfig {
                aging_after_days: 1,
                stale_after_days: 2,
            },
        )
        .unwrap();
        assert!(tight
            .commitments
            .iter()
            .all(|commitment| commitment.tier == "stale"));

        let generous = list_commitments(
            &ledger,
            &index,
            &CommitmentsParams::default(),
            today(),
            AgingConfig {
                aging_after_days: 365,
                stale_after_days: 730,
            },
        )
        .unwrap();
        assert!(generous
            .commitments
            .iter()
            .all(|commitment| commitment.tier == "fresh"));
    }
}
