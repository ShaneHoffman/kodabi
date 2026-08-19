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

use super::{Direction, EntryDetail, EntryState, UntrackedVia};

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

/// How long an untouched commitment stays [`AgingTier::Fresh`], then
/// [`AgingTier::Aging`], before it reads as [`AgingTier::Stale`].
///
/// Days rather than instants because this is a calendar judgement a person
/// makes ("nobody has said anything for a month"), and the same reason
/// [`AgingTier::derive`] takes `today` rather than an instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgingConfig {
    pub aging_after_days: u32,
    pub stale_after_days: u32,
}

/// The default thresholds: a commitment on a weekly cadence has missed at
/// least one full cycle at a fortnight, and a month of silence is the point
/// where it has most likely been forgotten rather than merely delayed.
pub const DEFAULT_AGING_AFTER_DAYS: u32 = 14;
pub const DEFAULT_STALE_AFTER_DAYS: u32 = 30;

impl Default for AgingConfig {
    fn default() -> Self {
        Self {
            aging_after_days: DEFAULT_AGING_AFTER_DAYS,
            stale_after_days: DEFAULT_STALE_AFTER_DAYS,
        }
    }
}

impl AgingConfig {
    /// The stale threshold as the derivation actually applies it: never below
    /// the aging one, so a config with the two inverted degrades to a single
    /// boundary rather than to a tier that can never be reached. Clamped at
    /// use rather than at construction, matching `RoutingConfig`, so the
    /// stored value stays the number the user typed.
    fn effective_stale_after_days(self) -> u32 {
        self.stale_after_days.max(self.aging_after_days)
    }
}

/// How long a commitment has gone without anyone touching it.
///
/// Derived, never stored: the inputs are already on the entry, and a stored
/// tier would need a writer on the day it changes — which is exactly the
/// mechanism [`super::EntryState::Snoozed`] deliberately avoids.
///
/// The pre-meeting prep briefs of Theme 2 are the other intended consumer:
/// [`derive`](AgingTier::derive) takes the two anchor strings and nothing
/// else, and the vault's `_ledger.yml` snapshot carries both, so a brief can
/// call this without opening the ledger database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgingTier {
    Fresh,
    Aging,
    Stale,
}

impl AgingTier {
    /// The wire spelling, matching [`super::EntryState::as_str`]'s convention.
    pub fn as_str(self) -> &'static str {
        match self {
            AgingTier::Fresh => "fresh",
            AgingTier::Aging => "aging",
            AgingTier::Stale => "stale",
        }
    }

    /// Tiers an entry by how long ago anything last touched it.
    ///
    /// The anchor is the later of the two: a mention in a meeting and an
    /// evidence check are both somebody looking at this commitment, and either
    /// one means it has not gone quiet. Deliberately *not* `updated_at`, which
    /// is the sync's wall clock — re-indexing an old vault would then make
    /// every entry look fresh, which is the failure `last_mention` was defined
    /// as a note date to avoid.
    ///
    /// Both anchors are RFC 3339 UTC with a `Z`
    /// (`.claude/rules/utc-timestamps.md`), so the later of the two is the
    /// lexically larger one. An anchor that will not parse reads as `Stale`:
    /// showing a commitment as needing attention is a smaller failure than
    /// hiding one that does, the same call [`snooze_lapsed`] makes.
    pub fn derive(
        last_mention: &str,
        last_evidence_check: Option<&str>,
        today: NaiveDate,
        config: AgingConfig,
    ) -> AgingTier {
        let anchor = match last_evidence_check {
            Some(checked) if checked > last_mention => checked,
            _ => last_mention,
        };
        let Some(day) = anchor
            .get(..10)
            .and_then(|prefix| NaiveDate::parse_from_str(prefix, "%Y-%m-%d").ok())
        else {
            return AgingTier::Stale;
        };
        // A future anchor is not aged: a note dated tomorrow is odd, but it is
        // not evidence of neglect.
        let age_days = today.signed_duration_since(day).num_days().max(0);
        if age_days >= i64::from(config.effective_stale_after_days()) {
            AgingTier::Stale
        } else if age_days >= i64::from(config.aging_after_days) {
            AgingTier::Aging
        } else {
            AgingTier::Fresh
        }
    }
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
    /// How long this entry has gone untouched. Stamped for every entry
    /// whatever its state, including snoozed and settled ones: what a tier
    /// *means* for sort order and for what a row says is the surface's call,
    /// the same division [`Commitment::snooze_lapsed`] draws.
    pub tier: AgingTier,
}

/// Whether one extracted line is in the working set, as the note view asks it.
///
/// Three answers, not two: "no entry" and "untracked" both mean the item is out
/// of the ledger, but they are different offers. The first is an item the mode
/// never enrolled; the second is one that was enrolled and then removed, which
/// a reader may want to see acknowledged rather than silently identical to
/// never having been tracked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemTracking {
    /// Has a live entry.
    Tracked,
    /// Has an entry in [`EntryState::Untracked`].
    Untracked,
    /// Has no entry at all: the enrollment gate skipped it.
    NotEnrolled,
}

impl ItemTracking {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            ItemTracking::Tracked => "tracked",
            ItemTracking::Untracked => "untracked",
            ItemTracking::NotEnrolled => "not_enrolled",
        }
    }
}

/// One of a note's extracted lines, with its enrollment status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteItemEnrollment {
    pub item: ActionItemRow,
    /// Derived from the line's owner, not from the entry: an un-enrolled item
    /// has no entry to ask, and the two always agree for one that does.
    pub direction: Direction,
    pub tracking: ItemTracking,
    /// Why it left the working set; present only when `tracking` is
    /// [`ItemTracking::Untracked`].
    pub untracked_via: Option<UntrackedVia>,
    pub entry_id: Option<String>,
    pub entry_state: Option<EntryState>,
}

/// Joins a note's extracted lines to the entries they produced.
///
/// Keyed on **active refs only**, which is what makes this honest: a retired ref
/// is the history of a line that was edited away, and matching it would report a
/// line as tracked by an entry that has since moved on to different words.
///
/// Body order is preserved from `items`, so the note view lists lines in the
/// order the reader sees them in the Markdown.
pub fn assemble_note_items(
    note_id: &str,
    items: Vec<ActionItemRow>,
    details: &[EntryDetail],
) -> Vec<NoteItemEnrollment> {
    let mut by_item: HashMap<&str, &EntryDetail> = HashMap::new();
    for detail in details {
        for item_ref in &detail.item_refs {
            if item_ref.active && item_ref.note_id == note_id {
                by_item.insert(item_ref.item_id.as_str(), detail);
            }
        }
    }

    items
        .into_iter()
        .map(|item| {
            let matched = by_item.get(item.id.as_str());
            let direction = Direction::from_owner(&item.owner);
            match matched {
                Some(detail) => NoteItemEnrollment {
                    item,
                    direction,
                    tracking: match detail.entry.state {
                        EntryState::Untracked => ItemTracking::Untracked,
                        _ => ItemTracking::Tracked,
                    },
                    untracked_via: detail.entry.untracked_via,
                    entry_id: Some(detail.entry.entry_id.clone()),
                    entry_state: Some(detail.entry.state),
                },
                None => NoteItemEnrollment {
                    item,
                    direction,
                    tracking: ItemTracking::NotEnrolled,
                    untracked_via: None,
                    entry_id: None,
                    entry_state: None,
                },
            }
        })
        .collect()
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
    aging: AgingConfig,
) -> Vec<Commitment> {
    details
        .into_iter()
        .map(|detail| assemble_one(detail, notes, today, aging))
        .collect()
}

fn assemble_one(
    detail: EntryDetail,
    notes: &HashMap<String, NoteContext>,
    today: NaiveDate,
    aging: AgingConfig,
) -> Commitment {
    let snooze_lapsed = snooze_lapsed(&detail, today);
    let tier = AgingTier::derive(
        &detail.entry.last_mention,
        detail.entry.last_evidence_check.as_deref(),
        today,
        aging,
    );
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
        tier,
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
    use crate::ledger::{ClosedVia, Ledger, UntrackedVia};
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
                link_hints: &[],
                now: NOW,
            })
            .unwrap();
        (ledger, outcome.created[0].clone())
    }

    #[test]
    fn note_items_report_tracked_untracked_and_never_enrolled() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        // A context-only meeting: only the direct ask is enrolled.
        ledger
            .set_note_tracking("n_a1b2c3", "Briarwood Golf", true, NOW)
            .unwrap();
        let items = vec![
            fact("a_111111", "Priya", "send the revised deck"),
            fact("a_222222", "You", "book the venue"),
        ];
        ledger
            .sync_note_items(&NoteSync {
                note_id: "n_a1b2c3",
                project: "Briarwood Golf",
                note_date_utc: DAY,
                items: &items,
                link_hints: &[],
                now: NOW,
            })
            .unwrap();
        let mine = ledger
            .entry_for_item("n_a1b2c3", "a_222222")
            .unwrap()
            .unwrap();
        ledger
            .untrack(&mine.entry_id, UntrackedVia::Manual, NOW)
            .unwrap();

        let details = ledger.list_details(&Default::default()).unwrap();
        let assembled = assemble_note_items(
            "n_a1b2c3",
            vec![
                row("a_111111", "Priya", "send the revised deck", None, false),
                row("a_222222", "You", "book the venue", None, false),
            ],
            &details,
        );

        // Body order is the reader's order.
        assert_eq!(assembled[0].item.id, "a_111111");
        assert_eq!(assembled[0].tracking, ItemTracking::NotEnrolled);
        assert_eq!(assembled[0].direction, Direction::Theirs);
        assert_eq!(assembled[0].entry_id, None);

        assert_eq!(assembled[1].tracking, ItemTracking::Untracked);
        assert_eq!(assembled[1].direction, Direction::Mine);
        assert_eq!(assembled[1].untracked_via, Some(UntrackedVia::Manual));
        assert_eq!(assembled[1].entry_state, Some(EntryState::Untracked));
    }

    #[test]
    fn a_note_item_joins_only_through_its_own_active_ref() {
        let (ledger, _) = seeded();
        let details = ledger.list_details(&Default::default()).unwrap();

        // The same line id read against a different note: a ref belongs to the
        // note it was linked in, so this must not report as tracked.
        let elsewhere = assemble_note_items(
            "n_d4e5f6",
            vec![row(
                "a_111111",
                "Priya",
                "send the revised deck",
                None,
                false,
            )],
            &details,
        );
        assert_eq!(elsewhere[0].tracking, ItemTracking::NotEnrolled);

        let here = assemble_note_items(
            "n_a1b2c3",
            vec![row(
                "a_111111",
                "Priya",
                "send the revised deck",
                None,
                false,
            )],
            &details,
        );
        assert_eq!(here[0].tracking, ItemTracking::Tracked);
        assert_eq!(here[0].untracked_via, None);
    }

    #[test]
    fn an_edited_line_reads_as_not_enrolled_through_its_retired_ref() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![fact("a_111111", "Priya", "send the revised deck")];
        ledger
            .sync_note_items(&NoteSync {
                note_id: "n_a1b2c3",
                project: "Briarwood Golf",
                note_date_utc: DAY,
                items: &items,
                link_hints: &[],
                now: NOW,
            })
            .unwrap();
        // Tier A relinks the entry to the new id and retires the old ref.
        let edited = vec![fact("a_999999", "Priya", "send the revised deck v2")];
        ledger
            .sync_note_items(&NoteSync {
                note_id: "n_a1b2c3",
                project: "Briarwood Golf",
                note_date_utc: DAY,
                items: &edited,
                link_hints: &[],
                now: NOW,
            })
            .unwrap();

        let details = ledger.list_details(&Default::default()).unwrap();
        let assembled = assemble_note_items(
            "n_a1b2c3",
            vec![
                row("a_999999", "Priya", "send the revised deck v2", None, false),
                row("a_111111", "Priya", "send the revised deck", None, false),
            ],
            &details,
        );

        assert_eq!(assembled[0].tracking, ItemTracking::Tracked);
        // A retired ref is the history of a line that was edited away; matching
        // it would claim a line is tracked by an entry that moved on.
        assert_eq!(assembled[1].tracking, ItemTracking::NotEnrolled);
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

        let assembled = assemble(details, &notes, today(), AgingConfig::default());

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
                link_hints: &[],
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
        let assembled = assemble(vec![retired], &notes, today(), AgingConfig::default());
        assert!(assembled[0].item.is_none(), "a retired ref is history");
        assert!(assembled[0].source.is_none());
    }

    #[test]
    fn a_missing_index_row_degrades_to_cached_text() {
        let (ledger, _) = seeded();
        let details = ledger.list_details(&Default::default()).unwrap();

        let assembled = assemble(details, &HashMap::new(), today(), AgingConfig::default());

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

        let assembled = assemble(details.clone(), &notes, today(), AgingConfig::default());
        assert_eq!(
            assembled[0].item.as_ref().unwrap().status,
            ActionItemStatus::Overdue
        );

        // The same data read a week earlier is not overdue.
        let earlier = NaiveDate::from_ymd_opt(2026, 8, 10).unwrap();
        let assembled = assemble(details, &notes, earlier, AgingConfig::default());
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

        let assembled = assemble(details, &notes, today(), AgingConfig::default());

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
            let assembled = assemble(details, &HashMap::new(), today(), AgingConfig::default());
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

        let assembled = assemble(details, &HashMap::new(), today(), AgingConfig::default());
        assert!(assembled[0].snooze_lapsed);
    }

    #[test]
    fn only_a_snoozed_entry_can_lapse() {
        let (mut ledger, entry_id) = seeded();
        ledger.close(&entry_id, ClosedVia::Manual, NOW).unwrap();
        let details = ledger.list_details(&Default::default()).unwrap();

        let assembled = assemble(details, &HashMap::new(), today(), AgingConfig::default());
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

    /// `today()` is 2026-08-17, so an anchor N days back is `17 - N` in August.
    fn days_ago(days: u32) -> String {
        let day = today() - chrono::Duration::days(i64::from(days));
        format!("{}T09:00:00Z", day.format("%Y-%m-%d"))
    }

    #[test]
    fn tiers_fall_on_their_configured_boundaries() {
        let config = AgingConfig::default();
        let tier = |days: u32| AgingTier::derive(&days_ago(days), None, today(), config);

        // Fresh right up to the boundary, which is itself aging: "after 14
        // days" means the fourteenth day already counts.
        assert_eq!(tier(0), AgingTier::Fresh);
        assert_eq!(tier(13), AgingTier::Fresh);
        assert_eq!(tier(14), AgingTier::Aging);
        assert_eq!(tier(29), AgingTier::Aging);
        assert_eq!(tier(30), AgingTier::Stale);
        assert_eq!(tier(365), AgingTier::Stale);
    }

    #[test]
    fn an_evidence_check_counts_as_touching_the_entry() {
        let config = AgingConfig::default();
        let stale_mention = days_ago(40);

        // Nobody has said it out loud in forty days, but something checked it
        // yesterday, so it has not gone quiet.
        assert_eq!(
            AgingTier::derive(&stale_mention, Some(&days_ago(1)), today(), config),
            AgingTier::Fresh
        );
        // An older check never drags a recent mention backwards: the anchor is
        // the later of the two, not the evidence one when it exists.
        assert_eq!(
            AgingTier::derive(&days_ago(1), Some(&days_ago(40)), today(), config),
            AgingTier::Fresh
        );
        // With no check at all the mention stands alone.
        assert_eq!(
            AgingTier::derive(&stale_mention, None, today(), config),
            AgingTier::Stale
        );
    }

    #[test]
    fn an_unreadable_anchor_reads_as_stale_and_a_future_one_as_fresh() {
        let config = AgingConfig::default();
        assert_eq!(
            AgingTier::derive("not a timestamp", None, today(), config),
            AgingTier::Stale
        );
        assert_eq!(
            AgingTier::derive("", None, today(), config),
            AgingTier::Stale
        );
        // A note dated tomorrow is odd, but it is not evidence of neglect.
        assert_eq!(
            AgingTier::derive("2026-09-01T00:00:00Z", None, today(), config),
            AgingTier::Fresh
        );
    }

    #[test]
    fn an_inverted_config_collapses_to_one_boundary_rather_than_a_dead_tier() {
        let config = AgingConfig {
            aging_after_days: 30,
            stale_after_days: 7,
        };
        // Below the aging threshold nothing has happened yet...
        assert_eq!(
            AgingTier::derive(&days_ago(20), None, today(), config),
            AgingTier::Fresh
        );
        // ...and at it the entry goes straight to stale, because a stale
        // threshold under the aging one cannot mean anything else.
        assert_eq!(
            AgingTier::derive(&days_ago(30), None, today(), config),
            AgingTier::Stale
        );
    }

    #[test]
    fn assemble_stamps_a_tier_against_the_supplied_today() {
        let (ledger, _) = seeded();
        let details = ledger.list_details(&Default::default()).unwrap();
        let notes = HashMap::new();

        // The entry was mentioned on `DAY` (2026-08-17).
        let fresh = assemble(details.clone(), &notes, today(), AgingConfig::default());
        assert_eq!(fresh[0].tier, AgingTier::Fresh);

        // Same entry, a clock two months on: the ledger did not change, the
        // day did.
        let later = NaiveDate::from_ymd_opt(2026, 10, 17).unwrap();
        let stale = assemble(details, &notes, later, AgingConfig::default());
        assert_eq!(stale[0].tier, AgingTier::Stale);
    }

    #[test]
    fn a_snoozed_entry_is_tiered_like_any_other() {
        let (mut ledger, entry_id) = seeded();
        ledger.snooze(&entry_id, "2026-12-01", NOW).unwrap();
        let details = ledger.list_details(&Default::default()).unwrap();

        // Shelved is not the same as touched: the tier keeps running so a
        // snooze that lapses months later rejoins the live work reading
        // honestly rather than looking freshly minted.
        let later = NaiveDate::from_ymd_opt(2026, 12, 1).unwrap();
        let assembled = assemble(details, &HashMap::new(), later, AgingConfig::default());
        assert_eq!(assembled[0].tier, AgingTier::Stale);
        assert!(assembled[0].snooze_lapsed);
    }
}
