//! Per-meeting tracking overrides, retro-application, and manual promotion.
//!
//! [`sync`](super::sync) enforces the enrollment gate on the way *in*, deciding
//! what earns an entry. This module owns the two things it cannot: changing a
//! meeting's mind after the fact, and letting a person overrule the mode for one
//! line.
//!
//! ## What retro-application may and may not touch
//!
//! Flipping a meeting's tracking is setting a **default**, and a default never
//! overrules a person. So the re-evaluation walks only entries that are all of:
//!
//! * still in a live state (never a closure, a waiver, or a supersede — those
//!   are real judgements about the commitment),
//! * `touched = 0` (nobody has acted on it),
//! * `enrolled_via = 'default'` in the untracking direction (a manual promote
//!   said "track this one anyway", which outranks the meeting's mode), and
//! * `untracked_via = 'override'` in the retracking direction (a person's own
//!   untrack survives the meeting being re-tracked).
//!
//! The asymmetry is deliberate: each direction only undoes what an override did.
//!
//! The untracking direction adds one more: **this meeting has to be the entry's
//! only live source.** A commitment restated across meetings holds an active ref
//! in each of them, and a meeting attended for context has no standing to
//! untrack what another meeting is carrying.
//!
//! ## Why only one direction needs code
//!
//! Untracking has to be applied here, because the entries already exist and
//! sync's create leg will never see them again. Re-tracking is *half* free —
//! this module revives what the override untracked, and the shell follows the
//! flip with an ordinary re-sync, whose idempotent create leg enrolls the items
//! that never got an entry at all. Nothing here needs to know which items those
//! were.

use rusqlite::params;

use super::sync::{insert_open_entry, note_override, NewEntry};
use super::{
    Direction, EnrolledVia, EnrollmentMode, EntryState, Ledger, LedgerEntry, Result, UntrackedVia,
};
use crate::meeting::ActionItemFact;

/// What flipping one meeting's tracking did to the entries it had already
/// produced.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct NoteTrackingOutcome {
    /// The mode the note now carries.
    pub context_only: bool,
    /// Entries the flip removed from the working set.
    pub untracked: Vec<String>,
    /// Entries the flip put back, having previously untracked them.
    pub retracked: Vec<String>,
}

impl Ledger {
    /// The tracking override this note carries, if it has set one.
    pub fn note_tracking_override(&self, note_id: &str) -> Result<Option<EnrollmentMode>> {
        note_override(&self.conn, note_id)
    }

    /// Sets (or clears) a meeting's tracking override and re-evaluates the
    /// entries it already produced.
    ///
    /// `context_only = false` **deletes** the row rather than storing
    /// `'tracked'`: absence is the default, so a note that never opted in and
    /// one that opted back out should be indistinguishable. The column still
    /// admits `'tracked'` because meeting categories will need a note to say
    /// "tracked, whatever my category defaults to".
    ///
    /// Idempotent in both directions: setting the mode a note already has finds
    /// nothing left to change.
    pub fn set_note_tracking(
        &mut self,
        note_id: &str,
        project: &str,
        context_only: bool,
        now: &str,
    ) -> Result<NoteTrackingOutcome> {
        let mut outcome = NoteTrackingOutcome {
            context_only,
            ..Default::default()
        };

        if context_only {
            self.conn.execute(
                "INSERT INTO ledger_note_overrides (note_id, project, mode, set_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (note_id)
                 DO UPDATE SET project = ?2, mode = ?3, set_at = ?4",
                params![note_id, project, EnrollmentMode::ContextOnly.as_str(), now],
            )?;
            for entry_id in self.overridable_entries(note_id)? {
                self.untrack(&entry_id, UntrackedVia::Override, now)?;
                outcome.untracked.push(entry_id);
            }
        } else {
            self.conn.execute(
                "DELETE FROM ledger_note_overrides WHERE note_id = ?1",
                [note_id],
            )?;
            for entry_id in self.override_untracked_entries(note_id)? {
                self.reopen(&entry_id, now)?;
                outcome.retracked.push(entry_id);
            }
        }

        if !project.is_empty() {
            self.mark_dirty(project);
        }
        Ok(outcome)
    }

    /// Entries in this note that a context-only flip may untrack.
    ///
    /// [`Direction::Mine`] is excluded because that is what context-only
    /// *means*: a direct ask is a commitment regardless of why you attended.
    ///
    /// **This meeting must be the entry's only live source.** A cross-note
    /// re-mention leaves an active ref in every meeting that restated the
    /// commitment, so without the second `NOT EXISTS` a context-only all-hands
    /// would untrack a commitment a tracked one-to-one is carrying, purely for
    /// having been mentioned in the wrong room. The mirror query needs no such
    /// guard: an untracked entry is skipped by `match_live_entry`, so it can
    /// never pick up a ref in a second note after the fact.
    fn overridable_entries(&self, note_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT entry_id FROM ledger_entries
             WHERE state IN ('open', 'needs_review', 'snoozed')
               AND touched = 0
               AND enrolled_via = 'default'
               AND direction <> 'mine'
               AND EXISTS (SELECT 1 FROM ledger_item_refs
                           WHERE ledger_item_refs.entry_id = ledger_entries.entry_id
                             AND note_id = ?1 AND active = 1)
               AND NOT EXISTS (SELECT 1 FROM ledger_item_refs
                               WHERE ledger_item_refs.entry_id = ledger_entries.entry_id
                                 AND note_id <> ?1 AND active = 1)
             ORDER BY entry_id",
        )?;
        let rows = stmt.query_map([note_id], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Entries in this note that an override untracked and may now be revived.
    fn override_untracked_entries(&self, note_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT entry_id FROM ledger_entries
             WHERE state = 'untracked'
               AND untracked_via = 'override'
               AND touched = 0
               AND EXISTS (SELECT 1 FROM ledger_item_refs
                           WHERE ledger_item_refs.entry_id = ledger_entries.entry_id
                             AND note_id = ?1 AND active = 1)
             ORDER BY entry_id",
        )?;
        let rows = stmt.query_map([note_id], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Tracks one extracted line by hand, whatever the meeting's mode says.
    ///
    /// Three cases, and the interesting one is the middle:
    ///
    /// * **No entry** — mint one, `enrolled_via = 'manual'`.
    /// * **Untracked** — reopen it *and* rewrite `enrolled_via` to `'manual'`.
    ///   That is the one place enrollment provenance is ever restated, and it
    ///   earns the exception: the newest true answer to "why are you in my
    ///   ledger" is now "because I said so", and it is also what stops the next
    ///   context-only flip from quietly untracking it again.
    /// * **Anything else** — already tracked, or settled by a real judgement.
    ///   Returned unchanged, so the call is idempotent and a stale UI cannot
    ///   resurrect a closed commitment.
    pub fn track_item(
        &mut self,
        note_id: &str,
        item: &ActionItemFact,
        project: &str,
        note_date_utc: &str,
        now: &str,
    ) -> Result<LedgerEntry> {
        if let Some(existing) = self.entry_for_item(note_id, item.id.as_str())? {
            if existing.state != EntryState::Untracked {
                return Ok(existing);
            }
            let entry = self.reopen(&existing.entry_id, now)?;
            self.conn.execute(
                "UPDATE ledger_entries SET enrolled_via = ?2 WHERE entry_id = ?1",
                params![entry.entry_id, EnrolledVia::Manual.as_str()],
            )?;
            self.mark_dirty(&entry.project);
            return self.reread(&entry.entry_id);
        }

        let direction = Direction::from_owner(&item.owner);
        let entry_id = {
            let tx = self.connection_mut().transaction()?;
            let entry_id = insert_open_entry(
                &tx,
                NewEntry {
                    item,
                    direction,
                    project,
                    last_mention: note_date_utc,
                    enrolled_via: EnrolledVia::Manual,
                    now,
                },
            )?;
            tx.execute(
                "INSERT INTO ledger_item_refs (entry_id, item_id, note_id, active, linked_at)
                 VALUES (?1, ?2, ?3, 1, ?4)
                 ON CONFLICT (entry_id, note_id, item_id)
                 DO UPDATE SET active = 1, retired_at = NULL, linked_at = ?4",
                params![entry_id, item.id, note_id, now],
            )?;
            tx.commit()?;
            entry_id
        };
        self.mark_dirty(project);
        self.reread(&entry_id)
    }

    /// Re-reads an entry that must exist, for a mutator returning its new shape.
    fn reread(&self, entry_id: &str) -> Result<LedgerEntry> {
        self.get_entry(entry_id)?
            .map(|detail| detail.entry)
            .ok_or_else(|| super::LedgerError::EntryNotFound {
                entry_id: entry_id.to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{ClosedVia, EntryFilter, NoteSync};

    const NOW: &str = "2026-08-17T12:00:00Z";
    const LATER: &str = "2026-08-18T09:00:00Z";
    const DAY_ONE: &str = "2026-08-01T00:00:00Z";
    const PROJECT: &str = "Briarwood Golf";
    const NOTE: &str = "n_a1b2c3";

    fn fact(id: &str, owner: &str, description: &str) -> ActionItemFact {
        ActionItemFact {
            id: id.to_string(),
            description: description.to_string(),
            owner: owner.to_string(),
            due_date: None,
            done: false,
            extracted_date: Some("2026-08-01".to_string()),
        }
    }

    /// A note with one item of each direction, already synced and tracked.
    fn seeded() -> (Ledger, Vec<ActionItemFact>) {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![
            fact("a_111111", "Priya", "send the revised deck"),
            fact("a_222222", "You", "book the venue"),
            fact("a_333333", "Unassigned", "chase the caterer"),
        ];
        ledger
            .sync_note_items(&NoteSync {
                note_id: NOTE,
                project: PROJECT,
                note_date_utc: DAY_ONE,
                items: &items,
                link_hints: &[],
                now: NOW,
            })
            .unwrap();
        (ledger, items)
    }

    fn entry_for(ledger: &Ledger, item_id: &str) -> LedgerEntry {
        ledger.entry_for_item(NOTE, item_id).unwrap().unwrap()
    }

    #[test]
    fn context_only_untracks_what_is_not_mine_and_spares_the_rest() {
        let (mut ledger, _) = seeded();

        let outcome = ledger
            .set_note_tracking(NOTE, PROJECT, true, LATER)
            .unwrap();

        assert!(outcome.context_only);
        assert_eq!(outcome.untracked.len(), 2);
        assert!(outcome.retracked.is_empty());

        assert_eq!(entry_for(&ledger, "a_111111").state, EntryState::Untracked);
        assert_eq!(entry_for(&ledger, "a_333333").state, EntryState::Untracked);
        // The direct ask stays: a commitment regardless of why you attended.
        assert_eq!(entry_for(&ledger, "a_222222").state, EntryState::Open);

        // Provenance says an override did this, which is what lets the flip back
        // tell it apart from a person's own untrack.
        assert_eq!(
            entry_for(&ledger, "a_111111").untracked_via,
            Some(UntrackedVia::Override)
        );
    }

    #[test]
    fn flipping_back_revives_exactly_what_the_override_untracked() {
        let (mut ledger, _) = seeded();
        ledger
            .set_note_tracking(NOTE, PROJECT, true, LATER)
            .unwrap();

        let outcome = ledger
            .set_note_tracking(NOTE, PROJECT, false, LATER)
            .unwrap();

        assert!(!outcome.context_only);
        assert_eq!(outcome.retracked.len(), 2);
        for item_id in ["a_111111", "a_222222", "a_333333"] {
            let entry = entry_for(&ledger, item_id);
            assert_eq!(entry.state, EntryState::Open, "{item_id} should be live");
            assert_eq!(entry.untracked_via, None, "{item_id} keeps no stale trace");
        }
        assert_eq!(ledger.note_tracking_override(NOTE).unwrap(), None);
    }

    #[test]
    fn a_flip_spares_a_commitment_another_meeting_is_also_carrying() {
        let (mut ledger, _) = seeded();
        // The same commitment restated in a second meeting: tier 4 re-mentions
        // the entry and leaves a second active ref behind, in a note that never
        // produced it.
        let restated = vec![fact("a_444444", "Priya", "send the revised deck")];
        ledger
            .sync_note_items(&NoteSync {
                note_id: "n_d4e5f6",
                project: PROJECT,
                note_date_utc: DAY_ONE,
                items: &restated,
                link_hints: &[],
                now: NOW,
            })
            .unwrap();

        let outcome = ledger
            .set_note_tracking("n_d4e5f6", PROJECT, true, LATER)
            .unwrap();

        assert!(
            outcome.untracked.is_empty(),
            "a context-only meeting decides what it enrolls, never what              another meeting is already carrying"
        );
        assert_eq!(entry_for(&ledger, "a_111111").state, EntryState::Open);
    }

    #[test]
    fn retro_application_never_overrides_an_entry_a_person_touched() {
        let (mut ledger, _) = seeded();
        // Anything the shell's mutation path did sets this flag; here it stands
        // in for "the user snoozed it", "waived it", "judged its evidence".
        let touched = entry_for(&ledger, "a_111111").entry_id;
        ledger.mark_touched(&touched).unwrap();

        let outcome = ledger
            .set_note_tracking(NOTE, PROJECT, true, LATER)
            .unwrap();

        assert_eq!(
            outcome.untracked,
            vec![entry_for(&ledger, "a_333333").entry_id]
        );
        assert_eq!(
            entry_for(&ledger, "a_111111").state,
            EntryState::Open,
            "a default never overrules someone who already looked"
        );
    }

    #[test]
    fn retro_application_never_touches_a_settled_entry() {
        let (mut ledger, _) = seeded();
        let closed = entry_for(&ledger, "a_111111").entry_id;
        ledger.close(&closed, ClosedVia::Manual, NOW).unwrap();
        let waived = entry_for(&ledger, "a_333333").entry_id;
        ledger.waive(&waived, NOW).unwrap();

        let outcome = ledger
            .set_note_tracking(NOTE, PROJECT, true, LATER)
            .unwrap();

        assert!(outcome.untracked.is_empty());
        assert_eq!(entry_for(&ledger, "a_111111").state, EntryState::Closed);
        assert_eq!(entry_for(&ledger, "a_333333").state, EntryState::Waived);
    }

    #[test]
    fn a_manual_untrack_survives_the_meeting_being_re_tracked() {
        let (mut ledger, _) = seeded();
        let mine = entry_for(&ledger, "a_222222").entry_id;
        ledger.untrack(&mine, UntrackedVia::Manual, NOW).unwrap();

        ledger
            .set_note_tracking(NOTE, PROJECT, true, LATER)
            .unwrap();
        let outcome = ledger
            .set_note_tracking(NOTE, PROJECT, false, LATER)
            .unwrap();

        assert!(
            !outcome.retracked.contains(&mine),
            "each direction only undoes what an override did"
        );
        assert_eq!(entry_for(&ledger, "a_222222").state, EntryState::Untracked);
    }

    #[test]
    fn a_manually_promoted_entry_is_not_swept_up_by_a_later_flip() {
        let (mut ledger, items) = seeded();
        // Untrack by hand, then track it back: that is a manual promote, and it
        // outranks the meeting's mode from then on.
        let theirs = entry_for(&ledger, "a_111111").entry_id;
        ledger.untrack(&theirs, UntrackedVia::Manual, NOW).unwrap();
        let promoted = ledger
            .track_item(NOTE, &items[0], PROJECT, DAY_ONE, LATER)
            .unwrap();
        assert_eq!(promoted.enrolled_via, EnrolledVia::Manual);
        assert_eq!(promoted.state, EntryState::Open);

        let outcome = ledger
            .set_note_tracking(NOTE, PROJECT, true, LATER)
            .unwrap();

        assert!(!outcome.untracked.contains(&theirs));
        assert_eq!(entry_for(&ledger, "a_111111").state, EntryState::Open);
    }

    #[test]
    fn setting_the_same_mode_twice_changes_nothing() {
        let (mut ledger, _) = seeded();
        let first = ledger
            .set_note_tracking(NOTE, PROJECT, true, LATER)
            .unwrap();
        let second = ledger
            .set_note_tracking(NOTE, PROJECT, true, LATER)
            .unwrap();

        assert_eq!(first.untracked.len(), 2);
        assert!(second.untracked.is_empty(), "idempotent in both directions");
        assert!(second.retracked.is_empty());
    }

    #[test]
    fn flipping_back_then_re_syncing_enrolls_what_was_never_created() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![
            fact("a_111111", "Priya", "send the revised deck"),
            fact("a_222222", "You", "book the venue"),
        ];
        ledger.set_note_tracking(NOTE, PROJECT, true, NOW).unwrap();
        let sync = |ledger: &mut Ledger| {
            ledger
                .sync_note_items(&NoteSync {
                    note_id: NOTE,
                    project: PROJECT,
                    note_date_utc: DAY_ONE,
                    items: &items,
                    link_hints: &[],
                    now: NOW,
                })
                .unwrap()
        };
        let gated = sync(&mut ledger);
        assert_eq!(gated.not_enrolled, 1);
        assert_eq!(gated.created.len(), 1);

        // The other half of retro-application: this module revives what it
        // untracked, and the shell's follow-up sync creates what never existed.
        ledger
            .set_note_tracking(NOTE, PROJECT, false, LATER)
            .unwrap();
        let after = sync(&mut ledger);

        assert_eq!(after.created.len(), 1, "the gated item enrolls now");
        assert_eq!(after.not_enrolled, 0);
        assert_eq!(
            ledger.list_entries(&EntryFilter::default()).unwrap().len(),
            2
        );
        assert_eq!(
            entry_for(&ledger, "a_111111").enrolled_via,
            EnrolledVia::Default
        );
    }

    #[test]
    fn tracking_an_item_with_no_entry_mints_one_marked_manual() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        ledger.set_note_tracking(NOTE, PROJECT, true, NOW).unwrap();
        let item = fact("a_111111", "Priya", "send the revised deck");
        ledger
            .sync_note_items(&NoteSync {
                note_id: NOTE,
                project: PROJECT,
                note_date_utc: DAY_ONE,
                items: std::slice::from_ref(&item),
                link_hints: &[],
                now: NOW,
            })
            .unwrap();
        assert!(ledger.entry_for_item(NOTE, "a_111111").unwrap().is_none());

        let entry = ledger
            .track_item(NOTE, &item, PROJECT, DAY_ONE, LATER)
            .unwrap();

        assert_eq!(entry.state, EntryState::Open);
        assert_eq!(entry.enrolled_via, EnrolledVia::Manual);
        assert_eq!(entry.direction, Direction::Theirs);
        assert_eq!(
            entry.last_mention, DAY_ONE,
            "the note's date, not the clock"
        );
        assert_eq!(entry.project, PROJECT);
    }

    #[test]
    fn tracking_an_already_tracked_item_is_idempotent() {
        let (mut ledger, items) = seeded();
        let before = entry_for(&ledger, "a_111111");

        let after = ledger
            .track_item(NOTE, &items[0], PROJECT, DAY_ONE, LATER)
            .unwrap();

        assert_eq!(after, before, "a live entry is returned untouched");
        assert_eq!(
            ledger.list_entries(&EntryFilter::default()).unwrap().len(),
            3
        );
    }

    #[test]
    fn tracking_a_settled_item_never_resurrects_it() {
        let (mut ledger, items) = seeded();
        let closed = entry_for(&ledger, "a_111111").entry_id;
        ledger.close(&closed, ClosedVia::Manual, NOW).unwrap();

        let after = ledger
            .track_item(NOTE, &items[0], PROJECT, DAY_ONE, LATER)
            .unwrap();

        assert_eq!(
            after.state,
            EntryState::Closed,
            "a stale note view must not undo a real judgement"
        );
    }

    #[test]
    fn an_override_marks_its_project_dirty_for_the_snapshot() {
        let (mut ledger, _) = seeded();
        ledger.clear_all_dirty();

        ledger
            .set_note_tracking(NOTE, PROJECT, true, LATER)
            .unwrap();

        assert_eq!(ledger.dirty_projects(), vec![PROJECT.to_string()]);
    }
}
