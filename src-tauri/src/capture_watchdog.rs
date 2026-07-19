//! The heartbeat that keeps capture indicators honest.
//!
//! Every state change driven by a toggle broadcasts itself, but capture can
//! also change state on its own: a device vanishes mid-meeting and the capture
//! thread drops into a silent rebuild-retry loop. Nothing in the toggle path
//! ever runs again, so without this the tray and window would keep claiming
//! "listening" while zero audio is recorded — a silently missed meeting.
//!
//! So a background thread re-derives the truth from `DualCapture` once a
//! second and broadcasts whenever it differs from what was last shown. It
//! ticks only while holding the toggle lock, which both keeps it from
//! interleaving with a toggle's own broadcasts and makes it the backstop that
//! drains a coalesced hotkey press no in-flight toggle picked up.

use std::sync::atomic::Ordering;
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::audio_cmds::{event_from_state, CapturePhase, CaptureState, CaptureStateEvent};
use crate::capture_control::{
    broadcast_event, drain_pending_toggles, perform_one_toggle, try_lock_toggle, CaptureController,
};

/// How often the truth is re-derived. The acceptance bar is "within a few
/// seconds", and a poll is cheap (one mutex + two atomic loads), so a second
/// leaves room for the confirm delay below while staying well inside it.
const WATCHDOG_INTERVAL: Duration = Duration::from_secs(1);

/// Consecutive ticks a newly-degraded state must persist before it is shown.
///
/// A device change (switching headsets, a USB mic re-enumerating) makes the
/// stream stall for a few hundred milliseconds while the capture thread
/// rebuilds it, and that recovers on its own. Confirming across two ticks
/// keeps a routine rebuild from flashing a scary degraded state, while a real
/// loss still surfaces in ~2-3s. Recovery is never delayed this way.
const DEGRADED_CONFIRM_TICKS: u8 = 2;

/// Decides whether an observed state is worth broadcasting, damping the
/// transient stall of a self-healing device rebuild.
///
/// Pure, so every transition is unit-testable without audio hardware.
#[derive(Default)]
pub(crate) struct DegradedDebounce {
    consecutive_degraded: u8,
}

impl DegradedDebounce {
    /// The event to broadcast, if any, given what was last shown and what is
    /// true now.
    ///
    /// Only *entering* degraded waits for confirmation. Everything else —
    /// recovery, a stop, a change in which source is down — broadcasts
    /// immediately: delaying good news, or a change in what's wrong, would be
    /// its own kind of lying.
    pub(crate) fn decide(
        &mut self,
        last_broadcast: &CaptureStateEvent,
        observed: CaptureStateEvent,
    ) -> Option<CaptureStateEvent> {
        if observed == *last_broadcast {
            self.consecutive_degraded = 0;
            return None;
        }

        let entering_degraded = observed.phase == CapturePhase::Degraded
            && last_broadcast.phase != CapturePhase::Degraded;
        if entering_degraded {
            self.consecutive_degraded = self.consecutive_degraded.saturating_add(1);
            if self.consecutive_degraded < DEGRADED_CONFIRM_TICKS {
                return None;
            }
        }

        self.consecutive_degraded = 0;
        Some(observed)
    }
}

/// Start the watchdog: one tick every [`WATCHDOG_INTERVAL`], forever. Detached,
/// and dies with the process. Call once from `setup`, after the tray has
/// managed the [`CaptureController`] this reads.
pub(crate) fn start(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let mut debounce = DegradedDebounce::default();
        loop {
            tick(&app, &mut debounce);
            std::thread::sleep(WATCHDOG_INTERVAL);
        }
    });
}

/// One pass: re-derive the truth and broadcast it if it differs from what is
/// on screen, then drain any stranded toggle press.
///
/// Skips entirely while a toggle holds the lock — that toggle is mid-
/// read-decide-act and will broadcast its own outcome, and its `Starting`
/// announcement must not be overwritten by a truth read that predates the
/// start finishing.
fn tick(app: &AppHandle, debounce: &mut DegradedDebounce) {
    let (Some(controller), Some(state)) = (
        app.try_state::<CaptureController>(),
        app.try_state::<CaptureState>(),
    ) else {
        return;
    };
    // A poisoned lock is treated as acquirable, so a toggle thread that
    // panicked mid-start (leaving `Starting` on screen forever) is corrected
    // by the next tick rather than wedging the watchdog too.
    let Some(_guard) = try_lock_toggle(&controller) else {
        return;
    };

    match event_from_state(&state) {
        Ok(observed) => {
            if let Some(event) = debounce.decide(&controller.last_broadcast(), observed) {
                broadcast_event(app, event);
            }
        }
        // A failed health read proves nothing either way; leave the current
        // state alone and try again next tick.
        Err(err) => eprintln!("capture watchdog health read failed: {err}"),
    }

    // Backstop for the press that set `pending_toggle` in the sliver between
    // the holder's last drain check and its release: without this it would
    // wait for the next press instead of being honored.
    if controller.pending_toggle.load(Ordering::SeqCst) {
        drain_pending_toggles(&controller.pending_toggle, || perform_one_toggle(app));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_cmds::{CaptureSources, SourceState};

    fn event(
        phase: CapturePhase,
        loopback: SourceState,
        microphone: SourceState,
    ) -> CaptureStateEvent {
        CaptureStateEvent {
            phase,
            sources: CaptureSources {
                loopback,
                microphone,
            },
        }
    }

    fn listening() -> CaptureStateEvent {
        event(
            CapturePhase::Listening,
            SourceState::Live,
            SourceState::Live,
        )
    }

    fn idle() -> CaptureStateEvent {
        event(CapturePhase::Idle, SourceState::Off, SourceState::Off)
    }

    fn mic_stalled() -> CaptureStateEvent {
        event(
            CapturePhase::Degraded,
            SourceState::Live,
            SourceState::Stalled,
        )
    }

    #[test]
    fn no_change_is_silent() {
        let mut debounce = DegradedDebounce::default();
        assert_eq!(debounce.decide(&listening(), listening()), None);
        assert_eq!(debounce.decide(&listening(), listening()), None);
    }

    #[test]
    fn degraded_needs_two_consecutive_ticks() {
        let mut debounce = DegradedDebounce::default();
        // A single stalled tick could just be a device rebuild in progress.
        assert_eq!(debounce.decide(&listening(), mic_stalled()), None);
        // Still stalled a second later: real, and the user must be told.
        assert_eq!(
            debounce.decide(&listening(), mic_stalled()),
            Some(mic_stalled())
        );
    }

    #[test]
    fn fast_recovery_never_broadcasts_degraded() {
        let mut debounce = DegradedDebounce::default();
        assert_eq!(debounce.decide(&listening(), mic_stalled()), None);
        // The rebuild succeeded before the confirm window elapsed, so the
        // indicator never flickered — and the counter must not carry over.
        assert_eq!(debounce.decide(&listening(), listening()), None);
        assert_eq!(debounce.decide(&listening(), mic_stalled()), None);
    }

    #[test]
    fn recovery_broadcasts_immediately() {
        let mut debounce = DegradedDebounce::default();
        assert_eq!(
            debounce.decide(&mic_stalled(), listening()),
            Some(listening())
        );
    }

    #[test]
    fn source_change_while_degraded_broadcasts_immediately() {
        let mut debounce = DegradedDebounce::default();
        // Already degraded, but now it's the other source that's down: the
        // copy names a specific source, so this has to be corrected at once.
        let loopback_stalled = event(
            CapturePhase::Degraded,
            SourceState::Stalled,
            SourceState::Live,
        );
        assert_eq!(
            debounce.decide(&mic_stalled(), loopback_stalled),
            Some(loopback_stalled)
        );
    }

    #[test]
    fn idle_transitions_broadcast_immediately() {
        let mut debounce = DegradedDebounce::default();
        assert_eq!(debounce.decide(&listening(), idle()), Some(idle()));
        assert_eq!(debounce.decide(&idle(), listening()), Some(listening()));
    }
}
