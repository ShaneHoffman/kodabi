//! The heartbeat that keeps capture indicators honest.
//!
//! Every state change driven by a toggle broadcasts itself, but capture can
//! also change state on its own: a device vanishes mid-meeting and the capture
//! thread drops into a silent rebuild-retry loop. Nothing in the toggle path
//! ever runs again, so without this the tray and window would keep claiming
//! "listening" while zero audio is recorded — a silently missed meeting.
//!
//! So a background thread re-derives the truth from `DualCapture` while a
//! capture is engaged and broadcasts whenever it differs from what was last
//! shown. It ticks only while holding the toggle lock, which both keeps it
//! from interleaving with a toggle's own broadcasts and makes it the backstop
//! that drains a coalesced hotkey press no in-flight toggle picked up.

use std::sync::atomic::Ordering;
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::audio_cmds::{event_from_state, CapturePhase, CaptureState, CaptureStateEvent};
use crate::capture_control::{
    broadcast_event, drain_pending_toggles, perform_one_toggle, try_lock_toggle, CaptureController,
    TogglePress,
};

/// How often the truth is re-derived while a capture is engaged. The
/// acceptance bar is "within a few seconds", and a poll is cheap (one mutex +
/// two atomic loads), so a second leaves room for the confirm delay below
/// while staying well inside it.
const WATCHDOG_INTERVAL: Duration = Duration::from_secs(1);

/// How often the truth is re-derived while nothing is installed.
///
/// With no capture running there is nothing that can change state on its own —
/// the silent rebuild-retry loop this watchdog exists to catch only happens to
/// a *live* capture — and every toggle broadcasts its own outcome. So the idle
/// tick is pure belt-and-braces (it re-converges an indicator left stale by a
/// panicked toggle, and clears a stranded `pending_toggle` flag), and polling
/// it once a second would spend ~86k wakes a day next to a dormant tray icon
/// against `docs/RESOURCE_BUDGET.md`'s "idle ≈ zero". Backing off to 10s costs
/// nothing user-visible: a stranded press while idle is a no-op anyway, since
/// a coalesced press can never start a capture.
const IDLE_WATCHDOG_INTERVAL: Duration = Duration::from_secs(10);

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

    /// Forget any part-built degraded confirmation.
    ///
    /// Called whenever a tick can't observe the truth at all (the toggle lock
    /// was busy, state wasn't managed yet, the health read failed), so
    /// "consecutive" always means consecutive *observations*. Without this a
    /// single transient stall seen before a long unobservable gap would pair
    /// with an unrelated stall minutes later and be broadcast as confirmed —
    /// exactly the flash [`DEGRADED_CONFIRM_TICKS`] exists to suppress.
    pub(crate) fn reset(&mut self) {
        self.consecutive_degraded = 0;
    }
}

/// Start the watchdog: tick, sleep, forever. Detached, and dies with the
/// process. Call once from `setup`, after the tray has managed the
/// [`CaptureController`] this reads.
///
/// The interval adapts to whether there is anything to watch — see
/// [`IDLE_WATCHDOG_INTERVAL`].
pub(crate) fn start(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let mut debounce = DegradedDebounce::default();
        loop {
            tick(&app, &mut debounce);
            std::thread::sleep(next_interval(&app));
        }
    });
}

/// How long to sleep before the next tick: the engaged rate while a capture is
/// installed, the backed-off idle rate otherwise. An unreadable state counts as
/// engaged, so uncertainty always polls at the faster rate.
fn next_interval(app: &AppHandle) -> Duration {
    let engaged = match app.try_state::<CaptureState>() {
        Some(state) => state.is_active().unwrap_or(true),
        None => true,
    };
    if engaged {
        WATCHDOG_INTERVAL
    } else {
        IDLE_WATCHDOG_INTERVAL
    }
}

/// One pass: re-derive the truth and broadcast it if it differs from what is
/// on screen, then drain any stranded toggle press.
///
/// Skips entirely while a toggle holds the lock — that toggle is mid-
/// read-decide-act and will broadcast its own outcome, and its `Starting`
/// announcement must not be overwritten by a truth read that predates the
/// start finishing. Every skip path resets the debounce, so a suppressed
/// degraded observation never pairs with one from after the gap.
fn tick(app: &AppHandle, debounce: &mut DegradedDebounce) {
    let (Some(controller), Some(state)) = (
        app.try_state::<CaptureController>(),
        app.try_state::<CaptureState>(),
    ) else {
        debounce.reset();
        return;
    };
    // A poisoned lock is treated as acquirable, so a toggle thread that
    // panicked mid-start (leaving `Starting` on screen forever) is corrected
    // by the next tick rather than wedging the watchdog too.
    let Some(_guard) = try_lock_toggle(&controller) else {
        debounce.reset();
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
        Err(err) => {
            debounce.reset();
            eprintln!("capture watchdog health read failed: {err}");
        }
    }

    // Backstop for the press that set `pending_toggle` in the sliver between
    // the holder's last drain check and its release: without this it would
    // wait for the next press instead of being honored. Every pass here is
    // replaying a press, so it can only stop — never start.
    if controller.pending_toggle.load(Ordering::SeqCst) {
        drain_pending_toggles(
            &controller.pending_toggle,
            TogglePress::Coalesced,
            |press| perform_one_toggle(app, press),
        );
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
    fn an_unobservable_gap_restarts_the_degraded_confirmation() {
        let mut debounce = DegradedDebounce::default();
        // One transient stall is seen and correctly suppressed.
        assert_eq!(debounce.decide(&listening(), mic_stalled()), None);
        // Then the watchdog goes blind for a while — a toggle holds the lock,
        // or the health read fails — so it never observes the recovery that
        // would have cleared the counter.
        debounce.reset();
        // An unrelated stall much later must start its own confirmation, not
        // inherit credit from the one before the gap.
        assert_eq!(debounce.decide(&listening(), mic_stalled()), None);
        assert_eq!(
            debounce.decide(&listening(), mic_stalled()),
            Some(mic_stalled())
        );
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
