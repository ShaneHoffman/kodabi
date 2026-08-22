//! Retro-application of a changed enrollment mode, and manual promotion.
//!
//! [`sync`](super::sync) enforces the enrollment gate on the way *in*, deciding
//! what earns an entry. This module owns the two things it cannot: changing a
//! meeting's mind after the fact, and letting a person overrule the mode for one
//! line.
//!
//! **Neither half of the mode is stored in this crate's database.** The
//! per-meeting override is a frontmatter key on the note
//! ([`crate::vault::set_note_tracking`]), so it travels with the file; the
//! meeting category's default is a user setting, reached through
//! [`category_default_for`](super::category_default_for). What lives here is the
//! *consequence* — the entries a previous mode already minted, which no re-sync
//! revisits on its own. The shell writes the note first and calls
//! [`Ledger::retro_apply_note_tracking`] after, naming which source changed its
//! mind ([`RetroSource`]).
//!
//! ## What retro-application may and may not touch
//!
//! Flipping a meeting's tracking is setting a **default**, and a default never
//! overrules a person. So the re-evaluation walks only entries that are all of:
//!
//! * still in a live state (never a closure, a waiver, or a supersede — those
//!   are real judgements about the commitment),
//! * `touched = 0` (nobody has acted on it),
//! * enrolled by a *machine* judgement in the untracking direction
//!   (`enrolled_via IN ('default', 'category')`; a manual promote said "track
//!   this one anyway", which outranks the meeting's mode), and
//! * untracked by a *machine* judgement in the retracking direction
//!   (`untracked_via IN ('override', 'category')`; a person's own untrack
//!   survives the meeting being re-tracked).
//!
//! The asymmetry is deliberate: each direction only undoes what a default did.
//! Both machine sources appear in both lists rather than each undoing only its
//! own kind, and that breadth is load-bearing: entries minted while a meeting
//! was a standup say `default`, so a recategorization into an observer genre
//! would sweep nothing if the untrack direction insisted on `category`.
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

use std::path::Path;

use rusqlite::params;

use super::sync::{insert_open_entry, NewEntry};
use super::{
    Direction, EnrolledVia, EnrollmentMode, EntryState, Ledger, LedgerEntry, Result, UntrackedVia,
};
use crate::meeting::ActionItemFact;
use crate::note::NoteId;

/// Which half of the enrollment chain changed its mind, for
/// [`Ledger::retro_apply_note_tracking`] to attribute the untracks it performs.
///
/// Deliberately two-variant: [`UntrackedVia::Manual`] is a person's own untrack
/// and can never be the reason a re-evaluation runs, so it is unrepresentable
/// here rather than merely unused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetroSource {
    /// The meeting's own `tracking:` override was set or cleared.
    Override,
    /// The meeting was recategorized, and its new genre's default decides.
    Category,
}

impl RetroSource {
    /// The provenance an untrack from this source is stamped with.
    fn untracked_via(self) -> UntrackedVia {
        match self {
            RetroSource::Override => UntrackedVia::Override,
            RetroSource::Category => UntrackedVia::Category,
        }
    }
}

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

/// What one owner re-resolution pass moved.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OwnerResolutionOutcome {
    /// Entries the new identity re-filed as the user's own.
    pub claimed: Vec<String>,
}

impl Ledger {
    /// Re-evaluates the entries a meeting already produced, after its effective
    /// enrollment mode changed — either because its `tracking:` override was
    /// flipped or because it was recategorized into a genre with a different
    /// default.
    ///
    /// **The judgement itself is not stored here.** It lives in the note's
    /// `tracking:` and `category:` keys, so it survives a re-route, a vault
    /// rebuild and a sync to another machine; this store owns only the
    /// consequence — the entries that judgement already minted, which no re-sync
    /// would revisit on its own because sync's gate decides what *earns* an
    /// entry, never what happens to one that exists.
    ///
    /// `context_only` is the **effective** mode after the change, not the raw
    /// override, so a recategorization of a meeting that pins its own override
    /// correctly re-applies the mode it already had. `source` names which half
    /// of the chain changed, and only affects how an untrack is attributed.
    ///
    /// Call it after the frontmatter write has succeeded. Idempotent in both
    /// directions: re-applying the mode a note already had finds nothing left
    /// to change.
    pub fn retro_apply_note_tracking(
        &mut self,
        note_id: &str,
        project: &str,
        context_only: bool,
        source: RetroSource,
        now: &str,
    ) -> Result<NoteTrackingOutcome> {
        let mut outcome = NoteTrackingOutcome {
            context_only,
            ..Default::default()
        };

        if context_only {
            for entry_id in self.overridable_entries(note_id)? {
                self.untrack(&entry_id, source.untracked_via(), now)?;
                outcome.untracked.push(entry_id);
            }
        } else {
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

    /// Every tracking override still parked in the legacy `ledger_note_overrides`
    /// table, as `(note_id, mode)` — the startup drain's input.
    ///
    /// The table predates the frontmatter `tracking:` key and is no longer
    /// written; see [`crate::ledger::migrations`]. Rows only ever leave it.
    pub fn legacy_note_overrides(&self) -> Result<Vec<(String, EnrollmentMode)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT note_id, mode FROM ledger_note_overrides ORDER BY note_id")?;
        let rows = stmt
            .query_map([], |row| {
                let note_id: String = row.get(0)?;
                let mode: String = row.get(1)?;
                Ok((note_id, mode))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(note_id, mode)| Ok((note_id, EnrollmentMode::parse(&mode)?)))
            .collect()
    }

    /// Drops one drained row from the legacy override table. Returns whether a
    /// row was there to drop.
    pub fn forget_legacy_note_override(&mut self, note_id: &str) -> Result<bool> {
        let removed = self.conn.execute(
            "DELETE FROM ledger_note_overrides WHERE note_id = ?1",
            [note_id],
        )?;
        Ok(removed > 0)
    }

    /// Entries in this note that a context-only mode may untrack.
    ///
    /// [`Direction::Mine`] is excluded because that is what context-only
    /// *means*: a direct ask is a commitment regardless of why you attended.
    ///
    /// **Both machine provenances are in scope, not only the one that changed.**
    /// An entry minted while the meeting was a standup says `default`; one
    /// minted while it was an all-hands with a `tracked` setting says `default`
    /// too. Recategorizing into an observer genre has to reach all of them, and
    /// what it must *not* reach is `manual` — a person's promote.
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
               AND enrolled_via IN ('default', 'category')
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

    /// Entries in this note that a default untracked and may now be revived.
    ///
    /// Machine untracks only, from either source: a person's own untrack
    /// (`manual`) is a judgement about the commitment and outlives any change of
    /// mode.
    fn override_untracked_entries(&self, note_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT entry_id FROM ledger_entries
             WHERE state = 'untracked'
               AND untracked_via IN ('override', 'category')
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
        identity: &crate::ledger::OwnerIdentity,
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

        let direction = Direction::resolve(&item.owner, identity);
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

    /// Re-evaluates which open commitments are the user's own, after the
    /// configured identity changed.
    ///
    /// The mirror of [`Ledger::retro_apply_note_tracking`], and bound by the
    /// same rule: learning a name is setting a **default**, and a default never
    /// overrules a person. So this walks only entries that are in a live state
    /// and `touched = 0`. There is no `enrolled_via` filter here, and none is
    /// needed — every human act on an entry, including a manual promote, marks
    /// it touched, so that one column already excludes everything a person
    /// decided.
    ///
    /// **It only ever moves entries *toward* [`Direction::Mine`].** Removing an
    /// alias re-files nothing, and that asymmetry is the same one the enrolment
    /// gate's observer rule rests on: a stray in Waiting-on-them is one click to
    /// fix, while silently dropping a commitment out of Mine is the failure the
    /// user would never see coming. `unassigned` is excluded for a different
    /// reason — by construction only the grammar's own `Unassigned` token
    /// resolves there, so no alias can match it.
    ///
    /// **What it cannot reach:** items a context-only meeting gated out have no
    /// row at all ([`sync`](super::sync)'s create leg), so no sweep can find
    /// them; they enrol when their note is next re-synced. Re-forwarding the
    /// whole vault on every alias edit would be a far larger action than the
    /// control looks like, so it is deliberately not done, and the per-line
    /// promote remains the immediate path for one that matters.
    pub fn retro_resolve_owners(
        &mut self,
        identity: &super::OwnerIdentity,
        now: &str,
    ) -> Result<OwnerResolutionOutcome> {
        let mut outcome = OwnerResolutionOutcome::default();
        if identity.is_empty() {
            return Ok(outcome);
        }

        for entry_id in self.unresolved_entries(identity)? {
            self.claim_mine(&entry_id, now)?;
            outcome.claimed.push(entry_id);
        }
        Ok(outcome)
    }

    /// Live, untouched, them-side entries whose owner is one of the user's
    /// names.
    ///
    /// Matched on `owner_norm`, the column [`sync`](super::sync) wrote with the
    /// same normalization [`super::OwnerIdentity`] applies to the alias set, so
    /// a name stored months ago and a name typed just now fold identically.
    fn unresolved_entries(&self, identity: &super::OwnerIdentity) -> Result<Vec<String>> {
        let aliases = identity.normalized_aliases();
        let placeholders = std::iter::repeat_n("?", aliases.len())
            .collect::<Vec<_>>()
            .join(", ");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT entry_id FROM ledger_entries
             WHERE state IN ('open', 'needs_review', 'snoozed')
               AND touched = 0
               AND direction = 'theirs'
               AND owner_norm IN ({placeholders})
             ORDER BY entry_id"
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(aliases), |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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

/// What one drain pass did, for the caller's log line.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DrainOutcome {
    /// Overrides written into their notes' frontmatter.
    pub graduated: usize,
    /// Rows dropped without a write: the note is gone, or already says it.
    pub discarded: usize,
    /// Rows left for the next launch because something failed.
    pub deferred: usize,
}

impl Ledger {
    /// Graduates every tracking override still parked in the legacy
    /// `ledger_note_overrides` table into its note's frontmatter, then drops
    /// the row.
    ///
    /// The override used to live in `ledger.db`; it now lives in the note,
    /// where it travels with the file. A migration could not perform this move
    /// — migrations run inside the database and cannot reach the vault — and
    /// this store's doctrine forbids dropping a row it cannot carry forward, so
    /// the graduation happens here, where the vault and the ledger are both in
    /// hand. The shell calls it once at startup, behind the snapshot restore
    /// (which is the only thing that still *adds* rows to that table).
    ///
    /// Per note:
    ///
    /// * a note that has vanished loses its row — nothing is left to carry it;
    /// * a note that already carries a `tracking:` key keeps it, because
    ///   frontmatter is the truth now and a stale row must not overwrite a
    ///   later, deliberate flip;
    /// * anything else is written **and then retro-applied**, because the
    ///   window between this launch and the write is one in which a sync may
    ///   already have enrolled items the override should have gated.
    ///
    /// A note that fails keeps its row and is retried next launch. Rows only
    /// ever leave this table, so a repeat run converges and a second run over
    /// an already-drained vault writes nothing.
    pub fn drain_legacy_note_overrides(
        &mut self,
        vault_root: &Path,
        now: &str,
    ) -> Result<DrainOutcome> {
        let mut outcome = DrainOutcome::default();
        let rows = self.legacy_note_overrides()?;
        for (note_id, mode) in rows {
            let Ok(parsed) = NoteId::parse(&note_id) else {
                // An id the note layer will never match: nothing can carry it.
                self.forget_legacy_note_override(&note_id)?;
                outcome.discarded += 1;
                continue;
            };
            let found = match crate::vault::find_note_anywhere(vault_root, &parsed) {
                Ok(found) => found,
                Err(err) => {
                    eprintln!(
                        "ledger: couldn't look up note {note_id} to graduate its tracking: {err}"
                    );
                    outcome.deferred += 1;
                    continue;
                }
            };
            let Some((project, listed)) = found else {
                self.forget_legacy_note_override(&note_id)?;
                outcome.discarded += 1;
                continue;
            };
            if listed.note.tracking.is_some() {
                self.forget_legacy_note_override(&note_id)?;
                outcome.discarded += 1;
                continue;
            }
            if let Err(err) = crate::vault::set_note_tracking(vault_root, &parsed, Some(mode)) {
                eprintln!("ledger: couldn't write tracking into note {note_id}: {err}");
                outcome.deferred += 1;
                continue;
            }
            if let Err(err) = self.retro_apply_note_tracking(
                &note_id,
                &project,
                mode == EnrollmentMode::ContextOnly,
                // A drained row *is* a per-meeting override, whatever the note's
                // category would have said on its own.
                RetroSource::Override,
                now,
            ) {
                eprintln!("ledger: couldn't re-check note {note_id}'s commitments: {err}");
                outcome.deferred += 1;
                continue;
            }
            self.forget_legacy_note_override(&note_id)?;
            outcome.graduated += 1;
        }
        Ok(outcome)
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
            firm: true,
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
                note_override: None,
                category_default: None,
                identity: &crate::ledger::OwnerIdentity::default(),
                now: NOW,
            })
            .unwrap();
        (ledger, items)
    }

    fn entry_for(ledger: &Ledger, item_id: &str) -> LedgerEntry {
        ledger.entry_for_item(NOTE, item_id).unwrap().unwrap()
    }

    /// The sanctioned exception to the firmness gate: the note rail's Track
    /// button. `track_item` decides on the person's say-so alone and never
    /// consults firmness, which is what keeps "not tracked" from meaning "not
    /// trackable" — and stamps the entry manual, so a later sweep leaves it be.
    #[test]
    fn a_soft_item_can_still_be_tracked_by_hand() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let item = ActionItemFact {
            firm: false,
            ..fact("a_111111", "You", "look into pricing sometime")
        };
        ledger
            .sync_note_items(&NoteSync {
                note_id: NOTE,
                project: PROJECT,
                note_date_utc: DAY_ONE,
                items: std::slice::from_ref(&item),
                link_hints: &[],
                note_override: None,
                category_default: None,
                identity: &crate::ledger::OwnerIdentity::default(),
                now: NOW,
            })
            .unwrap();
        assert!(
            ledger.entry_for_item(NOTE, "a_111111").unwrap().is_none(),
            "the gate declined it on the way in"
        );

        let entry = ledger
            .track_item(
                NOTE,
                &item,
                PROJECT,
                DAY_ONE,
                &crate::ledger::OwnerIdentity::default(),
                LATER,
            )
            .unwrap();

        assert_eq!(entry.state, EntryState::Open);
        assert_eq!(entry.enrolled_via, EnrolledVia::Manual);
    }

    /// The same note, synced by a user who has told the app their name.
    fn seeded_as(identity: &crate::ledger::OwnerIdentity) -> (Ledger, Vec<ActionItemFact>) {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![
            fact("a_111111", "Priya", "send the revised deck"),
            fact("a_444444", "Avery", "circulate the minutes"),
        ];
        ledger
            .sync_note_items(&NoteSync {
                note_id: NOTE,
                project: PROJECT,
                note_date_utc: DAY_ONE,
                items: &items,
                link_hints: &[],
                note_override: None,
                category_default: None,
                identity,
                now: NOW,
            })
            .unwrap();
        (ledger, items)
    }

    #[test]
    fn a_configured_name_enrols_as_the_users_own_commitment() {
        let identity = crate::ledger::OwnerIdentity::new("Avery", &[]);
        let (ledger, _) = seeded_as(&identity);

        assert_eq!(entry_for(&ledger, "a_444444").direction, Direction::Mine);
        assert_eq!(entry_for(&ledger, "a_111111").direction, Direction::Theirs);
    }

    #[test]
    fn a_context_only_meeting_still_enrols_the_users_own_commitment_by_name() {
        // The headline failure this feature exists for: under ContextOnly the
        // direction *is* the gate, so an owner spelled as a name rather than
        // "You" used to produce no row at all.
        let identity = crate::ledger::OwnerIdentity::new("Avery", &[]);
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![
            fact("a_444444", "Avery", "circulate the minutes"),
            fact("a_111111", "Priya", "send the revised deck"),
        ];
        let outcome = ledger
            .sync_note_items(&NoteSync {
                note_id: NOTE,
                project: PROJECT,
                note_date_utc: DAY_ONE,
                items: &items,
                link_hints: &[],
                note_override: Some(EnrollmentMode::ContextOnly),
                category_default: None,
                identity: &identity,
                now: NOW,
            })
            .unwrap();

        assert_eq!(outcome.created.len(), 1);
        assert_eq!(outcome.not_enrolled, 1, "the other side is still gated out");
        assert_eq!(entry_for(&ledger, "a_444444").direction, Direction::Mine);
        assert!(ledger.entry_for_item(NOTE, "a_111111").unwrap().is_none());
    }

    #[test]
    fn learning_a_name_re_files_the_commitments_already_recorded_under_it() {
        let (mut ledger, _) = seeded_as(&crate::ledger::OwnerIdentity::default());
        // Synced before the user said who they were, so their own line landed
        // on the them side.
        assert_eq!(entry_for(&ledger, "a_444444").direction, Direction::Theirs);

        let identity = crate::ledger::OwnerIdentity::new("avery", &[]);
        let outcome = ledger.retro_resolve_owners(&identity, LATER).unwrap();

        assert_eq!(outcome.claimed.len(), 1);
        assert_eq!(entry_for(&ledger, "a_444444").direction, Direction::Mine);
        assert_eq!(
            entry_for(&ledger, "a_111111").direction,
            Direction::Theirs,
            "someone else's commitment is untouched"
        );

        // Idempotent: a second pass has nothing left to move.
        let again = ledger.retro_resolve_owners(&identity, LATER).unwrap();
        assert!(again.claimed.is_empty());
    }

    #[test]
    fn re_resolution_never_overrides_an_entry_a_person_has_acted_on() {
        let (mut ledger, _) = seeded_as(&crate::ledger::OwnerIdentity::default());
        let entry_id = entry_for(&ledger, "a_444444").entry_id;
        // What the shell's mutation path does after every successful op.
        ledger.snooze(&entry_id, "2026-09-01", NOW).unwrap();
        ledger.mark_touched(&entry_id).unwrap();

        let identity = crate::ledger::OwnerIdentity::new("Avery", &[]);
        let outcome = ledger.retro_resolve_owners(&identity, LATER).unwrap();

        assert!(outcome.claimed.is_empty());
        assert_eq!(entry_for(&ledger, "a_444444").direction, Direction::Theirs);
    }

    #[test]
    fn re_resolution_leaves_settled_commitments_alone() {
        let (mut ledger, _) = seeded_as(&crate::ledger::OwnerIdentity::default());
        let entry_id = entry_for(&ledger, "a_444444").entry_id;
        // Settled by a machine, so `touched` is not what excludes it here: the
        // live-state filter is.
        ledger
            .close(&entry_id, ClosedVia::Conversation, LATER)
            .unwrap();

        let identity = crate::ledger::OwnerIdentity::new("Avery", &[]);
        let outcome = ledger.retro_resolve_owners(&identity, LATER).unwrap();

        assert!(outcome.claimed.is_empty());
        assert_eq!(entry_for(&ledger, "a_444444").state, EntryState::Closed);
        assert_eq!(entry_for(&ledger, "a_444444").direction, Direction::Theirs);
    }

    #[test]
    fn re_resolution_never_demotes_a_commitment_out_of_mine() {
        // Clearing a name must not silently drop the user's own commitments:
        // the sweep only ever moves entries toward Mine.
        let (mut ledger, _) = seeded_as(&crate::ledger::OwnerIdentity::new("Avery", &[]));
        assert_eq!(entry_for(&ledger, "a_444444").direction, Direction::Mine);

        let outcome = ledger
            .retro_resolve_owners(&crate::ledger::OwnerIdentity::new("Blake", &[]), LATER)
            .unwrap();

        assert!(outcome.claimed.is_empty());
        assert_eq!(entry_for(&ledger, "a_444444").direction, Direction::Mine);
    }

    #[test]
    fn an_unattributed_line_is_never_swept_into_mine() {
        let (mut ledger, _) = seeded();
        // Even an identity that names the sentinel cannot reach it: the row is
        // `unassigned`, and the sweep only reads `theirs`.
        let identity = crate::ledger::OwnerIdentity::new(crate::distill::UNASSIGNED_OWNER, &[]);
        let outcome = ledger.retro_resolve_owners(&identity, LATER).unwrap();

        assert!(outcome.claimed.is_empty());
        assert_eq!(
            entry_for(&ledger, "a_333333").direction,
            Direction::Unassigned
        );
    }

    #[test]
    fn claiming_an_entry_moves_only_its_direction() {
        let (mut ledger, _) = seeded_as(&crate::ledger::OwnerIdentity::default());
        let before = entry_for(&ledger, "a_111111");
        ledger.snooze(&before.entry_id, "2026-09-01", NOW).unwrap();

        let claimed = ledger.claim_mine(&before.entry_id, LATER).unwrap();

        assert_eq!(claimed.direction, Direction::Mine);
        assert_eq!(claimed.state, EntryState::Snoozed, "still snoozed");
        assert_eq!(
            claimed.owner, "Priya",
            "the note's line is what it is; the ledger's copy still matches it"
        );

        // Idempotent, and a second claim does not restamp `updated_at`.
        let again = ledger
            .claim_mine(&before.entry_id, "2026-08-19T09:00:00Z")
            .unwrap();
        assert_eq!(again.updated_at, claimed.updated_at);
    }

    #[test]
    fn claiming_an_entry_that_does_not_exist_is_an_error() {
        let (mut ledger, _) = seeded();
        assert!(ledger.claim_mine("le_missing0001", LATER).is_err());
    }

    #[test]
    fn context_only_untracks_what_is_not_mine_and_spares_the_rest() {
        let (mut ledger, _) = seeded();

        let outcome = ledger
            .retro_apply_note_tracking(NOTE, PROJECT, true, RetroSource::Override, LATER)
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
    fn recategorizing_untracks_what_the_old_genre_had_enrolled() {
        // The whole point of the category sweep: these entries were minted
        // while the meeting was, say, a standup, so they say `default`. If the
        // sweep only matched `category` it would find nothing here.
        let (mut ledger, _) = seeded();
        assert_eq!(
            entry_for(&ledger, "a_111111").enrolled_via,
            EnrolledVia::Default
        );

        let outcome = ledger
            .retro_apply_note_tracking(NOTE, PROJECT, true, RetroSource::Category, LATER)
            .unwrap();

        assert_eq!(outcome.untracked.len(), 2);
        assert_eq!(entry_for(&ledger, "a_222222").state, EntryState::Open);
        // Attributed to the genre, so the row can answer why it left.
        assert_eq!(
            entry_for(&ledger, "a_111111").untracked_via,
            Some(UntrackedVia::Category)
        );
    }

    #[test]
    fn recategorizing_back_revives_what_the_genre_untracked() {
        let (mut ledger, _) = seeded();
        ledger
            .retro_apply_note_tracking(NOTE, PROJECT, true, RetroSource::Category, LATER)
            .unwrap();

        let outcome = ledger
            .retro_apply_note_tracking(NOTE, PROJECT, false, RetroSource::Category, LATER)
            .unwrap();

        assert_eq!(outcome.retracked.len(), 2);
        for item_id in ["a_111111", "a_222222", "a_333333"] {
            let entry = entry_for(&ledger, item_id);
            assert_eq!(entry.state, EntryState::Open, "{item_id} should be live");
            assert_eq!(entry.untracked_via, None);
        }
    }

    #[test]
    fn each_machine_source_undoes_the_others_work() {
        // Deliberately symmetric: both sources are the machine's, so flipping a
        // meeting's own switch back revives what a recategorization untracked
        // and vice versa. Only a person's `manual` is out of reach either way.
        let (mut ledger, _) = seeded();
        ledger
            .retro_apply_note_tracking(NOTE, PROJECT, true, RetroSource::Category, LATER)
            .unwrap();

        let outcome = ledger
            .retro_apply_note_tracking(NOTE, PROJECT, false, RetroSource::Override, LATER)
            .unwrap();

        assert_eq!(outcome.retracked.len(), 2);
        assert_eq!(entry_for(&ledger, "a_111111").state, EntryState::Open);
    }

    #[test]
    fn a_recategorization_never_overrides_an_entry_a_person_touched() {
        let (mut ledger, _) = seeded();
        // A person snoozed Priya's line: a judgement about the commitment,
        // which a genre's default has no standing to revisit.
        let priya = entry_for(&ledger, "a_111111").entry_id;
        ledger.snooze(&priya, "2026-09-01", LATER).unwrap();
        ledger.mark_touched(&priya).unwrap();

        let outcome = ledger
            .retro_apply_note_tracking(NOTE, PROJECT, true, RetroSource::Category, LATER)
            .unwrap();

        assert_eq!(outcome.untracked.len(), 1, "only the untouched stray moves");
        assert_eq!(entry_for(&ledger, "a_111111").state, EntryState::Snoozed);
        assert_eq!(entry_for(&ledger, "a_333333").state, EntryState::Untracked);
    }

    #[test]
    fn a_manually_promoted_entry_survives_a_recategorization() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let item = fact("a_111111", "Priya", "send the revised deck");
        // Gated out by the genre, then promoted by hand anyway.
        ledger
            .sync_note_items(&NoteSync {
                note_id: NOTE,
                project: PROJECT,
                note_date_utc: DAY_ONE,
                items: std::slice::from_ref(&item),
                link_hints: &[],
                note_override: None,
                category_default: Some(EnrollmentMode::ContextOnly),
                identity: &crate::ledger::OwnerIdentity::default(),
                now: NOW,
            })
            .unwrap();
        let entry = ledger
            .track_item(
                NOTE,
                &item,
                PROJECT,
                DAY_ONE,
                &crate::ledger::OwnerIdentity::default(),
                LATER,
            )
            .unwrap();
        assert_eq!(entry.enrolled_via, EnrolledVia::Manual);

        let outcome = ledger
            .retro_apply_note_tracking(NOTE, PROJECT, true, RetroSource::Category, LATER)
            .unwrap();

        assert!(
            outcome.untracked.is_empty(),
            "a promote outranks the genre that gated it"
        );
        assert_eq!(entry_for(&ledger, "a_111111").state, EntryState::Open);
    }

    #[test]
    fn flipping_back_revives_exactly_what_the_override_untracked() {
        let (mut ledger, _) = seeded();
        ledger
            .retro_apply_note_tracking(NOTE, PROJECT, true, RetroSource::Override, LATER)
            .unwrap();

        let outcome = ledger
            .retro_apply_note_tracking(NOTE, PROJECT, false, RetroSource::Override, LATER)
            .unwrap();

        assert!(!outcome.context_only);
        assert_eq!(outcome.retracked.len(), 2);
        for item_id in ["a_111111", "a_222222", "a_333333"] {
            let entry = entry_for(&ledger, item_id);
            assert_eq!(entry.state, EntryState::Open, "{item_id} should be live");
            assert_eq!(entry.untracked_via, None, "{item_id} keeps no stale trace");
        }
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
                note_override: None,
                category_default: None,
                identity: &crate::ledger::OwnerIdentity::default(),
                now: NOW,
            })
            .unwrap();

        let outcome = ledger
            .retro_apply_note_tracking("n_d4e5f6", PROJECT, true, RetroSource::Override, LATER)
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
            .retro_apply_note_tracking(NOTE, PROJECT, true, RetroSource::Override, LATER)
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
            .retro_apply_note_tracking(NOTE, PROJECT, true, RetroSource::Override, LATER)
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
            .retro_apply_note_tracking(NOTE, PROJECT, true, RetroSource::Override, LATER)
            .unwrap();
        let outcome = ledger
            .retro_apply_note_tracking(NOTE, PROJECT, false, RetroSource::Override, LATER)
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
            .track_item(
                NOTE,
                &items[0],
                PROJECT,
                DAY_ONE,
                &crate::ledger::OwnerIdentity::default(),
                LATER,
            )
            .unwrap();
        assert_eq!(promoted.enrolled_via, EnrolledVia::Manual);
        assert_eq!(promoted.state, EntryState::Open);

        let outcome = ledger
            .retro_apply_note_tracking(NOTE, PROJECT, true, RetroSource::Override, LATER)
            .unwrap();

        assert!(!outcome.untracked.contains(&theirs));
        assert_eq!(entry_for(&ledger, "a_111111").state, EntryState::Open);
    }

    #[test]
    fn setting_the_same_mode_twice_changes_nothing() {
        let (mut ledger, _) = seeded();
        let first = ledger
            .retro_apply_note_tracking(NOTE, PROJECT, true, RetroSource::Override, LATER)
            .unwrap();
        let second = ledger
            .retro_apply_note_tracking(NOTE, PROJECT, true, RetroSource::Override, LATER)
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
        ledger
            .retro_apply_note_tracking(NOTE, PROJECT, true, RetroSource::Override, NOW)
            .unwrap();
        // The note's frontmatter is the gate's input, so each re-sync carries
        // whatever the note says at that moment — exactly what the shell does.
        let sync = |ledger: &mut Ledger, note_override: Option<EnrollmentMode>| {
            ledger
                .sync_note_items(&NoteSync {
                    note_id: NOTE,
                    project: PROJECT,
                    note_date_utc: DAY_ONE,
                    items: &items,
                    link_hints: &[],
                    note_override,
                    category_default: None,
                    identity: &crate::ledger::OwnerIdentity::default(),
                    now: NOW,
                })
                .unwrap()
        };
        let gated = sync(&mut ledger, Some(EnrollmentMode::ContextOnly));
        assert_eq!(gated.not_enrolled, 1);
        assert_eq!(gated.created.len(), 1);

        // The other half of retro-application: this module revives what it
        // untracked, and the shell's follow-up sync creates what never existed.
        ledger
            .retro_apply_note_tracking(NOTE, PROJECT, false, RetroSource::Override, LATER)
            .unwrap();
        let after = sync(&mut ledger, None);

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
        ledger
            .retro_apply_note_tracking(NOTE, PROJECT, true, RetroSource::Override, NOW)
            .unwrap();
        let item = fact("a_111111", "Priya", "send the revised deck");
        ledger
            .sync_note_items(&NoteSync {
                note_id: NOTE,
                project: PROJECT,
                note_date_utc: DAY_ONE,
                items: std::slice::from_ref(&item),
                link_hints: &[],
                note_override: Some(EnrollmentMode::ContextOnly),
                category_default: None,
                identity: &crate::ledger::OwnerIdentity::default(),
                now: NOW,
            })
            .unwrap();
        assert!(ledger.entry_for_item(NOTE, "a_111111").unwrap().is_none());

        let entry = ledger
            .track_item(
                NOTE,
                &item,
                PROJECT,
                DAY_ONE,
                &crate::ledger::OwnerIdentity::default(),
                LATER,
            )
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
            .track_item(
                NOTE,
                &items[0],
                PROJECT,
                DAY_ONE,
                &crate::ledger::OwnerIdentity::default(),
                LATER,
            )
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
            .track_item(
                NOTE,
                &items[0],
                PROJECT,
                DAY_ONE,
                &crate::ledger::OwnerIdentity::default(),
                LATER,
            )
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
            .retro_apply_note_tracking(NOTE, PROJECT, true, RetroSource::Override, LATER)
            .unwrap();

        assert_eq!(ledger.dirty_projects(), vec![PROJECT.to_string()]);
    }

    // --- the legacy-override drain ----------------------------------------

    /// Writes a meeting note into `vault/Ops`.
    fn write_drain_meeting(vault: &Path, id: &str, tracking: Option<EnrollmentMode>) {
        let written = crate::note::Note::new(
            NoteId::parse(id).unwrap(),
            crate::note::NoteType::Meeting,
            crate::note::Routing::Manual {
                project: PROJECT.to_string(),
            },
            "2026-08-18",
            Vec::new(),
            crate::note::Source::parse("transcript").unwrap(),
            "Body.",
        )
        .unwrap()
        .with_tracking(tracking);
        crate::note::write_note(vault, &written, Some("Weekly sync")).unwrap();
    }

    /// Seeds a ledger the way a **pre-graduation** vault does: a `_ledger.yml`
    /// carrying a `note_overrides` section, restored into an empty database.
    /// That restore is the only route a legacy row can still take, which is
    /// exactly what makes it the right fixture.
    fn restore_pre_graduation_snapshot(vault: &Path, body: &str) -> Ledger {
        let project_dir = vault.join(PROJECT);
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(project_dir.join(crate::ledger::LEDGER_SNAPSHOT_FILE), body).unwrap();
        let mut ledger = Ledger::open_in_memory().unwrap();
        ledger.restore_from_snapshots_if_empty(vault).unwrap();
        ledger
    }

    const OVERRIDE_ONLY_SNAPSHOT: &str = "version: 2\nentries: []\nnote_overrides:\n  - note_id: n_a1b2c3\n    mode: context_only\n    set_at: 2026-08-18T12:00:00Z\n";

    /// A pre-graduation snapshot holding a parked override *and* an entry the
    /// override should have kept out.
    const OVERRIDE_AND_ENTRY_SNAPSHOT: &str = "version: 2\nentries:\n  - entry_id: le_aaaaaaaaaaaa\n    state: open\n    direction: theirs\n    owner: Priya\n    description: send the deck\n    created_at: 2026-08-18T12:00:00Z\n    updated_at: 2026-08-18T12:00:00Z\n    last_mention: 2026-08-18T00:00:00Z\n    items:\n      - item_id: a_111111\n        note_id: n_a1b2c3\n        active: true\n        linked_at: 2026-08-18T12:00:00Z\nnote_overrides:\n  - note_id: n_a1b2c3\n    mode: context_only\n    set_at: 2026-08-18T12:00:00Z\n";

    #[test]
    fn the_drain_graduates_a_parked_override_into_its_note() {
        let vault = tempfile::tempdir().unwrap();
        write_drain_meeting(vault.path(), "n_a1b2c3", None);
        let mut ledger = restore_pre_graduation_snapshot(vault.path(), OVERRIDE_ONLY_SNAPSHOT);
        assert_eq!(ledger.legacy_note_overrides().unwrap().len(), 1);

        let outcome = ledger
            .drain_legacy_note_overrides(vault.path(), LATER)
            .unwrap();

        assert_eq!(outcome.graduated, 1);
        let found =
            crate::vault::find_note_anywhere(vault.path(), &NoteId::parse("n_a1b2c3").unwrap())
                .unwrap()
                .expect("the note is still there");
        assert_eq!(
            found.1.note.tracking,
            Some(EnrollmentMode::ContextOnly),
            "the judgement must survive into the note"
        );
        assert!(
            ledger.legacy_note_overrides().unwrap().is_empty(),
            "a graduated row is dropped"
        );
    }

    #[test]
    fn the_drain_re_evaluates_entries_the_override_should_have_gated() {
        let vault = tempfile::tempdir().unwrap();
        write_drain_meeting(vault.path(), "n_a1b2c3", None);
        let mut ledger = restore_pre_graduation_snapshot(vault.path(), OVERRIDE_AND_ENTRY_SNAPSHOT);
        assert_eq!(
            ledger.entries_for_note("n_a1b2c3").unwrap()[0].entry.state,
            EntryState::Open
        );

        ledger
            .drain_legacy_note_overrides(vault.path(), LATER)
            .unwrap();

        let details = ledger.entries_for_note("n_a1b2c3").unwrap();
        assert_eq!(details.len(), 1);
        assert_eq!(
            details[0].entry.state,
            EntryState::Untracked,
            "an entry minted while the override was still parked has to be re-checked"
        );
    }

    #[test]
    fn the_drain_leaves_a_note_that_already_carries_the_key_alone() {
        let vault = tempfile::tempdir().unwrap();
        // The note already says "tracked" — a later, deliberate flip back.
        write_drain_meeting(vault.path(), "n_a1b2c3", Some(EnrollmentMode::Tracked));
        let mut ledger = restore_pre_graduation_snapshot(vault.path(), OVERRIDE_ONLY_SNAPSHOT);

        let outcome = ledger
            .drain_legacy_note_overrides(vault.path(), LATER)
            .unwrap();

        assert_eq!(outcome.graduated, 0);
        assert_eq!(outcome.discarded, 1);
        let found =
            crate::vault::find_note_anywhere(vault.path(), &NoteId::parse("n_a1b2c3").unwrap())
                .unwrap()
                .expect("the note is still there");
        assert_eq!(
            found.1.note.tracking,
            Some(EnrollmentMode::Tracked),
            "frontmatter is the truth; a stale row must not overwrite it"
        );
        assert!(ledger.legacy_note_overrides().unwrap().is_empty());
    }

    #[test]
    fn the_drain_drops_a_row_whose_note_is_gone() {
        let vault = tempfile::tempdir().unwrap();
        // No note written at all: nothing is left to carry the judgement.
        let mut ledger = restore_pre_graduation_snapshot(vault.path(), OVERRIDE_ONLY_SNAPSHOT);

        let outcome = ledger
            .drain_legacy_note_overrides(vault.path(), LATER)
            .unwrap();

        assert_eq!(outcome.discarded, 1);
        assert!(ledger.legacy_note_overrides().unwrap().is_empty());
    }

    #[test]
    fn draining_twice_changes_nothing_the_second_time() {
        let vault = tempfile::tempdir().unwrap();
        write_drain_meeting(vault.path(), "n_a1b2c3", None);
        let mut ledger = restore_pre_graduation_snapshot(vault.path(), OVERRIDE_ONLY_SNAPSHOT);

        ledger
            .drain_legacy_note_overrides(vault.path(), LATER)
            .unwrap();
        let path =
            crate::vault::find_note_anywhere(vault.path(), &NoteId::parse("n_a1b2c3").unwrap())
                .unwrap()
                .unwrap()
                .1
                .path;
        let content = std::fs::read_to_string(&path).unwrap();

        let second = ledger
            .drain_legacy_note_overrides(vault.path(), LATER)
            .unwrap();

        assert_eq!(second, DrainOutcome::default(), "an empty table is a no-op");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
    }
}
