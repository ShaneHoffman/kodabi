//! What a distill's commitment classifications do to the ledger.
//!
//! The distill pass reads a conversation and says three kinds of thing about
//! commitments the ledger already holds: this one came up again, this one was
//! replaced by that new one, this one was reported done. Turning those into
//! state is this module's whole job, and it is here rather than in the shell
//! because every rule below is a judgement about the ledger — which claim is
//! confident enough to act on, what a supersede needs before it can be linked,
//! what to do when the model names a commitment that has since closed.
//!
//! Order matters and is the reason this is one function rather than a handful
//! of calls the shell makes in sequence. A supersede needs the id of the entry
//! the *new* commitment became, and new entries only exist once the note's own
//! lines have been reconciled — so the sync runs first, here, synchronously,
//! rather than whenever the vault watcher gets to it.
//!
//! Nothing here reads the clock: `now` arrives from the shell like every other
//! ledger mutation ([`super`]).

use crate::distill::{LedgerUpdateDraft, LedgerUpdateKind};
use crate::meeting::ActionItemFact;

use super::sync::{LinkHint, NoteSync, SyncOutcome};
use super::LinkKind;
use super::{EntryState, Evidence, EvidenceSource, Ledger, LedgerEntry, LedgerError, Result};

/// One distilled note, with what its conversation said about existing
/// commitments.
pub struct DistillFollowUp<'a> {
    pub note_id: &'a str,
    /// The note's project slug as written (the Inbox sentinel is valid).
    pub project: &'a str,
    /// The note's `date_utc`: what a mention and an observation are dated by,
    /// deliberately not the wall clock.
    pub note_date_utc: &'a str,
    /// The note's action items, in body order — the order the model's `item`
    /// indexes refer to.
    pub items: &'a [ActionItemFact],
    pub updates: &'a [LedgerUpdateDraft],
    /// The note's tracking override, read straight off the `Note` this distill
    /// just wrote. See [`crate::ledger::sync::NoteSync::note_override`].
    pub note_override: Option<crate::ledger::EnrollmentMode>,
    /// The default this note's meeting category carries, resolved by the shell.
    /// See [`crate::ledger::sync::NoteSync::category_default`].
    pub category_default: Option<crate::ledger::EnrollmentMode>,
    /// Who the local user is. See [`crate::ledger::sync::NoteSync::identity`].
    pub identity: &'a crate::ledger::OwnerIdentity,
}

/// A commitment the conversation closed on its own.
pub struct AutoClose {
    pub entry: LedgerEntry,
    pub evidence: Evidence,
    /// The live source line the closed entry still points at, when it has one:
    /// the checkbox the shell ticks and annotates. `None` when the entry's
    /// line was already gone, in which case there is nothing to write.
    pub source_ref: Option<(String, String)>,
}

/// What one follow-up pass did, for the caller's log line and for the writes
/// it still has to make in the vault.
#[derive(Default)]
pub struct AppliedUpdates {
    /// What syncing the note's own lines did, hints included.
    pub sync: SyncOutcome,
    /// Entries whose mention clock was advanced.
    pub refreshed: Vec<String>,
    /// `(old, new)` pairs, the old now superseded.
    pub superseded: Vec<(String, String)>,
    /// Entries closed by a confident conversation claim.
    pub auto_closed: Vec<AutoClose>,
    /// Entries parked for a human, with the claim attached.
    pub parked: Vec<String>,
    /// `(entry_id, why)` for updates that could not be applied. Never an
    /// error: the model naming a commitment that has since closed is an
    /// ordinary race, not a failure.
    pub skipped: Vec<(String, String)>,
}

/// Applies a distill's classifications, after its note is on disk.
///
/// `autoclose_threshold` is the confidence split: at or below it a completion
/// claim is recorded and the entry parked for a human, above it the claim
/// closes the entry. The caller owns the number (it is the user's setting);
/// [`super::DEFAULT_CONVERSATION_AUTOCLOSE`] is the default it comes from.
///
/// Only a structural failure propagates. A single update that cannot be
/// applied lands in [`AppliedUpdates::skipped`], because one confused
/// classification must not cost the others or the sync that ran first.
pub fn apply_distill_follow_up(
    ledger: &mut Ledger,
    follow_up: &DistillFollowUp<'_>,
    autoclose_threshold: f64,
    now: &str,
) -> Result<AppliedUpdates> {
    let mut applied = AppliedUpdates::default();

    // A refresh naming one of this note's own lines is the dedup case: the
    // model is saying "that new checkbox is the commitment you already have".
    // Handing it to the sync as a hint is what stops a second live entry being
    // minted for one promise.
    let hints: Vec<LinkHint> = follow_up
        .updates
        .iter()
        .filter(|update| update.kind == LedgerUpdateKind::Refresh)
        .filter_map(|update| {
            let item = follow_up.items.get(update.item?)?;
            Some(LinkHint {
                item_id: item.id.clone(),
                entry_id: update.entry_id.clone(),
            })
        })
        .collect();

    applied.sync = ledger.sync_note_items(&NoteSync {
        note_id: follow_up.note_id,
        project: follow_up.project,
        note_date_utc: follow_up.note_date_utc,
        items: follow_up.items,
        link_hints: &hints,
        note_override: follow_up.note_override,
        category_default: follow_up.category_default,
        identity: follow_up.identity,
        now,
    })?;

    // One state change per entry per pass. Two updates about the same
    // commitment can only disagree, and the first one wins rather than the
    // last one silently overwriting it.
    let mut touched: Vec<String> = Vec::new();
    // And one *line* per pass, for the mirror-image case: two commitments the
    // model says were both restated by the same new checkbox. Only one entry
    // can hold a line, so without this the second update would take it off the
    // first and close that first entry out as history.
    let mut claimed_lines: Vec<String> = Vec::new();

    for update in follow_up.updates {
        let entry_id = update.entry_id.as_str();
        if touched.iter().any(|seen| seen == entry_id) {
            applied.skipped.push((
                entry_id.to_string(),
                "already changed by an earlier update in this conversation".to_string(),
            ));
            continue;
        }
        let Some(detail) = ledger.get_entry(entry_id)? else {
            applied
                .skipped
                .push((entry_id.to_string(), "no such entry".to_string()));
            continue;
        };
        if detail.entry.state.is_terminal() {
            applied
                .skipped
                .push((entry_id.to_string(), "already settled".to_string()));
            continue;
        }

        let outcome = match update.kind {
            LedgerUpdateKind::Refresh => {
                apply_refresh(ledger, follow_up, update, &mut claimed_lines, now)
            }
            LedgerUpdateKind::Supersede => apply_supersede(ledger, follow_up, update, now),
            LedgerUpdateKind::Completed => {
                apply_completion(ledger, follow_up, update, autoclose_threshold, now)
            }
        };

        match outcome {
            Ok(Applied::Refreshed) => {
                touched.push(entry_id.to_string());
                applied.refreshed.push(entry_id.to_string());
            }
            Ok(Applied::Superseded(new_entry)) => {
                touched.push(entry_id.to_string());
                applied.superseded.push((entry_id.to_string(), new_entry));
            }
            Ok(Applied::Closed(close)) => {
                touched.push(entry_id.to_string());
                applied.auto_closed.push(*close);
            }
            Ok(Applied::Parked) => {
                touched.push(entry_id.to_string());
                applied.parked.push(entry_id.to_string());
            }
            Ok(Applied::Skipped(why)) => applied.skipped.push((entry_id.to_string(), why)),
            // A transition the table refuses, or an entry that vanished
            // between the check above and the write, is this update's problem
            // and nobody else's.
            Err(
                err @ (LedgerError::IllegalTransition { .. } | LedgerError::EntryNotFound { .. }),
            ) => {
                applied
                    .skipped
                    .push((entry_id.to_string(), err.to_string()));
            }
            Err(err) => return Err(err),
        }
    }

    Ok(applied)
}

/// What one update did, before it is recorded on the outcome.
enum Applied {
    Refreshed,
    Superseded(String),
    Closed(Box<AutoClose>),
    Parked,
    Skipped(String),
}

/// "Still outstanding": advance the clock, change nothing else.
///
/// `claimed_lines` is the lines earlier updates in this pass already paired
/// off. A line can only belong to one entry, so a second update naming it is
/// the model telling us two commitments were restated by one checkbox: the
/// first keeps the line and this one degrades to a bare re-mention, which is
/// the whole truth about it anyway. Taking the line instead would close the
/// first entry out as history over nothing.
fn apply_refresh(
    ledger: &mut Ledger,
    follow_up: &DistillFollowUp<'_>,
    update: &LedgerUpdateDraft,
    claimed_lines: &mut Vec<String>,
    now: &str,
) -> Result<Applied> {
    let line = update
        .item
        .and_then(|index| follow_up.items.get(index))
        .filter(|item| !claimed_lines.contains(&item.id));
    if let Some(item) = line {
        claimed_lines.push(item.id.clone());
        // The hint already linked this line during the sync, which bumped the
        // mention with it. Unless the watcher got to the note first and minted
        // a duplicate for the same line, in which case the two entries have to
        // be reconciled here rather than left both live.
        let holder = ledger.entry_for_item(follow_up.note_id, &item.id)?;
        match holder {
            Some(holder) if holder.entry_id == update.entry_id => return Ok(Applied::Refreshed),
            Some(duplicate) => {
                ledger.relink_item(&update.entry_id, follow_up.note_id, &item.id, now)?;
                // The duplicate now points at nothing. Linking it to the entry
                // that kept the line closes it out as history rather than
                // leaving a second live commitment for one promise.
                if !duplicate.state.is_terminal() {
                    ledger.link_entries(
                        &duplicate.entry_id,
                        &update.entry_id,
                        LinkKind::Supersedes,
                        now,
                    )?;
                }
                return Ok(Applied::Refreshed);
            }
            None => {}
        }
    }
    // A bare re-mention: spoken about, but no line in this note is it.
    ledger.record_mention(&update.entry_id, follow_up.note_date_utc, now)?;
    Ok(Applied::Refreshed)
}

/// "Replaced by that one instead": link the old to the new and close it out.
fn apply_supersede(
    ledger: &mut Ledger,
    follow_up: &DistillFollowUp<'_>,
    update: &LedgerUpdateDraft,
    now: &str,
) -> Result<Applied> {
    let Some(item) = update.item.and_then(|index| follow_up.items.get(index)) else {
        return Ok(Applied::Skipped(
            "no replacement commitment in this note".to_string(),
        ));
    };
    let Some(replacement) = ledger.entry_for_item(follow_up.note_id, &item.id)? else {
        return Ok(Applied::Skipped(
            "the replacement line has no entry".to_string(),
        ));
    };
    if replacement.entry_id == update.entry_id {
        // The sync matched the "replacement" straight back to the same entry,
        // so the two commitments are one and the model called a re-wording a
        // supersede. The mention it already bumped is the whole truth here.
        return Ok(Applied::Skipped(
            "the replacement is the same commitment".to_string(),
        ));
    }
    ledger.link_entries(
        &update.entry_id,
        &replacement.entry_id,
        LinkKind::Supersedes,
        now,
    )?;
    Ok(Applied::Superseded(replacement.entry_id))
}

/// "That's already done": record the claim, then close or park on confidence.
fn apply_completion(
    ledger: &mut Ledger,
    follow_up: &DistillFollowUp<'_>,
    update: &LedgerUpdateDraft,
    autoclose_threshold: f64,
    now: &str,
) -> Result<Applied> {
    // The claim is recorded either way. A parked entry with no evidence
    // attached would be a question with its answer thrown away.
    let evidence = ledger.add_evidence(
        &update.entry_id,
        EvidenceSource::Conversation,
        Some(follow_up.note_id),
        update.confidence,
        follow_up.note_date_utc,
        now,
    )?;

    if update.confidence > autoclose_threshold {
        // The source line has to be read before the close: the shell ticks and
        // annotates that checkbox, and closing does not change which line it is.
        let source_ref = ledger
            .get_entry(&update.entry_id)?
            .and_then(|detail| {
                detail
                    .item_refs
                    .into_iter()
                    .find(|item_ref| item_ref.active)
            })
            .map(|item_ref| (item_ref.note_id, item_ref.item_id));
        let (entry, evidence) =
            ledger.close_from_evidence(&update.entry_id, &evidence.evidence_id, now)?;
        return Ok(Applied::Closed(Box::new(AutoClose {
            entry,
            evidence,
            source_ref,
        })));
    }

    // Not sure enough to act. An entry already in review keeps the question it
    // was parked with; the claim is attached either way.
    if matches!(
        ledger
            .get_entry(&update.entry_id)?
            .map(|detail| detail.entry.state),
        Some(EntryState::Open) | Some(EntryState::Snoozed)
    ) {
        ledger.send_to_review(
            &update.entry_id,
            &format!("a conversation reported this done ({})", follow_up.note_id),
            now,
        )?;
    }
    Ok(Applied::Parked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::distill::LedgerUpdateDraft;
    use crate::ledger::{ClosedVia, EntryFilter, EvidenceSource, LinkKind, UntrackedVia};

    const NOW: &str = "2026-08-17T12:00:00Z";
    const DAY: &str = "2026-08-17T00:00:00Z";
    const EARLIER: &str = "2026-07-01T00:00:00Z";
    const AUTOCLOSE: f64 = 0.8;

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

    fn update(
        entry_id: &str,
        kind: LedgerUpdateKind,
        item: Option<usize>,
        confidence: f64,
    ) -> LedgerUpdateDraft {
        LedgerUpdateDraft {
            entry_id: entry_id.to_string(),
            kind,
            item,
            confidence,
            quote: None,
        }
    }

    /// A ledger holding one open entry, mentioned in an older note.
    fn seeded() -> (Ledger, String) {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![fact("a_old", "You", "send the deck")];
        let outcome = ledger
            .sync_note_items(&NoteSync {
                note_id: "n_old",
                project: "Briarwood Golf",
                note_date_utc: EARLIER,
                items: &items,
                link_hints: &[],
                note_override: None,
                category_default: None,
                identity: &crate::ledger::OwnerIdentity::default(),
                now: EARLIER,
            })
            .unwrap();
        (ledger, outcome.created[0].clone())
    }

    fn apply(
        ledger: &mut Ledger,
        items: &[ActionItemFact],
        updates: &[LedgerUpdateDraft],
    ) -> AppliedUpdates {
        apply_distill_follow_up(
            ledger,
            &DistillFollowUp {
                note_id: "n_new",
                project: "Briarwood Golf",
                note_date_utc: DAY,
                items,
                updates,
                note_override: None,
                category_default: None,
                identity: &crate::ledger::OwnerIdentity::default(),
            },
            AUTOCLOSE,
            NOW,
        )
        .unwrap()
    }

    #[test]
    fn a_paraphrased_re_mention_links_instead_of_duplicating() {
        let (mut ledger, entry_id) = seeded();
        // The new meeting writes the same promise in different words, which
        // exact-text matching would never pair.
        let items = vec![fact("a_new", "You", "send the slide deck")];
        let updates = vec![update(&entry_id, LedgerUpdateKind::Refresh, Some(0), 0.9)];

        let applied = apply(&mut ledger, &items, &updates);

        // One live entry for one promise, now pointing at the newer line.
        assert!(applied.sync.created.is_empty());
        assert_eq!(applied.sync.rementioned, vec![entry_id.clone()]);
        assert_eq!(applied.refreshed, vec![entry_id.clone()]);
        let live = ledger
            .list_entries(&EntryFilter {
                states: Some(vec![EntryState::Open]),
                ..EntryFilter::default()
            })
            .unwrap();
        assert_eq!(live.len(), 1);
        // The clock moved to the newer note, which is the whole point.
        assert_eq!(live[0].last_mention, DAY);
    }

    #[test]
    fn a_bare_re_mention_moves_the_clock_and_nothing_else() {
        let (mut ledger, entry_id) = seeded();
        // Spoken about, but no line in this note is it.
        let updates = vec![update(&entry_id, LedgerUpdateKind::Refresh, None, 0.9)];

        let applied = apply(&mut ledger, &[], &updates);

        assert_eq!(applied.refreshed, vec![entry_id.clone()]);
        let entry = ledger.get_entry(&entry_id).unwrap().unwrap().entry;
        assert_eq!(entry.state, EntryState::Open);
        assert_eq!(entry.last_mention, DAY);
    }

    #[test]
    fn a_re_mention_never_drags_the_clock_backwards() {
        let (mut ledger, entry_id) = seeded();
        // Distilling an old backlog must not make a recent entry look stale,
        // nor a stale one look fresh.
        let applied = apply_distill_follow_up(
            &mut ledger,
            &DistillFollowUp {
                note_id: "n_ancient",
                project: "Briarwood Golf",
                note_date_utc: "2026-01-01T00:00:00Z",
                items: &[],
                updates: &[update(&entry_id, LedgerUpdateKind::Refresh, None, 0.9)],
                note_override: None,
                category_default: None,
                identity: &crate::ledger::OwnerIdentity::default(),
            },
            AUTOCLOSE,
            NOW,
        )
        .unwrap();

        assert_eq!(applied.refreshed, vec![entry_id.clone()]);
        let entry = ledger.get_entry(&entry_id).unwrap().unwrap().entry;
        assert_eq!(entry.last_mention, EARLIER);
    }

    #[test]
    fn a_re_mention_revives_an_entry_parked_for_review() {
        let (mut ledger, entry_id) = seeded();
        ledger
            .send_to_review(&entry_id, "its line vanished", NOW)
            .unwrap();

        apply(
            &mut ledger,
            &[],
            &[update(&entry_id, LedgerUpdateKind::Refresh, None, 0.9)],
        );

        let entry = ledger.get_entry(&entry_id).unwrap().unwrap().entry;
        assert_eq!(entry.state, EntryState::Open);
        assert_eq!(entry.review_reason, None);
    }

    #[test]
    fn a_supersede_links_the_old_commitment_to_its_replacement() {
        let (mut ledger, entry_id) = seeded();
        // "Let's do a video walkthrough instead."
        let items = vec![fact("a_new", "You", "record a video walkthrough")];
        let updates = vec![update(&entry_id, LedgerUpdateKind::Supersede, Some(0), 0.9)];

        let applied = apply(&mut ledger, &items, &updates);

        let new_id = applied.sync.created[0].clone();
        assert_eq!(applied.superseded, vec![(entry_id.clone(), new_id.clone())]);
        let old = ledger.get_entry(&entry_id).unwrap().unwrap();
        assert_eq!(old.entry.state, EntryState::Superseded);
        assert_eq!(old.links_out.len(), 1);
        assert_eq!(old.links_out[0].to_entry, new_id);
        assert_eq!(old.links_out[0].kind, LinkKind::Supersedes);
        // The replacement is live and unencumbered.
        let new = ledger.get_entry(&new_id).unwrap().unwrap();
        assert_eq!(new.entry.state, EntryState::Open);
    }

    #[test]
    fn a_supersede_that_resolves_to_itself_is_left_as_the_re_mention_it_is() {
        let (mut ledger, entry_id) = seeded();
        // The "replacement" is the same commitment word for word, so the sync
        // paired it back to the same entry: the model called a restatement a
        // supersede, and superseding an entry with itself is not a thing.
        let items = vec![fact("a_new", "You", "send the deck")];
        let updates = vec![update(&entry_id, LedgerUpdateKind::Supersede, Some(0), 0.9)];

        let applied = apply(&mut ledger, &items, &updates);

        assert!(applied.superseded.is_empty());
        assert_eq!(applied.skipped.len(), 1);
        let entry = ledger.get_entry(&entry_id).unwrap().unwrap().entry;
        assert_eq!(entry.state, EntryState::Open);
        assert_eq!(entry.last_mention, DAY);
    }

    #[test]
    fn a_confident_completion_claim_closes_the_entry_as_conversation() {
        let (mut ledger, entry_id) = seeded();
        let updates = vec![update(&entry_id, LedgerUpdateKind::Completed, None, 0.95)];

        let applied = apply(&mut ledger, &[], &updates);

        assert_eq!(applied.auto_closed.len(), 1);
        let closed = &applied.auto_closed[0];
        assert_eq!(closed.entry.state, EntryState::Closed);
        // Closed by what actually closed it, never as a manual tick.
        assert_eq!(closed.entry.closed_via, Some(ClosedVia::Conversation));
        assert_eq!(closed.evidence.source, EvidenceSource::Conversation);
        assert_eq!(closed.evidence.reference.as_deref(), Some("n_new"));
        // The line the shell has to tick and annotate.
        assert_eq!(
            closed.source_ref,
            Some(("n_old".to_string(), "a_old".to_string()))
        );
    }

    #[test]
    fn an_unsure_completion_claim_parks_the_entry_with_its_evidence() {
        let (mut ledger, entry_id) = seeded();
        let updates = vec![update(&entry_id, LedgerUpdateKind::Completed, None, 0.5)];

        let applied = apply(&mut ledger, &[], &updates);

        assert!(applied.auto_closed.is_empty());
        assert_eq!(applied.parked, vec![entry_id.clone()]);
        let detail = ledger.get_entry(&entry_id).unwrap().unwrap();
        assert_eq!(detail.entry.state, EntryState::NeedsReview);
        assert!(detail.entry.review_reason.is_some());
        // The claim is kept: a question with its answer discarded is worse
        // than no question.
        assert_eq!(detail.evidence.len(), 1);
        assert_eq!(detail.evidence[0].confidence, 0.5);
    }

    #[test]
    fn the_threshold_is_exclusive_so_the_default_never_closes_on_a_tie() {
        let (mut ledger, entry_id) = seeded();
        let applied = apply(
            &mut ledger,
            &[],
            &[update(
                &entry_id,
                LedgerUpdateKind::Completed,
                None,
                AUTOCLOSE,
            )],
        );

        assert!(applied.auto_closed.is_empty());
        assert_eq!(applied.parked, vec![entry_id]);
    }

    #[test]
    fn an_entry_the_ledger_does_not_have_is_skipped_not_fatal() {
        let (mut ledger, entry_id) = seeded();
        let updates = vec![
            update("le_invented", LedgerUpdateKind::Completed, None, 0.99),
            update(&entry_id, LedgerUpdateKind::Refresh, None, 0.9),
        ];

        let applied = apply(&mut ledger, &[], &updates);

        // The invented one is reported, and the real one beside it still ran.
        assert_eq!(applied.skipped.len(), 1);
        assert_eq!(applied.skipped[0].0, "le_invented");
        assert_eq!(applied.refreshed, vec![entry_id]);
    }

    #[test]
    fn a_settled_commitment_is_never_reopened_by_a_conversation() {
        let (mut ledger, entry_id) = seeded();
        ledger.waive(&entry_id, NOW).unwrap();

        let applied = apply(
            &mut ledger,
            &[],
            &[update(&entry_id, LedgerUpdateKind::Refresh, None, 0.9)],
        );

        assert_eq!(applied.skipped.len(), 1);
        assert_eq!(
            ledger.get_entry(&entry_id).unwrap().unwrap().entry.state,
            EntryState::Waived
        );
    }

    #[test]
    fn an_untracked_commitment_is_left_out_of_the_conversation_entirely() {
        let (mut ledger, entry_id) = seeded();
        ledger
            .untrack(&entry_id, UntrackedVia::Manual, NOW)
            .unwrap();

        // `is_terminal` covering Untracked is what buys this: a commitment the
        // user removed from the working set must not be quietly resurrected,
        // auto-closed, or superseded by a later conversation about it.
        for kind in [
            LedgerUpdateKind::Refresh,
            LedgerUpdateKind::Completed,
            LedgerUpdateKind::Supersede,
        ] {
            let applied = apply(&mut ledger, &[], &[update(&entry_id, kind, None, 0.99)]);
            assert_eq!(applied.skipped.len(), 1, "{kind:?} should be skipped");
            assert_eq!(applied.skipped[0].1, "already settled");
            assert_eq!(
                ledger.get_entry(&entry_id).unwrap().unwrap().entry.state,
                EntryState::Untracked
            );
        }
    }

    #[test]
    fn only_the_first_update_about_one_commitment_is_applied() {
        let (mut ledger, entry_id) = seeded();
        // Two classifications of the same commitment can only disagree.
        let updates = vec![
            update(&entry_id, LedgerUpdateKind::Refresh, None, 0.9),
            update(&entry_id, LedgerUpdateKind::Completed, None, 0.99),
        ];

        let applied = apply(&mut ledger, &[], &updates);

        assert_eq!(applied.refreshed, vec![entry_id.clone()]);
        assert!(applied.auto_closed.is_empty());
        assert_eq!(applied.skipped.len(), 1);
        assert_eq!(
            ledger.get_entry(&entry_id).unwrap().unwrap().entry.state,
            EntryState::Open
        );
    }

    #[test]
    fn a_watcher_that_won_the_race_leaves_one_live_entry_behind() {
        let (mut ledger, entry_id) = seeded();
        // The vault watcher indexed the new note first and, seeing only the
        // text, minted a second entry for the paraphrase.
        let items = vec![fact("a_new", "You", "send the slide deck")];
        ledger
            .sync_note_items(&NoteSync {
                note_id: "n_new",
                project: "Briarwood Golf",
                note_date_utc: DAY,
                items: &items,
                link_hints: &[],
                note_override: None,
                category_default: None,
                identity: &crate::ledger::OwnerIdentity::default(),
                now: NOW,
            })
            .unwrap();

        let applied = apply(
            &mut ledger,
            &items,
            &[update(&entry_id, LedgerUpdateKind::Refresh, Some(0), 0.9)],
        );

        assert_eq!(applied.refreshed, vec![entry_id.clone()]);
        // The original keeps the line, and the duplicate is closed out as
        // history rather than left as a second live promise.
        let live = ledger
            .list_entries(&EntryFilter {
                states: Some(vec![EntryState::Open]),
                ..EntryFilter::default()
            })
            .unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].entry_id, entry_id);
        let superseded = ledger
            .list_entries(&EntryFilter {
                states: Some(vec![EntryState::Superseded]),
                ..EntryFilter::default()
            })
            .unwrap();
        assert_eq!(superseded.len(), 1);
    }

    #[test]
    fn two_refreshes_naming_one_line_never_supersede_each_other() {
        let (mut ledger, first) = seeded();
        // A second open commitment, so the note's single line is the only
        // thing both updates can point at.
        let second = ledger
            .sync_note_items(&NoteSync {
                note_id: "n_other",
                project: "Briarwood Golf",
                note_date_utc: EARLIER,
                items: &[fact("a_other", "You", "book the venue")],
                link_hints: &[],
                note_override: None,
                category_default: None,
                identity: &crate::ledger::OwnerIdentity::default(),
                now: EARLIER,
            })
            .unwrap()
            .created[0]
            .clone();

        // One line, and a model that says both commitments were restated by
        // it. Whichever entry loses the line is still a live promise: the
        // second claim must not close the first out as history.
        let items = vec![fact("a_new", "You", "handle the deck and the venue")];
        let applied = apply(
            &mut ledger,
            &items,
            &[
                update(&first, LedgerUpdateKind::Refresh, Some(0), 0.9),
                update(&second, LedgerUpdateKind::Refresh, Some(0), 0.9),
            ],
        );

        assert_eq!(applied.refreshed, vec![first.clone(), second.clone()]);
        assert!(applied.superseded.is_empty());
        for entry_id in [&first, &second] {
            let entry = ledger.get_entry(entry_id).unwrap().unwrap().entry;
            assert_eq!(
                entry.state,
                EntryState::Open,
                "{entry_id} is still an open promise"
            );
            assert_eq!(entry.last_mention, DAY, "{entry_id} was heard again");
        }
    }

    #[test]
    fn a_note_with_no_classifications_still_syncs_its_own_lines() {
        let (mut ledger, _) = seeded();
        let items = vec![fact("a_new", "Priya", "book the venue")];

        let applied = apply(&mut ledger, &items, &[]);

        assert_eq!(applied.sync.created.len(), 1);
        assert!(applied.refreshed.is_empty());
    }
}
