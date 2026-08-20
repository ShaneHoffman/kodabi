//! Reconciling a note's extracted action items with the ledger.
//!
//! This is the seam the index worker calls whenever a note's facts are
//! (re)derived. It is **idempotent** — re-syncing an unchanged note writes
//! nothing and marks no snapshot dirty — and **deterministic**: no clock is read
//! (the caller supplies `now`), and every tie is broken by an explicit rule, so
//! the same inputs always produce the same ledger.
//!
//! ## Why this is not a simple join
//!
//! An item's `a_` id is a hash of the parsed line, so it is stable across a
//! checkbox flip but re-minted the moment the owner, description, or due date is
//! edited. From the ledger's side an edit looks exactly like "one item vanished,
//! a different one appeared" — and a *deletion* looks the same. Getting that
//! wrong either orphans a commitment on every typo fix or silently merges two
//! unrelated ones, so the reconciliation runs in tiers, each only firing where
//! its answer is unambiguous:
//!
//! 1. **Present** — the id is on both sides. Nothing to do; this is where a
//!    `- [x]` flip lands, and it correctly changes nothing.
//! 2. **Tier A, per-owner swap** — within one owner, exactly one id vanished and
//!    exactly one appeared. That is an edited line; the entry follows its text.
//! 3. **Tier B, whole-note swap** — after tier A, exactly one of each is left
//!    for the whole note. That is an edited line whose *owner* changed.
//! 4. **Cross-note re-mention** — a new item whose owner and description exactly
//!    match a live entry elsewhere is the same commitment said again: one entry,
//!    a second ref, a bumped `last_mention`.
//! 5. **Create** — anything left is a new commitment, *if the meeting's
//!    enrollment mode says so*. See below.
//! 6. **Retire** — anything still vanished loses its ref. An entry with no live
//!    refs left goes to `needs_review` rather than being deleted or guessed at.
//!
//! Ambiguity always falls through to 5 + 6: a new entry plus a review flag. That
//! is noisy but never wrong, which is the right way round for a store whose
//! whole job is not losing commitments.
//!
//! ## The enrollment gate, and why it sits in tier 5 alone
//!
//! Extraction is unconditional; tracking is not
//! ([`EnrollmentMode`](super::EnrollmentMode)). The mode is resolved here, in
//! one place inside the transaction, so both ingest paths — the index worker
//! and the distill follow-up — are gated identically; a gate either of them
//! could forget to apply is not a gate.
//!
//! Both halves of the chain now arrive as inputs — the per-meeting override
//! ([`NoteSync::note_override`], which lives in the note's frontmatter) and the
//! meeting category's default ([`NoteSync::category_default`], which lives in
//! the settings file) — because this store can read neither the vault nor the
//! settings. What keeps that safe is that neither field is optional:
//! [`NoteSync`] has no `Default`, so a producer that forgets either does not
//! compile. Resolution — the part a caller could get *wrong* rather than merely
//! forget — stayed here, in [`effective_mode`](super::effective_mode).
//!
//! It gates **tier 5 only**, and each of the other tiers is deliberate:
//!
//! * **Present, tier A, tier B** already have an entry. The gate decides what
//!   earns one, never what happens to one that exists — that is retro-application's
//!   job, and it has rules about what a person has touched that sync cannot see.
//! * **Re-mention** links a line to a commitment that is already tracked. A
//!   context-only meeting that restates a live commitment is still evidence it
//!   is alive, and dropping the mention would age the entry out for being
//!   discussed in the wrong room.
//! * **Retire** is untouched, which is what keeps an untracked entry safe: its
//!   refs stay active, so it lands in tier 1 and never looks vanished.
//!
//! A gated item leaves no trace in the ledger at all — no entry, no ref — so a
//! pass that gates everything is a no-op, and flipping the meeting to tracked
//! later simply re-syncs and creates. Idempotence does the retro-application in
//! that direction for free.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{params, Connection, OptionalExtension};

use super::store::normalize;
use super::{mint_id, Direction, EnrolledVia, EnrollmentMode, EntryState, Ledger, Result};
use crate::meeting::ActionItemFact;

/// "This new line is that existing commitment, said again."
///
/// The reconciler pairs lines to entries by exact normalized text, which is
/// the only thing it can be sure of on its own. A distill that was shown the
/// open commitments knows more than that: it can tell that "send the slide
/// deck" is the commitment already recorded as "send the deck". A hint carries
/// that judgement in, so the paraphrase links instead of minting a second live
/// entry for one promise.
///
/// Advisory, never binding: a hint naming an entry that has since closed or
/// vanished is ignored, and the ordinary matching runs.
pub struct LinkHint {
    /// The action-item id in the note being synced.
    pub item_id: String,
    /// The existing entry it restates.
    pub entry_id: String,
}

/// One note's freshly derived facts, as handed to [`Ledger::sync_note_items`].
pub struct NoteSync<'a> {
    pub note_id: &'a str,
    /// The note's current project slug (the Inbox sentinel is a valid value).
    pub project: &'a str,
    /// The note's `date_utc`. Used for `last_mention`, deliberately in place of
    /// the wall clock: re-indexing an old vault must not make every commitment
    /// in it look freshly mentioned.
    pub note_date_utc: &'a str,
    pub items: &'a [ActionItemFact],
    /// Lines this note's distill recognized as commitments already tracked.
    /// Empty for an ordinary index-driven sync, which knows only what the text
    /// says.
    pub link_hints: &'a [LinkHint],
    /// The note's own tracking override, read from its frontmatter `tracking:`
    /// key; `None` means it inherits.
    ///
    /// Not defaulted and not `Option`-by-omission: it is a plain field on a
    /// struct with no `Default`, so every producer is forced by the compiler to
    /// say what the note's override is. That is what replaces the old
    /// "resolved inside sync so no caller can forget the gate" argument — the
    /// judgement now lives in the note file, which the ledger cannot read, so
    /// it has to arrive as an input; making it un-omittable is how the gate
    /// stays impossible to skip.
    pub note_override: Option<EnrollmentMode>,
    /// The default the note's meeting *category* carries, already resolved
    /// against the user's settings by the shell
    /// ([`crate::ledger::category_default_for`]); `None` when the note has no
    /// category to inherit from, which is every chat and any meeting the
    /// classifier left unlabelled.
    ///
    /// Resolved rather than raw for the same reason `note_override` is an input
    /// at all: the setting lives in a file this store cannot read. It is a plain
    /// field on the same no-`Default` struct, so the compiler holds the second
    /// half of the chain to the same standard as the first.
    pub category_default: Option<EnrollmentMode>,
    /// RFC 3339 UTC, for `created_at` / `updated_at` / ref timestamps.
    pub now: &'a str,
}

/// What one sync pass did. Empty vectors and a zero `unchanged` mean the note
/// carries no tracked commitments; a pass that changed nothing leaves every
/// vector empty with `unchanged` counting the items it matched.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    /// Entries minted for items never seen before.
    pub created: Vec<String>,
    /// Entries re-attached to a re-minted id after their line was edited.
    pub relinked: Vec<String>,
    /// Entries that gained a ref because the commitment was said again.
    pub rementioned: Vec<String>,
    /// Entries whose last live ref disappeared.
    pub sent_to_review: Vec<String>,
    /// Items whose id was already linked, and stayed linked.
    pub unchanged: usize,
    /// Items the meeting's enrollment mode kept out of the ledger.
    ///
    /// Reported for the log, not stored: an un-enrolled item has no row
    /// anywhere, which is the point of the gate.
    pub not_enrolled: usize,
}

impl SyncOutcome {
    /// Whether the pass wrote anything.
    ///
    /// `not_enrolled` is deliberately excluded: gating an item writes nothing,
    /// so a pass that gates every item must stay silent or the snapshot
    /// debounce would rearm on every reconcile of a context-only meeting.
    pub fn is_noop(&self) -> bool {
        self.created.is_empty()
            && self.relinked.is_empty()
            && self.rementioned.is_empty()
            && self.sent_to_review.is_empty()
    }
}

/// An entry as the reconciliation needs to see it.
struct EntryFacts {
    entry_id: String,
    owner_norm: String,
    project: String,
    last_mention: String,
    state: EntryState,
}

impl Ledger {
    /// Reconciles one note's action items with the ledger. See the module doc
    /// for the tier order and why it exists.
    pub fn sync_note_items(&mut self, sync: &NoteSync<'_>) -> Result<SyncOutcome> {
        let mut dirty = BTreeSet::new();
        let tx = self.connection_mut().transaction()?;
        let outcome = reconcile(&tx, sync, &mut dirty)?;
        tx.commit()?;
        self.mark_all_dirty(dirty);
        Ok(outcome)
    }

    /// Handles a note leaving the vault: every ref it held retires, and entries
    /// left with nothing live go to review.
    ///
    /// Entries are **never deleted** here. A commitment outlives the note that
    /// happened to record it, and deciding it no longer matters is a judgement
    /// only a human makes ([`Ledger::waive`]).
    pub fn note_removed(&mut self, note_id: &str, now: &str) -> Result<SyncOutcome> {
        self.sync_note_items(&NoteSync {
            note_id,
            // No items means no creates and no drift, so neither is read.
            project: "",
            note_date_utc: now,
            items: &[],
            link_hints: &[],
            // No items means nothing can be enrolled, so the gate is moot.
            note_override: None,
            category_default: None,
            now,
        })
    }
}

/// The whole reconciliation, inside one transaction.
fn reconcile(
    tx: &Connection,
    sync: &NoteSync<'_>,
    dirty: &mut BTreeSet<String>,
) -> Result<SyncOutcome> {
    let mut outcome = SyncOutcome::default();

    // --- Resolve the meeting's enrollment mode ----------------------------
    // Resolved once, inside the transaction, so every ingest path is gated the
    // same way. Both halves of the chain ride in on `NoteSync` because both live
    // where this store cannot read them: the override in the note's frontmatter,
    // the category default in the settings file.
    let note_override = sync.note_override;
    let category_default = sync.category_default;
    let mode = super::effective_mode(note_override, category_default);
    // An entry minted under a *gating* source is enrolled because of it — a Mine
    // item in a meeting that tracks direct asks only. Which source gets the
    // credit follows the chain: an override outranks a category default, and
    // whichever one decided, it is only recorded when it actually gated.
    //
    // A source that resolves to `tracked` enrolls nothing the global default
    // would not have, so it stays `Default`: recording it otherwise would
    // misreport why the entry is here, and would exclude it from
    // `overridable_entries`, which retro-untracks what a machine default
    // enrolled — including, crucially, entries minted before this meeting was
    // recategorized, which say `default` and must stay sweepable.
    let enrolled_via = match (note_override, category_default) {
        (Some(EnrollmentMode::ContextOnly), _) => EnrolledVia::Override,
        (None, Some(EnrollmentMode::ContextOnly)) => EnrolledVia::Category,
        _ => EnrolledVia::Default,
    };

    // --- Partition -------------------------------------------------------
    let existing = active_refs(tx, sync.note_id)?;
    let incoming: BTreeSet<&str> = sync.items.iter().map(|item| item.id.as_str()).collect();

    let mut vanished: Vec<(String, String)> = Vec::new(); // (item_id, entry_id)
    for (item_id, entry_id) in &existing {
        if incoming.contains(item_id.as_str()) {
            // Present on both sides. A checkbox flip lands here and correctly
            // changes nothing; only the project may have moved.
            outcome.unchanged += 1;
            follow_project(tx, entry_id, sync, dirty)?;
        } else {
            vanished.push((item_id.clone(), entry_id.clone()));
        }
    }

    let linked: BTreeSet<&str> = existing
        .iter()
        .map(|(item_id, _)| item_id.as_str())
        .collect();
    let mut fresh: Vec<&ActionItemFact> = sync
        .items
        .iter()
        .filter(|item| !linked.contains(item.id.as_str()))
        .collect();

    // --- Tier A: per-owner unique swap ------------------------------------
    let mut vanished_facts = Vec::new();
    for (item_id, entry_id) in vanished {
        let facts = entry_facts(tx, &entry_id)?;
        vanished_facts.push((item_id, facts));
    }

    let mut by_owner_gone: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (idx, (_, facts)) in vanished_facts.iter().enumerate() {
        by_owner_gone
            .entry(facts.owner_norm.clone())
            .or_default()
            .push(idx);
    }
    let mut by_owner_new: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (idx, item) in fresh.iter().enumerate() {
        by_owner_new
            .entry(normalize(&item.owner))
            .or_default()
            .push(idx);
    }

    // Pairs are recorded as they are decided, never re-derived afterwards: only
    // the tier that made a pair knows which side belongs to which, and tier B
    // pairs two *different* owners by construction, so nothing about the pair
    // survives in the owner keys.
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    let mut paired_gone: BTreeSet<usize> = BTreeSet::new();
    let mut paired_new: BTreeSet<usize> = BTreeSet::new();
    for (owner, gone_idxs) in &by_owner_gone {
        let Some(new_idxs) = by_owner_new.get(owner) else {
            continue;
        };
        if gone_idxs.len() == 1 && new_idxs.len() == 1 {
            paired_gone.insert(gone_idxs[0]);
            paired_new.insert(new_idxs[0]);
            pairs.push((gone_idxs[0], new_idxs[0]));
        }
    }

    // --- Tier B: whole-note unique swap -----------------------------------
    let unpaired_gone: Vec<usize> = (0..vanished_facts.len())
        .filter(|idx| !paired_gone.contains(idx))
        .collect();
    let unpaired_new: Vec<usize> = (0..fresh.len())
        .filter(|idx| !paired_new.contains(idx))
        .collect();
    if unpaired_gone.len() == 1 && unpaired_new.len() == 1 {
        paired_gone.insert(unpaired_gone[0]);
        paired_new.insert(unpaired_new[0]);
        pairs.push((unpaired_gone[0], unpaired_new[0]));
    }

    // Apply the pairs: the entry keeps its identity and adopts the new text.
    for (gone_idx, new_idx) in &pairs {
        let (old_item_id, facts) = &vanished_facts[*gone_idx];
        let item = fresh[*new_idx];
        retire_ref(tx, &facts.entry_id, sync.note_id, old_item_id, sync.now)?;
        link_ref(tx, &facts.entry_id, sync.note_id, &item.id, sync.now)?;
        tx.execute(
            "UPDATE ledger_entries
             SET owner = ?2, description = ?3, owner_norm = ?4, description_norm = ?5,
                 direction = ?6, updated_at = ?7
             WHERE entry_id = ?1",
            params![
                facts.entry_id,
                item.owner,
                item.description,
                normalize(&item.owner),
                normalize(&item.description),
                Direction::from_owner(&item.owner).as_str(),
                sync.now,
            ],
        )?;
        dirty.insert(facts.project.clone());
        follow_project(tx, &facts.entry_id, sync, dirty)?;
        outcome.relinked.push(facts.entry_id.clone());
    }

    // Everything the tiers did not pair.
    let leftover_gone: Vec<&(String, EntryFacts)> = vanished_facts
        .iter()
        .enumerate()
        .filter(|(idx, _)| !paired_gone.contains(idx))
        .map(|(_, value)| value)
        .collect();
    fresh = fresh
        .into_iter()
        .enumerate()
        .filter(|(idx, _)| !paired_new.contains(idx))
        .map(|(_, item)| item)
        .collect();

    // --- Cross-note re-mention, then create --------------------------------
    for item in &fresh {
        let owner_norm = normalize(&item.owner);
        let description_norm = normalize(&item.description);
        // A hint first, then the text. The hint is a stronger claim than exact
        // equality can make, and it is checked against the live states so a
        // stale one falls through rather than resurrecting a settled entry.
        let hinted = match sync.link_hints.iter().find(|hint| hint.item_id == item.id) {
            Some(hint) if is_live_entry(tx, &hint.entry_id)? => Some(hint.entry_id.clone()),
            _ => None,
        };
        let matched = match hinted {
            Some(entry_id) => Some(entry_id),
            None => match_live_entry(tx, &owner_norm, &description_norm)?,
        };
        match matched {
            Some(entry_id) => {
                link_ref(tx, &entry_id, sync.note_id, &item.id, sync.now)?;
                bump_mention(tx, &entry_id, sync, dirty)?;
                outcome.rementioned.push(entry_id);
            }
            None => {
                // The gate. An item the mode excludes gets no entry and no ref,
                // so nothing downstream — aging, evidence, the Commitments
                // view — ever sees it.
                let direction = Direction::from_owner(&item.owner);
                if !mode.enrolls(direction) {
                    outcome.not_enrolled += 1;
                    continue;
                }
                let entry_id = insert_open_entry(
                    tx,
                    NewEntry {
                        item,
                        direction,
                        project: sync.project,
                        last_mention: sync.note_date_utc,
                        enrolled_via,
                        now: sync.now,
                    },
                )?;
                link_ref(tx, &entry_id, sync.note_id, &item.id, sync.now)?;
                dirty.insert(sync.project.to_string());
                outcome.created.push(entry_id);
            }
        }
    }

    // --- Retire what is left ----------------------------------------------
    for (item_id, facts) in leftover_gone {
        retire_ref(tx, &facts.entry_id, sync.note_id, item_id, sync.now)?;
        dirty.insert(facts.project.clone());
        if !has_active_ref(tx, &facts.entry_id)? && !facts.state.is_terminal() {
            // Open or snoozed with nothing left to point at: a human has to say
            // whether it was done, dropped, or reworded past recognition.
            tx.execute(
                "UPDATE ledger_entries
                 SET state = 'needs_review', snoozed_until = NULL, review_reason = ?2,
                     updated_at = ?3
                 WHERE entry_id = ?1",
                params![
                    facts.entry_id,
                    format!("source line removed from {}", sync.note_id),
                    sync.now,
                ],
            )?;
            outcome.sent_to_review.push(facts.entry_id.clone());
        }
    }

    Ok(outcome)
}

/// The fields a freshly enrolled entry needs.
pub(crate) struct NewEntry<'a> {
    pub item: &'a ActionItemFact,
    pub direction: Direction,
    pub project: &'a str,
    pub last_mention: &'a str,
    pub enrolled_via: EnrolledVia,
    pub now: &'a str,
}

/// Mints one open entry. The single INSERT both create sites share — sync's
/// tier 5 and a manual promote — so an entry's shape cannot depend on which
/// door it came through.
pub(crate) fn insert_open_entry(tx: &Connection, new: NewEntry<'_>) -> Result<String> {
    let entry_id = mint_id("le_")?;
    tx.execute(
        "INSERT INTO ledger_entries
             (entry_id, state, direction, owner, description, owner_norm,
              description_norm, project, created_at, updated_at, last_mention, enrolled_via)
         VALUES (?1, 'open', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9, ?10)",
        params![
            entry_id,
            new.direction.as_str(),
            new.item.owner,
            new.item.description,
            normalize(&new.item.owner),
            normalize(&new.item.description),
            new.project,
            new.now,
            new.last_mention,
            new.enrolled_via.as_str(),
        ],
    )?;
    Ok(entry_id)
}

/// The `(item_id, entry_id)` pairs currently live in a note, in a stable order.
fn active_refs(tx: &Connection, note_id: &str) -> Result<Vec<(String, String)>> {
    let mut stmt = tx.prepare(
        "SELECT item_id, entry_id FROM ledger_item_refs
         WHERE note_id = ?1 AND active = 1 ORDER BY item_id, entry_id",
    )?;
    let rows = stmt.query_map([note_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Reads the columns the reconciliation needs from one entry.
fn entry_facts(tx: &Connection, entry_id: &str) -> Result<EntryFacts> {
    let (owner_norm, project, last_mention, state): (String, String, String, String) = tx
        .query_row(
            "SELECT owner_norm, project, last_mention, state FROM ledger_entries
             WHERE entry_id = ?1",
            [entry_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
    Ok(EntryFacts {
        entry_id: entry_id.to_string(),
        owner_norm,
        project,
        last_mention,
        state: EntryState::parse(&state)?,
    })
}

/// Whether an entry exists and is in a state a new line may attach to.
///
/// The same live set [`match_live_entry`] trusts: a closed or waived
/// commitment restated later is a new commitment, not a revival.
fn is_live_entry(tx: &Connection, entry_id: &str) -> Result<bool> {
    let state: Option<String> = tx
        .query_row(
            "SELECT state FROM ledger_entries WHERE entry_id = ?1",
            [entry_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(matches!(
        state.as_deref(),
        Some("open") | Some("needs_review") | Some("snoozed")
    ))
}

/// The live entry that best matches an owner + description, if any.
///
/// Only `open`, `needs_review`, and `snoozed` entries match. A commitment that
/// was closed or waived and is then re-stated is a **new** commitment: the old
/// judgement was about the old instance, and quietly reviving it would erase it.
/// Ties go to the most recently mentioned entry, then the smallest id.
fn match_live_entry(
    tx: &Connection,
    owner_norm: &str,
    description_norm: &str,
) -> Result<Option<String>> {
    Ok(tx
        .query_row(
            "SELECT entry_id FROM ledger_entries
             WHERE owner_norm = ?1 AND description_norm = ?2
               AND state IN ('open', 'needs_review', 'snoozed')
             ORDER BY last_mention DESC, entry_id
             LIMIT 1",
            params![owner_norm, description_norm],
            |row| row.get(0),
        )
        .optional()?)
}

/// Attaches an entry to one extracted line, reviving a retired ref if the same
/// wording came back.
fn link_ref(
    tx: &Connection,
    entry_id: &str,
    note_id: &str,
    item_id: &str,
    now: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO ledger_item_refs (entry_id, item_id, note_id, active, linked_at)
         VALUES (?1, ?2, ?3, 1, ?4)
         ON CONFLICT (entry_id, note_id, item_id)
         DO UPDATE SET active = 1, retired_at = NULL, linked_at = ?4",
        params![entry_id, item_id, note_id, now],
    )?;
    Ok(())
}

/// Retires a ref, keeping the row as history.
fn retire_ref(
    tx: &Connection,
    entry_id: &str,
    note_id: &str,
    item_id: &str,
    now: &str,
) -> Result<()> {
    tx.execute(
        "UPDATE ledger_item_refs SET active = 0, retired_at = ?4
         WHERE entry_id = ?1 AND note_id = ?2 AND item_id = ?3 AND active = 1",
        params![entry_id, note_id, item_id, now],
    )?;
    Ok(())
}

/// Whether an entry still points at any live line.
fn has_active_ref(tx: &Connection, entry_id: &str) -> Result<bool> {
    let count: i64 = tx.query_row(
        "SELECT count(*) FROM ledger_item_refs WHERE entry_id = ?1 AND active = 1",
        [entry_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Moves an entry to this note's project when this note is its newest mention.
///
/// Entries follow the conversation: if the most recent meeting that mentioned a
/// commitment lives in `Growth/Q3`, that is where the commitment belongs, and
/// where its snapshot is written. This one rule is also what makes re-filing a
/// note and renaming a project converge without either needing its own API, and
/// it is a no-op when the project has not changed, which keeps re-syncs silent.
fn follow_project(
    tx: &Connection,
    entry_id: &str,
    sync: &NoteSync<'_>,
    dirty: &mut BTreeSet<String>,
) -> Result<()> {
    if sync.project.is_empty() {
        return Ok(());
    }
    let facts = entry_facts(tx, entry_id)?;
    if facts.project == sync.project || facts.last_mention.as_str() > sync.note_date_utc {
        return Ok(());
    }
    tx.execute(
        "UPDATE ledger_entries SET project = ?2, updated_at = ?3 WHERE entry_id = ?1",
        params![entry_id, sync.project, sync.now],
    )?;
    // Both the old and the new project's snapshots are now behind.
    dirty.insert(facts.project);
    dirty.insert(sync.project.to_string());
    Ok(())
}

/// Records that a commitment was mentioned again by this note: the mention date
/// advances, the project follows, and an entry that was in review only because
/// its line vanished comes back to life.
fn bump_mention(
    tx: &Connection,
    entry_id: &str,
    sync: &NoteSync<'_>,
    dirty: &mut BTreeSet<String>,
) -> Result<()> {
    let facts = entry_facts(tx, entry_id)?;
    follow_project(tx, entry_id, sync, dirty)?;
    if sync.note_date_utc > facts.last_mention.as_str() {
        tx.execute(
            "UPDATE ledger_entries SET last_mention = ?2, updated_at = ?3 WHERE entry_id = ?1",
            params![entry_id, sync.note_date_utc, sync.now],
        )?;
    }
    if facts.state == EntryState::NeedsReview {
        tx.execute(
            "UPDATE ledger_entries
             SET state = 'open', review_reason = NULL, updated_at = ?2
             WHERE entry_id = ?1",
            params![entry_id, sync.now],
        )?;
    }
    dirty.insert(facts.project);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{ClosedVia, EntryFilter, UntrackedVia};

    const NOW: &str = "2026-08-17T12:00:00Z";
    const DAY_ONE: &str = "2026-08-01T00:00:00Z";
    const DAY_TWO: &str = "2026-08-10T00:00:00Z";

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

    fn sync_note(
        ledger: &mut Ledger,
        note_id: &str,
        project: &str,
        date: &str,
        items: &[ActionItemFact],
    ) -> SyncOutcome {
        ledger
            .sync_note_items(&NoteSync {
                note_id,
                project,
                note_date_utc: date,
                link_hints: &[],
                note_override: None,
                category_default: None,
                items,
                now: NOW,
            })
            .unwrap()
    }

    /// Syncs a note that carries a tracking override in its frontmatter — the
    /// gate's real input now that the judgement lives in the note file.
    fn sync_note_tracked_as(
        ledger: &mut Ledger,
        note_id: &str,
        project: &str,
        date: &str,
        items: &[ActionItemFact],
        note_override: Option<EnrollmentMode>,
    ) -> SyncOutcome {
        sync_note_gated_by(ledger, note_id, project, date, items, note_override, None)
    }

    /// Syncs a note under both halves of the enrollment chain: its own override
    /// and the default its meeting category resolved to.
    #[allow(clippy::too_many_arguments)]
    fn sync_note_gated_by(
        ledger: &mut Ledger,
        note_id: &str,
        project: &str,
        date: &str,
        items: &[ActionItemFact],
        note_override: Option<EnrollmentMode>,
        category_default: Option<EnrollmentMode>,
    ) -> SyncOutcome {
        ledger
            .sync_note_items(&NoteSync {
                note_id,
                project,
                note_date_utc: date,
                link_hints: &[],
                note_override,
                category_default,
                items,
                now: NOW,
            })
            .unwrap()
    }

    #[test]
    fn a_context_only_meeting_enrolls_only_what_is_mine() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![
            fact("a_111111", "Priya", "send the revised deck"),
            fact("a_222222", "You", "book the venue"),
            fact("a_333333", "Unassigned", "chase the caterer"),
        ];
        let outcome = sync_note_tracked_as(
            &mut ledger,
            "n_a1b2c3",
            "Briarwood Golf",
            DAY_ONE,
            &items,
            Some(EnrollmentMode::ContextOnly),
        );

        assert_eq!(outcome.created.len(), 1);
        assert_eq!(outcome.not_enrolled, 2);

        let entries = ledger.list_entries(&EntryFilter::default()).unwrap();
        assert_eq!(entries.len(), 1, "only the direct ask is tracked");
        assert_eq!(entries[0].owner, "You");
        // Enrolled *because of* the override, which is what stops a later flip
        // from treating it as something the default swept in.
        assert_eq!(entries[0].enrolled_via, EnrolledVia::Override);

        // A gated item leaves nothing at all behind, not even a retired ref.
        let refs: i64 = ledger
            .connection_mut()
            .query_row(
                "SELECT count(*) FROM ledger_item_refs WHERE item_id IN ('a_111111', 'a_333333')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(refs, 0);
    }

    #[test]
    fn a_gated_pass_is_a_noop_and_stays_one() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![fact("a_111111", "Priya", "send the revised deck")];

        let first = sync_note_tracked_as(
            &mut ledger,
            "n_a1b2c3",
            "Briarwood Golf",
            DAY_ONE,
            &items,
            Some(EnrollmentMode::ContextOnly),
        );
        assert_eq!(first.not_enrolled, 1);
        // The debounce arms off dirty projects, so a pass that only gated must
        // not read as work done.
        assert!(first.is_noop());
        assert!(ledger.dirty_projects().is_empty());

        let second = sync_note_tracked_as(
            &mut ledger,
            "n_a1b2c3",
            "Briarwood Golf",
            DAY_ONE,
            &items,
            Some(EnrollmentMode::ContextOnly),
        );
        assert!(second.is_noop());
        assert_eq!(second.not_enrolled, 1);
        assert_eq!(
            ledger.list_entries(&EntryFilter::default()).unwrap().len(),
            0
        );
    }

    #[test]
    fn a_context_only_meeting_still_re_mentions_a_tracked_commitment() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![fact("a_111111", "Priya", "send the revised deck")];
        sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &items);

        // The same commitment restated in a meeting attended for context. The
        // gate decides what earns an entry, never whether a live one was
        // mentioned, or a commitment would age out for being discussed in the
        // wrong room.
        let restated = vec![fact("a_444444", "Priya", "send the revised deck")];
        let outcome = sync_note_tracked_as(
            &mut ledger,
            "n_d4e5f6",
            "Briarwood Golf",
            DAY_TWO,
            &restated,
            Some(EnrollmentMode::ContextOnly),
        );

        assert_eq!(outcome.rementioned.len(), 1);
        assert_eq!(outcome.created.len(), 0);
        assert_eq!(outcome.not_enrolled, 0);
        let entries = ledger.list_entries(&EntryFilter::default()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].last_mention, DAY_TWO);
    }

    #[test]
    fn an_edit_in_a_context_only_meeting_still_relinks_its_entry() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![fact("a_111111", "Priya", "send the revised deck")];
        sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &items);

        // Flipping the meeting later must not orphan the entry it already made:
        // tier A owns an existing entry, and only retro-application may decide
        // an existing entry's fate.
        let edited = vec![fact("a_999999", "Priya", "send the revised deck v2")];
        let outcome = sync_note_tracked_as(
            &mut ledger,
            "n_a1b2c3",
            "Briarwood Golf",
            DAY_ONE,
            &edited,
            Some(EnrollmentMode::ContextOnly),
        );

        assert_eq!(outcome.relinked.len(), 1);
        assert_eq!(outcome.not_enrolled, 0);
        let entries = ledger.list_entries(&EntryFilter::default()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].description, "send the revised deck v2");
        assert_eq!(entries[0].state, EntryState::Open);
    }

    #[test]
    fn an_untracked_entry_rides_the_present_leg_and_is_never_reviewed() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![fact("a_111111", "Priya", "send the revised deck")];
        let created = sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &items);
        let entry_id = created.created[0].clone();
        ledger
            .untrack(&entry_id, UntrackedVia::Manual, NOW)
            .unwrap();

        // The refs stay active, so re-syncing the unchanged note sees the item
        // as present rather than vanished. If it took the retire leg instead,
        // the entry would be parked in needs_review and reappear in the view.
        let outcome = sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &items);
        assert!(outcome.is_noop());
        assert_eq!(outcome.unchanged, 1);
        assert!(outcome.sent_to_review.is_empty());

        let entry = ledger.get_entry(&entry_id).unwrap().unwrap().entry;
        assert_eq!(entry.state, EntryState::Untracked);
    }

    #[test]
    fn an_untracked_commitment_restated_elsewhere_mints_a_fresh_entry() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![fact("a_111111", "Priya", "send the revised deck")];
        let created = sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &items);
        let untracked_id = created.created[0].clone();
        ledger
            .untrack(&untracked_id, UntrackedVia::Manual, NOW)
            .unwrap();

        // Same doctrine as a waived commitment: the old judgement was about the
        // old instance. Silently folding the restatement into an invisible
        // entry would lose it entirely.
        let restated = vec![fact("a_444444", "Priya", "send the revised deck")];
        let outcome = sync_note(
            &mut ledger,
            "n_d4e5f6",
            "Briarwood Golf",
            DAY_TWO,
            &restated,
        );

        assert_eq!(outcome.created.len(), 1);
        assert!(outcome.rementioned.is_empty());
        assert_ne!(outcome.created[0], untracked_id);
    }

    #[test]
    fn a_link_hint_naming_an_untracked_entry_falls_through() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![fact("a_111111", "Priya", "send the revised deck")];
        let created = sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &items);
        let untracked_id = created.created[0].clone();
        ledger
            .untrack(&untracked_id, UntrackedVia::Manual, NOW)
            .unwrap();

        let restated = vec![fact("a_444444", "Priya", "send the slide deck")];
        let outcome = ledger
            .sync_note_items(&NoteSync {
                note_id: "n_d4e5f6",
                project: "Briarwood Golf",
                note_date_utc: DAY_TWO,
                items: &restated,
                link_hints: &[LinkHint {
                    item_id: "a_444444".to_string(),
                    entry_id: untracked_id.clone(),
                }],
                note_override: None,
                category_default: None,
                now: NOW,
            })
            .unwrap();

        assert_eq!(
            outcome.created.len(),
            1,
            "a hint never revives a settled entry"
        );
        assert_ne!(outcome.created[0], untracked_id);
    }

    /// An explicit `tracking: tracked` says "track this, whatever my category
    /// defaults to". It must not be recorded as an *override* enrolment: an
    /// override means "enrolled because a gating mode let this one through",
    /// and mislabelling it would exclude the entry from `overridable_entries`,
    /// so a later context-only flip would silently refuse to untrack it.
    #[test]
    fn an_explicit_tracked_override_still_reads_as_a_default_enrolment() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![fact("a_111111", "Priya", "send the revised deck")];

        let outcome = sync_note_tracked_as(
            &mut ledger,
            "n_a1b2c3",
            "Briarwood Golf",
            DAY_ONE,
            &items,
            Some(EnrollmentMode::Tracked),
        );

        assert_eq!(outcome.created.len(), 1);
        assert_eq!(outcome.not_enrolled, 0);
        let entries = ledger.list_entries(&EntryFilter::default()).unwrap();
        assert_eq!(entries[0].enrolled_via, EnrolledVia::Default);
    }

    #[test]
    fn a_category_default_gates_exactly_as_an_override_does() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![
            fact("a_111111", "Priya", "send the revised deck"),
            fact("a_222222", "You", "book the venue"),
        ];

        // An all-hands with no opinion of its own: the genre decides.
        let outcome = sync_note_gated_by(
            &mut ledger,
            "n_a1b2c3",
            "Briarwood Golf",
            DAY_ONE,
            &items,
            None,
            Some(EnrollmentMode::ContextOnly),
        );

        assert_eq!(outcome.not_enrolled, 1, "Priya's line stays in the note");
        assert_eq!(outcome.created.len(), 1);
        let entries = ledger.list_entries(&EntryFilter::default()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].direction, Direction::Mine);
        // The category is why this one is here, and the row can say so.
        assert_eq!(entries[0].enrolled_via, EnrolledVia::Category);
    }

    #[test]
    fn a_meetings_own_override_outranks_its_category() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![fact("a_111111", "Priya", "send the revised deck")];

        // "Track this all-hands in full" — the per-meeting judgement wins, and
        // records as a plain default because it enrolled nothing the global
        // default would not have.
        let outcome = sync_note_gated_by(
            &mut ledger,
            "n_a1b2c3",
            "Briarwood Golf",
            DAY_ONE,
            &items,
            Some(EnrollmentMode::Tracked),
            Some(EnrollmentMode::ContextOnly),
        );

        assert_eq!(outcome.created.len(), 1);
        assert_eq!(outcome.not_enrolled, 0);
        let entries = ledger.list_entries(&EntryFilter::default()).unwrap();
        assert_eq!(entries[0].enrolled_via, EnrolledVia::Default);
    }

    #[test]
    fn a_gating_override_outranks_a_tracking_category() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![fact("a_222222", "You", "book the venue")];

        // The mirror case: a context-only switch on a standup. The override
        // gated, so the override is the provenance.
        sync_note_gated_by(
            &mut ledger,
            "n_a1b2c3",
            "Briarwood Golf",
            DAY_ONE,
            &items,
            Some(EnrollmentMode::ContextOnly),
            Some(EnrollmentMode::Tracked),
        );

        let entries = ledger.list_entries(&EntryFilter::default()).unwrap();
        assert_eq!(entries[0].enrolled_via, EnrolledVia::Override);
    }

    #[test]
    fn a_category_that_tracks_enrolls_everything_as_a_plain_default() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![
            fact("a_111111", "Priya", "send the revised deck"),
            fact("a_222222", "You", "book the venue"),
        ];

        let outcome = sync_note_gated_by(
            &mut ledger,
            "n_a1b2c3",
            "Briarwood Golf",
            DAY_ONE,
            &items,
            None,
            Some(EnrollmentMode::Tracked),
        );

        assert_eq!(outcome.created.len(), 2);
        assert_eq!(outcome.not_enrolled, 0);
        let entries = ledger.list_entries(&EntryFilter::default()).unwrap();
        // Both plain defaults: nothing gated, so nothing claims the credit —
        // and, load-bearingly, both stay sweepable if the meeting is later
        // recategorized into a genre that does gate.
        assert!(entries
            .iter()
            .all(|entry| entry.enrolled_via == EnrolledVia::Default));
    }

    #[test]
    fn new_items_become_open_entries() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![
            fact("a_111111", "Priya", "send the revised deck"),
            fact("a_222222", "You", "book the venue"),
        ];
        let outcome = sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &items);
        assert_eq!(outcome.created.len(), 2);
        assert_eq!(outcome.unchanged, 0);

        let entries = ledger.list_entries(&EntryFilter::default()).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.state == EntryState::Open));
        assert!(entries.iter().all(|e| e.last_mention == DAY_ONE));
        let mine = entries.iter().find(|e| e.owner == "You").unwrap();
        assert_eq!(mine.direction, Direction::Mine);
        let theirs = entries.iter().find(|e| e.owner == "Priya").unwrap();
        assert_eq!(theirs.direction, Direction::Theirs);
    }

    #[test]
    fn re_syncing_an_unchanged_note_writes_nothing() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![fact("a_111111", "Priya", "send the revised deck")];
        sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &items);
        let before = ledger
            .get_entry(
                &ledger.list_entries(&EntryFilter::default()).unwrap()[0]
                    .entry_id
                    .clone(),
            )
            .unwrap()
            .unwrap();

        let outcome = sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &items);
        assert!(outcome.is_noop(), "{outcome:?}");
        assert_eq!(outcome.unchanged, 1);

        let after = ledger.get_entry(&before.entry.entry_id).unwrap().unwrap();
        assert_eq!(before, after, "an unchanged note must not touch the ledger");
    }

    #[test]
    fn a_checkbox_flip_is_invisible_to_the_ledger() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let open = vec![fact("a_111111", "Priya", "send the revised deck")];
        sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &open);
        let entry_id = ledger.list_entries(&EntryFilter::default()).unwrap()[0]
            .entry_id
            .clone();
        let before = ledger.get_entry(&entry_id).unwrap().unwrap();

        // The id is a hash of the parsed line, and `done` is not in it, so a
        // checked box arrives with the same id.
        let mut checked = open.clone();
        checked[0].done = true;
        let outcome = sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &checked);

        assert!(outcome.is_noop());
        assert_eq!(before, ledger.get_entry(&entry_id).unwrap().unwrap());
        assert_eq!(
            before.entry.state,
            EntryState::Open,
            "the checkbox owns done, not the ledger"
        );
    }

    #[test]
    fn editing_a_description_relinks_the_entry_tier_a() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let before = vec![
            fact("a_111111", "Priya", "send the deck"),
            fact("a_333333", "Sam", "book the venue"),
        ];
        sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &before);
        let entry_id = ledger
            .list_entries(&EntryFilter::default())
            .unwrap()
            .into_iter()
            .find(|e| e.owner == "Priya")
            .unwrap()
            .entry_id;

        // The description changed, so the id was re-minted.
        let after = vec![
            fact("a_999999", "Priya", "send the revised deck"),
            fact("a_333333", "Sam", "book the venue"),
        ];
        let outcome = sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &after);

        assert_eq!(outcome.relinked, vec![entry_id.clone()]);
        assert!(outcome.created.is_empty(), "no duplicate entry");
        assert!(outcome.sent_to_review.is_empty(), "not orphaned");

        let detail = ledger.get_entry(&entry_id).unwrap().unwrap();
        assert_eq!(detail.entry.description, "send the revised deck");
        let live: Vec<_> = detail.item_refs.iter().filter(|r| r.active).collect();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].item_id, "a_999999");
        // The old ref is kept as history, not deleted.
        assert!(detail
            .item_refs
            .iter()
            .any(|r| r.item_id == "a_111111" && !r.active));
    }

    #[test]
    fn editing_an_owner_relinks_the_entry_tier_b() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let before = vec![fact("a_111111", "Pryia", "send the deck")];
        sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &before);
        let entry_id = ledger.list_entries(&EntryFilter::default()).unwrap()[0]
            .entry_id
            .clone();

        // Typo fixed in the owner: a different owner key, so only tier B can
        // pair it.
        let after = vec![fact("a_999999", "Priya", "send the deck")];
        let outcome = sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &after);

        assert_eq!(outcome.relinked, vec![entry_id.clone()]);
        let detail = ledger.get_entry(&entry_id).unwrap().unwrap();
        assert_eq!(detail.entry.owner, "Priya");
        assert_eq!(detail.entry.direction, Direction::Theirs);
    }

    #[test]
    fn an_ambiguous_edit_creates_and_reviews_rather_than_guessing() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        // Two commitments for one owner...
        let before = vec![
            fact("a_111111", "Priya", "send the deck"),
            fact("a_222222", "Priya", "book the venue"),
        ];
        sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &before);

        // ...both reworded at once. Two gone, one new for the same owner: the
        // pairing is ambiguous, so nothing is guessed.
        let after = vec![fact("a_999999", "Priya", "send the revised deck")];
        let outcome = sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &after);

        assert!(outcome.relinked.is_empty(), "no guess was made");
        assert_eq!(outcome.created.len(), 1);
        assert_eq!(outcome.sent_to_review.len(), 2);
        for entry_id in &outcome.sent_to_review {
            let detail = ledger.get_entry(entry_id).unwrap().unwrap();
            assert_eq!(detail.entry.state, EntryState::NeedsReview);
            assert!(detail
                .entry
                .review_reason
                .as_deref()
                .unwrap()
                .contains("n_a1b2c3"));
        }
    }

    #[test]
    fn the_same_commitment_in_a_later_meeting_is_one_entry() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let first = vec![fact("a_111111", "Priya", "send the revised deck")];
        sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &first);
        let entry_id = ledger.list_entries(&EntryFilter::default()).unwrap()[0]
            .entry_id
            .clone();

        // Said again in a later meeting: same owner and wording, new note, so a
        // different item id.
        let second = vec![fact("a_222222", "Priya", "send the revised deck")];
        let outcome = sync_note(&mut ledger, "n_d4e5f6", "Briarwood Golf", DAY_TWO, &second);

        assert_eq!(outcome.rementioned, vec![entry_id.clone()]);
        assert!(outcome.created.is_empty(), "one commitment, one entry");

        let detail = ledger.get_entry(&entry_id).unwrap().unwrap();
        assert_eq!(detail.entry.last_mention, DAY_TWO, "aging clock reset");
        assert_eq!(
            detail.item_refs.iter().filter(|r| r.active).count(),
            2,
            "both mentions stay linked"
        );
    }

    #[test]
    fn a_re_mention_revives_an_entry_that_lost_its_line() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![fact("a_111111", "Priya", "send the revised deck")];
        sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &items);
        let entry_id = ledger.list_entries(&EntryFilter::default()).unwrap()[0]
            .entry_id
            .clone();

        // The line is deleted from its note.
        let outcome = sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &[]);
        assert_eq!(outcome.sent_to_review, vec![entry_id.clone()]);

        // A later meeting says it again.
        let again = vec![fact("a_222222", "Priya", "send the revised deck")];
        let outcome = sync_note(&mut ledger, "n_d4e5f6", "Briarwood Golf", DAY_TWO, &again);
        assert_eq!(outcome.rementioned, vec![entry_id.clone()]);

        let detail = ledger.get_entry(&entry_id).unwrap().unwrap();
        assert_eq!(detail.entry.state, EntryState::Open);
        assert_eq!(detail.entry.review_reason, None);
    }

    #[test]
    fn a_closed_commitment_restated_later_is_a_new_commitment() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![fact("a_111111", "Priya", "send the revised deck")];
        sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &items);
        let first = ledger.list_entries(&EntryFilter::default()).unwrap()[0]
            .entry_id
            .clone();
        ledger.close(&first, ClosedVia::Manual, NOW).unwrap();

        let again = vec![fact("a_222222", "Priya", "send the revised deck")];
        let outcome = sync_note(&mut ledger, "n_d4e5f6", "Briarwood Golf", DAY_TWO, &again);

        assert!(outcome.rementioned.is_empty(), "the old judgement stands");
        assert_eq!(outcome.created.len(), 1);
        assert_eq!(
            ledger.get_entry(&first).unwrap().unwrap().entry.state,
            EntryState::Closed
        );
    }

    #[test]
    fn duplicate_lines_collapse_to_one_entry_and_survive_a_deletion() {
        // Two byte-identical lines in one note are one commitment written twice,
        // so exact matching folds them into a single entry holding both refs.
        // That also disarms the occurrence-shift trap: deleting the *first* of
        // the pair re-mints the survivor as occurrence 0, which is the id the
        // deleted line had — but since both ids already belong to the same
        // entry, nothing is orphaned and nothing needs review.
        let mut ledger = Ledger::open_in_memory().unwrap();
        let both = vec![
            fact("a_occ000", "Priya", "send the deck"),
            fact("a_occ111", "Priya", "send the deck"),
        ];
        let outcome = sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &both);
        assert_eq!(outcome.created.len(), 1, "one commitment");
        assert_eq!(outcome.rementioned.len(), 1, "the duplicate joined it");

        let entry_id = ledger.list_entries(&EntryFilter::default()).unwrap()[0]
            .entry_id
            .clone();
        assert_eq!(
            ledger
                .get_entry(&entry_id)
                .unwrap()
                .unwrap()
                .item_refs
                .iter()
                .filter(|r| r.active)
                .count(),
            2
        );

        // One line deleted: the survivor keeps occurrence 0's id.
        let survivor = vec![fact("a_occ000", "Priya", "send the deck")];
        let outcome = sync_note(
            &mut ledger,
            "n_a1b2c3",
            "Briarwood Golf",
            DAY_ONE,
            &survivor,
        );

        assert_eq!(outcome.unchanged, 1);
        assert!(outcome.sent_to_review.is_empty(), "nothing was orphaned");
        assert!(outcome.created.is_empty(), "nothing invented");

        let open = ledger
            .list_entries(&EntryFilter {
                states: Some(vec![EntryState::Open]),
                ..EntryFilter::default()
            })
            .unwrap();
        assert_eq!(open.len(), 1, "exactly one commitment stays live");
        let detail = ledger.get_entry(&entry_id).unwrap().unwrap();
        assert_eq!(detail.item_refs.iter().filter(|r| r.active).count(), 1);
    }

    #[test]
    fn tier_b_pairs_the_leftovers_not_whoever_sorts_first() {
        // Two edits in one pass: Mia's line is reworded (tier A pairs it by
        // owner) and Ana's is both reworded *and* handed to Zoe (only tier B can
        // pair it). The two tiers must keep their own pairings; deriving them
        // from two sorted index sets crosses the wires and hands each entry the
        // other's text.
        let mut ledger = Ledger::open_in_memory().unwrap();
        let before = vec![
            fact("a_a00000", "Ana", "book the venue"),
            fact("a_m00000", "Mia", "send the deck"),
        ];
        sync_note(&mut ledger, "n_a1b2c3", "Ops", DAY_ONE, &before);
        let entries = ledger.list_entries(&EntryFilter::default()).unwrap();
        let ana = entries
            .iter()
            .find(|e| e.owner == "Ana")
            .unwrap()
            .entry_id
            .clone();
        let mia = entries
            .iter()
            .find(|e| e.owner == "Mia")
            .unwrap()
            .entry_id
            .clone();

        let after = vec![
            fact("a_m11111", "Mia", "send the revised deck"),
            fact("a_z11111", "Zoe", "book the venue soon"),
        ];
        sync_note(&mut ledger, "n_a1b2c3", "Ops", DAY_ONE, &after);

        let mia_now = ledger.get_entry(&mia).unwrap().unwrap().entry;
        assert_eq!(mia_now.owner, "Mia");
        assert_eq!(mia_now.description, "send the revised deck");
        let ana_now = ledger.get_entry(&ana).unwrap().unwrap().entry;
        assert_eq!(ana_now.owner, "Zoe");
        assert_eq!(ana_now.description, "book the venue soon");
    }

    #[test]
    fn note_removed_retires_every_ref_without_deleting_entries() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![
            fact("a_111111", "Priya", "send the deck"),
            fact("a_222222", "Sam", "book the venue"),
        ];
        sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &items);

        let outcome = ledger.note_removed("n_a1b2c3", NOW).unwrap();
        assert_eq!(outcome.sent_to_review.len(), 2);

        let entries = ledger.list_entries(&EntryFilter::default()).unwrap();
        assert_eq!(entries.len(), 2, "a commitment outlives its note");
        assert!(entries.iter().all(|e| e.state == EntryState::NeedsReview));
    }

    #[test]
    fn an_entry_follows_the_note_that_last_mentioned_it() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![fact("a_111111", "Priya", "send the deck")];
        sync_note(&mut ledger, "n_a1b2c3", "Inbox", DAY_ONE, &items);
        let entry_id = ledger.list_entries(&EntryFilter::default()).unwrap()[0]
            .entry_id
            .clone();
        assert_eq!(
            ledger.get_entry(&entry_id).unwrap().unwrap().entry.project,
            "Inbox"
        );

        // The note is re-filed; the same items arrive under a real project.
        sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &items);
        assert_eq!(
            ledger.get_entry(&entry_id).unwrap().unwrap().entry.project,
            "Briarwood Golf"
        );
        // Both the old and the new project need a fresh snapshot.
        assert!(ledger.dirty_projects().contains(&"Inbox".to_string()));
        assert!(ledger
            .dirty_projects()
            .contains(&"Briarwood Golf".to_string()));
    }

    #[test]
    fn an_older_meeting_does_not_drag_an_entry_backwards() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let newer = vec![fact("a_222222", "Priya", "send the deck")];
        sync_note(&mut ledger, "n_d4e5f6", "Briarwood Golf", DAY_TWO, &newer);
        let entry_id = ledger.list_entries(&EntryFilter::default()).unwrap()[0]
            .entry_id
            .clone();

        // An older meeting is indexed afterwards (a backfill, say).
        let older = vec![fact("a_111111", "Priya", "send the deck")];
        sync_note(&mut ledger, "n_a1b2c3", "Ops", DAY_ONE, &older);

        let detail = ledger.get_entry(&entry_id).unwrap().unwrap();
        assert_eq!(detail.entry.last_mention, DAY_TWO, "clock never runs back");
        assert_eq!(detail.entry.project, "Briarwood Golf", "project holds");
    }

    /// Syncs with the distill's judgement about which lines restate what.
    fn sync_hinted(
        ledger: &mut Ledger,
        note_id: &str,
        date: &str,
        items: &[ActionItemFact],
        hints: &[LinkHint],
    ) -> SyncOutcome {
        ledger
            .sync_note_items(&NoteSync {
                note_id,
                project: "Briarwood Golf",
                note_date_utc: date,
                link_hints: hints,
                note_override: None,
                category_default: None,
                items,
                now: NOW,
            })
            .unwrap()
    }

    #[test]
    fn a_hint_pairs_a_paraphrase_the_text_could_never_match() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let first = vec![fact("a_111111", "You", "send the deck")];
        let created = sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &first);
        let entry_id = created.created[0].clone();

        // Different words for the same promise: exact matching gives up here,
        // which is precisely why the distill's judgement is worth carrying in.
        let second = vec![fact("a_222222", "You", "send the slide deck")];
        let outcome = sync_hinted(
            &mut ledger,
            "n_d4e5f6",
            DAY_TWO,
            &second,
            &[LinkHint {
                item_id: "a_222222".to_string(),
                entry_id: entry_id.clone(),
            }],
        );

        assert!(outcome.created.is_empty());
        assert_eq!(outcome.rementioned, vec![entry_id.clone()]);
        let entries = ledger.list_entries(&EntryFilter::default()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].last_mention, DAY_TWO);
    }

    #[test]
    fn a_hint_naming_a_settled_entry_falls_through_to_the_ordinary_rules() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let first = vec![fact("a_111111", "You", "send the deck")];
        let created = sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &first);
        let entry_id = created.created[0].clone();
        ledger.waive(&entry_id, NOW).unwrap();

        // A hint is advisory. Reviving a commitment a human waived would
        // erase their judgement, so a stale hint is ignored rather than obeyed.
        let second = vec![fact("a_222222", "You", "send the slide deck")];
        let outcome = sync_hinted(
            &mut ledger,
            "n_d4e5f6",
            DAY_TWO,
            &second,
            &[LinkHint {
                item_id: "a_222222".to_string(),
                entry_id: entry_id.clone(),
            }],
        );

        assert_eq!(outcome.created.len(), 1);
        assert_ne!(outcome.created[0], entry_id);
        assert_eq!(
            ledger.get_entry(&entry_id).unwrap().unwrap().entry.state,
            EntryState::Waived
        );
    }

    #[test]
    fn a_hint_for_an_unknown_entry_is_ignored() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![fact("a_111111", "You", "send the deck")];
        let outcome = sync_hinted(
            &mut ledger,
            "n_a1b2c3",
            DAY_ONE,
            &items,
            &[LinkHint {
                item_id: "a_111111".to_string(),
                entry_id: "le_invented".to_string(),
            }],
        );

        assert_eq!(outcome.created.len(), 1);
    }

    #[test]
    fn a_hint_never_overrides_a_line_that_is_already_linked() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![
            fact("a_111111", "You", "send the deck"),
            fact("a_222222", "Priya", "book the venue"),
        ];
        let created = sync_note(&mut ledger, "n_a1b2c3", "Briarwood Golf", DAY_ONE, &items);
        let deck = created.created[0].clone();

        // Re-syncing the same note with a hint pointing the venue line at the
        // deck's entry: the lines are unchanged, so the earlier tiers pair
        // them first and the hint never gets a say.
        let outcome = sync_hinted(
            &mut ledger,
            "n_a1b2c3",
            DAY_ONE,
            &items,
            &[LinkHint {
                item_id: "a_222222".to_string(),
                entry_id: deck.clone(),
            }],
        );

        assert_eq!(outcome.unchanged, 2);
        assert!(outcome.rementioned.is_empty());
        assert_eq!(
            ledger.list_entries(&EntryFilter::default()).unwrap().len(),
            2
        );
    }
}
