//! The ledger's half of a distill: what the pass is shown before the call, and
//! what is done with what it says afterwards.
//!
//! Both distill legs share this. The meeting pass and the chat pass send the
//! same JSON contract and get back the same classifications, so neither has a
//! reason to hold its own copy of this wiring.
//!
//! Everything here is best effort, deliberately. The note is already on disk by
//! the time the second half runs, and the vault watcher will reconcile it with
//! or without us: a ledger that failed to open, a project whose signals will
//! not load, a claim about a commitment that has since closed — each costs the
//! feature and nothing else. A distill never fails because of this file.

use std::path::Path;

use kodabi_core::distill::{DistilledNote, OpenCommitment};
use kodabi_core::index::normalize_date_to_utc;
use kodabi_core::ledger::{AppliedUpdates, EntryFilter, EntryState};
use kodabi_core::meeting;
use kodabi_core::routing::{RouteGuess, RoutingConfig};
use kodabi_core::settings::LedgerSettings;
use kodabi_core::vault;
use tauri::{AppHandle, Emitter, Manager};

use crate::events::LEDGER_CHANGED_EVENT;
use crate::ledger_state::{LedgerClient, LedgerState, OwnedFollowUp};
use crate::settings_cmds::SettingsState;

/// The live states a conversation may talk about, which is the same set the
/// reconciler is willing to match a new line to.
const SHOWN_STATES: [EntryState; 3] = [
    EntryState::Open,
    EntryState::NeedsReview,
    EntryState::Snoozed,
];

/// Builds the fetcher the distill calls with its pre-call project guess.
///
/// The confidence floor lives here rather than in the pass: the routing
/// threshold is a shell concern (it is resolved from the environment at this
/// boundary), and a guess the router would not file a note on is not one worth
/// spending prompt budget on either.
pub(crate) fn open_commitments_fetcher(
    client: LedgerClient,
    config: RoutingConfig,
) -> impl Fn(&RouteGuess) -> Vec<OpenCommitment> {
    move |guess: &RouteGuess| {
        if guess.confidence < config.effective_threshold() {
            return Vec::new();
        }
        let filter = EntryFilter {
            states: Some(SHOWN_STATES.to_vec()),
            project: Some(guess.project.clone()),
            include_descendants: true,
            ..EntryFilter::default()
        };
        match client.list_details(filter) {
            Ok(details) => details
                .into_iter()
                .map(|detail| OpenCommitment {
                    entry_id: detail.entry.entry_id,
                    owner: detail.entry.owner,
                    description: detail.entry.description,
                })
                .collect(),
            Err(err) => {
                // No ledger, no context, same distill as before it existed.
                eprintln!("distill: couldn't read open commitments: {err:?}");
                Vec::new()
            }
        }
    }
}

/// Applies what a finished distill said about existing commitments.
///
/// Called after the note is written, on the distill's own background thread.
pub(crate) fn apply_after_distill(app: &AppHandle, kb: &Path, distilled: &DistilledNote) {
    if distilled.ledger_updates.is_empty() {
        return;
    }
    let client = app.state::<LedgerState>().client();
    if !client.is_available() {
        return;
    }

    // The note has just been written, so its items are re-derived from what is
    // actually on disk rather than from the model's output: the ids the ledger
    // links are content hashes of the rendered lines.
    let Ok(Some((project, listed))) = vault::find_note_anywhere(kb, &distilled.id) else {
        eprintln!("distill: the note vanished before its commitments could be applied");
        return;
    };
    let Some(facts) = meeting::meeting_facts_for(&listed.note, kb) else {
        return;
    };
    let Ok(note_date_utc) = normalize_date_to_utc(&listed.note.date) else {
        eprintln!("distill: unreadable note date, leaving commitments to the watcher");
        return;
    };

    let threshold = autoclose_threshold(app);
    let settings = app.state::<SettingsState>().snapshot();
    let applied = match client.distill_follow_up(
        OwnedFollowUp {
            note_id: distilled.id.as_str().to_string(),
            project,
            note_date_utc,
            items: facts.action_items,
            updates: distilled.ledger_updates.clone(),
            // Straight off the note this distill just wrote.
            note_override: listed.note.tracking,
            // And the category the same distill classified it as, resolved
            // against the user's settings: a fresh all-hands is gated on its
            // very first sync, not only once someone recategorizes it.
            category_default: kodabi_core::ledger::category_default_for(
                listed.note.category,
                &settings.categories,
            ),
            // From the same snapshot as the category default, so the gate and
            // the direction that feeds it read one settings state.
            identity: settings.identity.owner_identity(),
        },
        threshold,
    ) {
        Ok(applied) => applied,
        Err(err) => {
            eprintln!("distill: couldn't apply commitment updates: {err:?}");
            return;
        }
    };

    close_out_notes(app, kb, distilled, &applied);
    log_summary(&applied);
    let _ = app.emit(LEDGER_CHANGED_EVENT, ());
}

/// Ticks and annotates the source line of every commitment the conversation
/// closed.
///
/// The note being written is not the one edited here: the checkbox belongs to
/// whichever earlier note recorded the promise, which is the whole point of the
/// ledger holding a ref to it.
fn close_out_notes(
    app: &AppHandle,
    kb: &Path,
    distilled: &DistilledNote,
    applied: &AppliedUpdates,
) {
    let today = chrono::Local::now().date_naive().to_string();
    let annotation = annotation_for(distilled);

    for close in &applied.auto_closed {
        let Some((note_id, item_id)) = &close.source_ref else {
            continue;
        };
        let Ok(id) = kodabi_core::note::NoteId::parse(note_id) else {
            continue;
        };
        match vault::set_action_item_done(kb, &id, item_id, true) {
            Ok(vault::SetDoneOutcome::Updated(listed)) => {
                crate::ledger_cmds::reindex_and_broadcast(app, &listed, kb);
            }
            Ok(_) => {}
            Err(err) => {
                eprintln!("distill: couldn't tick a closed commitment: {err}");
                continue;
            }
        }
        // The annotation is what makes the tick answerable later: it names the
        // conversation that claimed it, so a surprised reader can go and check.
        if let Err(err) = vault::annotate_action_item(kb, &id, item_id, &today, &annotation) {
            eprintln!("distill: couldn't annotate a closed commitment: {err}");
        }
    }
}

/// The sentence written under a ticked line.
fn annotation_for(distilled: &DistilledNote) -> String {
    let id = distilled.id.as_str();
    match distilled.title.as_deref() {
        Some(title) => format!("reported done in \"{title}\" ({id})."),
        None => format!("reported done in a conversation ({id})."),
    }
}

/// The user's auto-close confidence, clamped to what the ledger validates.
fn autoclose_threshold(app: &AppHandle) -> f64 {
    let LedgerSettings {
        conversation_autoclose,
        ..
    } = app.state::<SettingsState>().snapshot().ledger;
    if conversation_autoclose.is_finite() {
        conversation_autoclose.clamp(0.0, 1.0)
    } else {
        kodabi_core::ledger::DEFAULT_CONVERSATION_AUTOCLOSE
    }
}

/// One line per distill that touched the ledger, matching `sync_one`'s.
fn log_summary(applied: &AppliedUpdates) {
    eprintln!(
        "distill: ledger {} created, {} re-mentioned, {} refreshed, {} superseded, \
         {} auto-closed, {} parked, {} skipped",
        applied.sync.created.len(),
        applied.sync.rementioned.len(),
        applied.refreshed.len(),
        applied.superseded.len(),
        applied.auto_closed.len(),
        applied.parked.len(),
        applied.skipped.len(),
    );
    for (entry_id, why) in &applied.skipped {
        eprintln!("distill: skipped {entry_id}: {why}");
    }
}
