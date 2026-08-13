//! The capture overlay pill: a small always-on-top window that stays visible
//! over full-screen apps while a capture runs, so a recording is never
//! completely invisible (`docs/FOUNDING_DOC.md` §3.7).
//!
//! Inherently Tauri-coupled (AppHandle, window show/hide, managed state), so
//! only the window control lives here; the visibility rule and the default
//! placement arithmetic are the pure
//! [`kodabi_core::overlay::should_show_overlay`] and
//! [`kodabi_core::overlay::default_pill_position`].
//!
//! **Placement lifecycle.** The pill is parked at its default spot at launch
//! and re-parked on every fresh capture start, so each recording begins from a
//! predictable place. Dragging it mid-recording is respected for the rest of
//! that recording — the reset happens on the next start, never in between (see
//! [`kodabi_core::overlay::is_capture_start`] for which transitions count).
//!
//! **Lock discipline — read before adding anything to [`sync`].** [`sync`] is
//! called from `capture_control::broadcast_event`, which almost always runs
//! with `CaptureController::toggle_lock` held (that lock is documented as the
//! outermost lock in its module). Everything here must therefore stay *leaf*:
//! the settings snapshot and the two mutexes below are taken alone and
//! released immediately, window calls are fire-and-forget, and nothing reaches
//! back for `toggle_lock` or for a blocking window getter such as `is_visible`.
//! Show and hide are idempotent, so no read-before-write is needed to stay
//! correct. Re-placement is the one thing here that *must* block on the main
//! thread — the monitor query is a round trip — so it is deferred through
//! [`park_at_default_deferred`] rather than run inline under that lock.
//!
//! Naming note: the `capture` here is *audio* capture, the same sense as
//! `capture_control.rs`. It is unrelated to `quick_capture.rs`'s text box.

use std::sync::Mutex;

use kodabi_core::overlay::{
    default_pill_position, is_capture_start, should_show_overlay, CaptureOrigin,
};
use tauri::{AppHandle, Manager};

use crate::audio_cmds::{CapturePhase, CaptureStateEvent};
use crate::capture_control::CaptureController;
use crate::settings_cmds::SettingsState;

/// Label of the statically-configured overlay window (`src-tauri/tauri.conf.json`).
/// Pre-created hidden at launch, like the quick-capture window, so a capture
/// start shows it with no webview cold start.
pub const WINDOW_LABEL: &str = "capture-overlay";

/// Per-session overlay state: how the capture that is running now began, and
/// whether one was running as of the last state [`sync`] saw.
///
/// Leaf state by design (see the module-level lock discipline). There is no
/// session dismissal to track: the pill carries no controls, so the only ways to
/// hide one are the persistent setting and stopping the capture — which is what
/// keeps the visibility guarantee from being switchable off per recording.
#[derive(Default)]
pub struct OverlayController {
    origin: Mutex<CaptureOrigin>,
    /// The `capture_active` of the last event [`sync`] handled. Held only to
    /// spot the idle -> active edge that starts a new recording; nothing else
    /// reads it, and it starts `false` because nothing is recording at launch.
    capture_was_active: Mutex<bool>,
}

impl OverlayController {
    fn set_origin(&self, origin: CaptureOrigin) {
        *self
            .origin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = origin;
    }

    fn origin(&self) -> CaptureOrigin {
        *self
            .origin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Record the capture-active state of the event being synced, and say
    /// whether it is the start of a new recording.
    ///
    /// Swap-and-compare in one lock hold, so two broadcasts racing in can't
    /// both read the same stale value and both claim the edge.
    fn observe_capture_active(&self, now_active: bool) -> bool {
        let mut last = self
            .capture_was_active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let was_active = std::mem::replace(&mut *last, now_active);
        is_capture_start(was_active, now_active)
    }
}

/// Record how the capture that is about to start began, so [`sync`] can pick
/// the matching setting.
///
/// Every call site passes [`CaptureOrigin::Manual`] today. Meeting
/// auto-detection will pass [`CaptureOrigin::AutoDetected`] from its own start
/// path; nothing else needs to change for the dormant default-on setting to
/// take effect.
pub fn note_capture_start(app: &AppHandle, origin: CaptureOrigin) {
    if let Some(controller) = app.try_state::<OverlayController>() {
        controller.set_origin(origin);
    }
}

/// Bring the overlay window in line with one capture state: visibility every
/// time, and position when this state is the start of a new recording.
///
/// Called from `capture_control::broadcast_event` — the single point every
/// capture-state change funnels through — so the pill tracks backend truth
/// whichever path (hotkey, tray, IPC, or the watchdog noticing a device died)
/// drove the transition. That funnel is also what makes the start edge
/// detectable here: every transition passes through, so "was idle, now isn't"
/// means a recording just began.
pub(crate) fn sync(app: &AppHandle, event: &CaptureStateEvent) {
    // No controller means startup hasn't reached `.manage` yet; there is no
    // window to sync either.
    let Some(controller) = app.try_state::<OverlayController>() else {
        return;
    };

    let capture_active = event.phase != CapturePhase::Idle;

    // A new recording starts from the default spot, wherever the last one was
    // dragged to. Queued before `apply` below and not run inline: the two go
    // through the same event-loop proxy in order, so the position lands before
    // the show and the pill never flashes at the old place. Done even when the
    // pill will stay hidden, so a setting toggled on mid-capture still finds it
    // where this recording put it.
    //
    // Known imprecision, accepted: `broadcast_truth` reports idle when a health
    // read fails mid-capture, so a failed-then-recovered read looks like a
    // start and re-parks a dragged pill. Rare, cosmetic, and in the honest
    // direction — the UI had just claimed nothing was recording.
    if controller.observe_capture_active(capture_active) {
        park_at_default_deferred(app);
    }

    // A missing settings state (very early startup) counts as "can't prove the
    // user asked for a pill", so nothing is shown — the same fail-safe
    // direction the consent gate takes, applied to the opposite risk.
    let show = match app.try_state::<SettingsState>() {
        Some(settings) => should_show_overlay(
            capture_active,
            controller.origin(),
            settings.snapshot().overlay,
        ),
        None => false,
    };

    apply(app, show);
}

/// Bring the pill in line with a just-changed overlay setting, so it appears or
/// disappears mid-capture instead of waiting for the next start.
pub(crate) fn apply_settings_change(app: &AppHandle) {
    resync(app);
}

/// Re-derive visibility from the last broadcast capture state.
///
/// Used when something other than a capture transition changes the answer —
/// today, the user toggling the setting — so the pill appears or disappears
/// mid-capture instead of waiting for the next start/stop.
fn resync(app: &AppHandle) {
    // Reading `last_broadcast` is safe without the toggle lock: it is its own
    // leaf mutex. Unlike [`sync`], this path never runs under `toggle_lock`.
    let Some(controller) = app.try_state::<CaptureController>() else {
        return;
    };
    let event = controller.last_broadcast();
    sync(app, &event);
}

/// Park the pill at its default spot: near the top-center of the primary
/// monitor, clear of the title bars and menus most full-screen apps put in the
/// corners.
///
/// **Blocks on the main thread** — `primary_monitor` and `outer_size` are
/// round trips — so it is only ever called where blocking is free: from setup,
/// before any capture exists, and from the main thread through
/// [`park_at_default_deferred`]. Never call it inline from [`sync`], which
/// usually runs under `toggle_lock`.
pub fn park_at_default(app: &AppHandle) {
    let Some(window) = app.get_webview_window(WINDOW_LABEL) else {
        return;
    };
    // Every step here is best-effort: a monitor query that fails just leaves
    // the pill where it is, which is still usable and still draggable.
    let Ok(Some(monitor)) = window.primary_monitor() else {
        return;
    };
    let Ok(size) = window.outer_size() else {
        return;
    };
    let screen = monitor.size();
    let origin = monitor.position();
    let (x, y) = default_pill_position(
        (origin.x, origin.y),
        (screen.width, screen.height),
        (size.width, size.height),
    );
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
}

/// Queue a [`park_at_default`] on the main thread and return immediately, so
/// its blocking monitor query never runs under a caller's lock.
///
/// Fire-and-forget like the rest of this module: a failed queue (the event loop
/// is gone) just leaves the pill where it is. The closure takes no locks, which
/// is what makes this safe when the caller is *already* the main thread — Tauri
/// runs it inline in that case, so anything it locked could deadlock against a
/// lock the caller holds.
fn park_at_default_deferred(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        park_at_default(&handle);
    });
}

/// Show or hide the overlay window. Fire-and-forget: a window that isn't built
/// yet, or a call that races teardown, must not take down the capture path that
/// called us.
fn apply(app: &AppHandle, show: bool) {
    let Some(window) = app.get_webview_window(WINDOW_LABEL) else {
        return;
    };
    if show {
        let _ = window.show();
    } else {
        let _ = window.hide();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_defaults_to_manual_and_round_trips() {
        let controller = OverlayController::default();
        // The conservative default: an unannounced capture reads the
        // default-off manual setting, never the dormant default-on auto one.
        assert_eq!(controller.origin(), CaptureOrigin::Manual);

        controller.set_origin(CaptureOrigin::AutoDetected);
        assert_eq!(controller.origin(), CaptureOrigin::AutoDetected);

        controller.set_origin(CaptureOrigin::Manual);
        assert_eq!(controller.origin(), CaptureOrigin::Manual);
    }

    #[test]
    fn a_capture_start_edge_fires_once_and_rearms_after_idle() {
        let controller = OverlayController::default();

        // Nothing is recording at launch, so the first active state is a start.
        assert!(controller.observe_capture_active(true));
        // Every later broadcast of a running capture — the `Starting` ->
        // `Listening` follow-up, a watchdog re-broadcast, a settings resync
        // replaying the last event, an idempotent re-start — is not.
        assert!(!controller.observe_capture_active(true));
        // Stopping is not a start either.
        assert!(!controller.observe_capture_active(false));
        // And the next recording gets its reset.
        assert!(controller.observe_capture_active(true));
    }
}
