//! The read model a commitments surface renders: ledger entries joined to the
//! source lines the index holds, with read-time snooze expiry applied.
//!
//! Lives here rather than in the shell because it is the whole of the join's
//! judgement — which ref is the live one, what a lapsed snooze means, what to
//! render when the source line is gone — and none of it needs SQLite or a
//! clock. `today` is an argument, matching the index's doctrine, so every rule
//! below is deterministically testable.
//!
//! The pieces come from two stores on purpose. The ledger owns identity and the
//! states a checkbox cannot spell; the index owns `done` and `due_date`, which
//! the note's Markdown is the source of truth for ([`super`]). Neither caches
//! the other's half.

use std::collections::HashMap;

use chrono::NaiveDate;

use crate::index::{ActionItemRow, ActionItemStatus};

use super::{EntryDetail, EntryState};

/// What the index knows about one note a commitment points at.
///
/// Assembled by the shell, which owns the index handle; the fields are exactly
/// what a row needs to render its source line and offer a click-through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteContext {
    pub note_id: String,
    /// Effective title, as the index stores it.
    pub title: String,
    /// Project slug, or `None` for an unfiled note.
    pub project: Option<String>,
    /// Vault-relative path, informational (a move changes it; the id is the
    /// handle).
    pub path: String,
    /// The note's action items, in body order.
    pub items: Vec<ActionItemRow>,
}

/// The live source line behind a commitment: the note's current text and the
/// checkbox that owns done/not-done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitmentItem {
    pub note_id: String,
    pub item_id: String,
    pub description: String,
    pub owner: String,
    pub due_date: Option<String>,
    pub done: bool,
    pub status: ActionItemStatus,
}

/// Where a commitment's source line lives, for a click-through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitmentSource {
    pub note_id: String,
    pub title: String,
    pub project: Option<String>,
    pub path: String,
}

/// One ledger entry as a surface renders it.
#[derive(Debug, Clone, PartialEq)]
pub struct Commitment {
    pub detail: EntryDetail,
    /// The live source line, or `None` when the entry has no active ref or the
    /// index has no row for it. A reader falls back to the entry's cached
    /// `owner`/`description`, which is exactly why those are stored.
    pub item: Option<CommitmentItem>,
    /// The note the active ref points into, when the index knows it.
    pub source: Option<CommitmentSource>,
    /// A snooze whose day has arrived. Nothing writes when a snooze lapses
    /// ([`EntryState::Snoozed`]), so the surface asks here and files a lapsed
    /// entry back with the live work.
    pub snooze_lapsed: bool,
}

/// Joins entries to their source lines and evaluates snooze expiry.
///
/// `notes` is keyed by note id and may be missing entries entirely: a note that
/// left the vault, or an index that failed to open, degrades to cached text
/// rather than to an error.
pub fn assemble(
    details: Vec<EntryDetail>,
    notes: &HashMap<String, NoteContext>,
    today: NaiveDate,
) -> Vec<Commitment> {
    details
        .into_iter()
        .map(|detail| assemble_one(detail, notes, today))
        .collect()
}

fn assemble_one(
    detail: EntryDetail,
    notes: &HashMap<String, NoteContext>,
    today: NaiveDate,
) -> Commitment {
    let snooze_lapsed = snooze_lapsed(&detail, today);
    // Refs come back active-first ([`Ledger::item_refs`]), and only a live ref
    // names the line a person can still tick. A retired ref is history.
    let active = detail.item_refs.iter().find(|item_ref| item_ref.active);

    let context = active.and_then(|item_ref| notes.get(&item_ref.note_id));
    let item = active.zip(context).and_then(|(item_ref, note)| {
        let row = note.items.iter().find(|row| row.id == item_ref.item_id)?;
        Some(CommitmentItem {
            note_id: note.note_id.clone(),
            item_id: row.id.clone(),
            description: row.description.clone(),
            owner: row.owner.clone(),
            due_date: row.due_date.clone(),
            done: row.done,
            status: ActionItemStatus::derive(row.done, row.due_date.as_deref(), today),
        })
    });
    let source = context.map(|note| CommitmentSource {
        note_id: note.note_id.clone(),
        title: note.title.clone(),
        project: note.project.clone(),
        path: note.path.clone(),
    });

    Commitment {
        detail,
        item,
        source,
        snooze_lapsed,
    }
}

/// Whether a snoozed entry's day has arrived.
///
/// "Snoozed until Friday" resurfaces *on* Friday, so the comparison is
/// inclusive. An unreadable date counts as lapsed: a surface that shows a
/// commitment early is a smaller failure than one that hides it forever.
fn snooze_lapsed(detail: &EntryDetail, today: NaiveDate) -> bool {
    if detail.entry.state != EntryState::Snoozed {
        return false;
    }
    match detail.entry.snoozed_until.as_deref() {
        Some(until) => match NaiveDate::parse_from_str(until, "%Y-%m-%d") {
            Ok(day) => day <= today,
            Err(_) => true,
        },
        None => true,
    }
}

/// The most recently settled entries, newest first.
///
/// The undo shelf: a closure reached by evidence has to be visible to be
/// undoable, so a surface shows what just settled rather than making the user
/// trust that it was right. Bounded twice over, by age and by count, because
/// this is a shelf and not a history view.
pub fn recently_settled(
    details: Vec<EntryDetail>,
    cutoff_utc: &str,
    cap: usize,
) -> Vec<EntryDetail> {
    let mut recent: Vec<EntryDetail> = details
        .into_iter()
        // Both sides are RFC 3339 UTC with a `Z` and seconds precision
        // (`.claude/rules/utc-timestamps.md`), which is exactly the format
        // whose lexical order is its chronological order.
        .filter(|detail| detail.entry.updated_at.as_str() >= cutoff_utc)
        .collect();
    recent.sort_by(|a, b| {
        b.entry
            .updated_at
            .cmp(&a.entry.updated_at)
            .then_with(|| a.entry.entry_id.cmp(&b.entry.entry_id))
    });
    recent.truncate(cap);
    recent
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::sync::NoteSync;
    use crate::ledger::{ClosedVia, Ledger};
    use crate::meeting::ActionItemFact;

    const NOW: &str = "2026-08-17T12:00:00Z";
    const DAY: &str = "2026-08-17T00:00:00Z";

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 8, 17).unwrap()
    }

    fn fact(id: &str, owner: &str, description: &str) -> ActionItemFact {
        ActionItemFact {
            id: id.to_string(),
            description: description.to_string(),
            owner: owner.to_string(),
            due_date: None,
            done: false,
            extracted_date: Some("2026-08-17".to_string()),
        }
    }

    fn row(
        id: &str,
        owner: &str,
        description: &str,
        due: Option<&str>,
        done: bool,
    ) -> ActionItemRow {
        ActionItemRow {
            id: id.to_string(),
            description: description.to_string(),
            owner: owner.to_string(),
            due_date: due.map(str::to_string),
            done,
            extracted_date: Some("2026-08-17".to_string()),
        }
    }

    fn context(note_id: &str, items: Vec<ActionItemRow>) -> NoteContext {
        NoteContext {
            note_id: note_id.to_string(),
            title: "Kickoff".to_string(),
            project: Some("Briarwood Golf".to_string()),
            path: format!("Briarwood Golf/{note_id}.md"),
            items,
        }
    }

    /// A ledger holding one open entry for `a_111111` in `n_a1b2c3`.
    fn seeded() -> (Ledger, String) {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![fact("a_111111", "Priya", "send the revised deck")];
        let outcome = ledger
            .sync_note_items(&NoteSync {
                note_id: "n_a1b2c3",
                project: "Briarwood Golf",
                note_date_utc: DAY,
                items: &items,
                now: NOW,
            })
            .unwrap();
        (ledger, outcome.created[0].clone())
    }

    #[test]
    fn assemble_joins_through_the_active_ref() {
        let (ledger, _) = seeded();
        let details = ledger.list_details(&Default::default()).unwrap();
        let notes = HashMap::from([(
            "n_a1b2c3".to_string(),
            context(
                "n_a1b2c3",
                vec![row(
                    "a_111111",
                    "Priya",
                    "send the revised deck by Friday",
                    Some("2026-08-20"),
                    false,
                )],
            ),
        )]);

        let assembled = assemble(details, &notes, today());

        let item = assembled[0].item.as_ref().expect("the live line");
        // The note's current text wins over the entry's cached copy.
        assert_eq!(item.description, "send the revised deck by Friday");
        assert_eq!(item.due_date.as_deref(), Some("2026-08-20"));
        assert_eq!(item.status, ActionItemStatus::Open);
        assert!(!item.done);
        let source = assembled[0].source.as_ref().expect("the source note");
        assert_eq!(source.note_id, "n_a1b2c3");
        assert_eq!(source.project.as_deref(), Some("Briarwood Golf"));
    }

    #[test]
    fn a_retired_ref_never_joins() {
        // The section was deleted, so the entry keeps only a retired ref: the
        // row must fall back to cached text rather than to a stale line.
        let (mut ledger, _) = seeded();
        ledger
            .sync_note_items(&NoteSync {
                note_id: "n_a1b2c3",
                project: "Briarwood Golf",
                note_date_utc: DAY,
                items: &[],
                now: NOW,
            })
            .unwrap();

        let details = ledger.list_details(&Default::default()).unwrap();
        let retired = details[0].clone();
        assert_eq!(retired.entry.state, EntryState::NeedsReview);
        assert!(retired.item_refs.iter().all(|item_ref| !item_ref.active));

        let notes = HashMap::from([(
            "n_a1b2c3".to_string(),
            context(
                "n_a1b2c3",
                vec![row("a_111111", "Priya", "stale text", None, true)],
            ),
        )]);
        let assembled = assemble(vec![retired], &notes, today());
        assert!(assembled[0].item.is_none(), "a retired ref is history");
        assert!(assembled[0].source.is_none());
    }

    #[test]
    fn a_missing_index_row_degrades_to_cached_text() {
        let (ledger, _) = seeded();
        let details = ledger.list_details(&Default::default()).unwrap();

        let assembled = assemble(details, &HashMap::new(), today());

        assert!(assembled[0].item.is_none());
        assert!(assembled[0].source.is_none());
        // The entry's own cached fields are what a reader renders instead.
        assert_eq!(
            assembled[0].detail.entry.description,
            "send the revised deck"
        );
        assert_eq!(assembled[0].detail.entry.owner, "Priya");
    }

    #[test]
    fn status_derives_against_the_supplied_today() {
        let (ledger, _) = seeded();
        let details = ledger.list_details(&Default::default()).unwrap();
        let notes = HashMap::from([(
            "n_a1b2c3".to_string(),
            context(
                "n_a1b2c3",
                vec![row(
                    "a_111111",
                    "Priya",
                    "send it",
                    Some("2026-08-16"),
                    false,
                )],
            ),
        )]);

        let assembled = assemble(details.clone(), &notes, today());
        assert_eq!(
            assembled[0].item.as_ref().unwrap().status,
            ActionItemStatus::Overdue
        );

        // The same data read a week earlier is not overdue.
        let earlier = NaiveDate::from_ymd_opt(2026, 8, 10).unwrap();
        let assembled = assemble(details, &notes, earlier);
        assert_eq!(
            assembled[0].item.as_ref().unwrap().status,
            ActionItemStatus::Open
        );
    }

    #[test]
    fn a_ticked_box_reads_as_done_without_the_ledger_knowing() {
        let (ledger, _) = seeded();
        let details = ledger.list_details(&Default::default()).unwrap();
        let notes = HashMap::from([(
            "n_a1b2c3".to_string(),
            context(
                "n_a1b2c3",
                vec![row("a_111111", "Priya", "send it", None, true)],
            ),
        )]);

        let assembled = assemble(details, &notes, today());

        assert!(assembled[0].item.as_ref().unwrap().done);
        assert_eq!(
            assembled[0].item.as_ref().unwrap().status,
            ActionItemStatus::Done
        );
        // The entry itself is still open: done lives in the note, not here.
        assert_eq!(assembled[0].detail.entry.state, EntryState::Open);
    }

    #[test]
    fn a_snooze_lapses_on_its_own_day() {
        let (mut ledger, entry_id) = seeded();

        for (until, expected) in [
            ("2026-08-16", true),  // yesterday
            ("2026-08-17", true),  // today: "until Friday" resurfaces ON Friday
            ("2026-08-18", false), // tomorrow
        ] {
            ledger.snooze(&entry_id, until, NOW).unwrap();
            let details = ledger.list_details(&Default::default()).unwrap();
            let assembled = assemble(details, &HashMap::new(), today());
            assert_eq!(
                assembled[0].snooze_lapsed, expected,
                "snoozed until {until}, read on 2026-08-17"
            );
        }
    }

    #[test]
    fn an_unreadable_snooze_date_counts_as_lapsed() {
        // Visible early beats hidden forever.
        let (ledger, _) = seeded();
        let mut details = ledger.list_details(&Default::default()).unwrap();
        details[0].entry.state = EntryState::Snoozed;
        details[0].entry.snoozed_until = Some("next Tuesday".to_string());

        let assembled = assemble(details, &HashMap::new(), today());
        assert!(assembled[0].snooze_lapsed);
    }

    #[test]
    fn only_a_snoozed_entry_can_lapse() {
        let (mut ledger, entry_id) = seeded();
        ledger.close(&entry_id, ClosedVia::Manual, NOW).unwrap();
        let details = ledger.list_details(&Default::default()).unwrap();

        let assembled = assemble(details, &HashMap::new(), today());
        assert!(!assembled[0].snooze_lapsed);
    }

    #[test]
    fn recently_settled_windows_sorts_and_caps() {
        let (ledger, _) = seeded();
        let template = ledger.list_details(&Default::default()).unwrap()[0].clone();
        let stamped = |entry_id: &str, updated_at: &str| {
            let mut detail = template.clone();
            detail.entry.entry_id = entry_id.to_string();
            detail.entry.updated_at = updated_at.to_string();
            detail
        };

        let settled = recently_settled(
            vec![
                stamped("le_old", "2026-08-01T00:00:00Z"),
                stamped("le_edge", "2026-08-10T00:00:00Z"),
                stamped("le_newest", "2026-08-17T09:00:00Z"),
                stamped("le_mid", "2026-08-14T00:00:00Z"),
            ],
            "2026-08-10T00:00:00Z",
            2,
        );

        // Newest first, the pre-cutoff entry dropped, capped at two.
        assert_eq!(
            settled
                .iter()
                .map(|detail| detail.entry.entry_id.as_str())
                .collect::<Vec<_>>(),
            vec!["le_newest", "le_mid"]
        );

        // The cutoff is inclusive, so an entry settled exactly at it survives.
        let settled = recently_settled(
            vec![stamped("le_edge", "2026-08-10T00:00:00Z")],
            "2026-08-10T00:00:00Z",
            10,
        );
        assert_eq!(settled.len(), 1);
    }
}
