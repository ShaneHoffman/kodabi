//! The capture overlay pill: a small always-on-top window that stays visible
//! over full-screen apps while a capture runs, so a recording is never
//! completely invisible (`docs/FOUNDING_DOC.md` §3.7).
//!
//! Inherently Tauri-coupled (AppHandle, window show/hide, managed state), so
//! only the window control lives here; the visibility rule itself is the pure
//! [`kodabi_core::overlay::should_show_overlay`].
//!
//! **Lock discipline — read before adding anything to [`sync`].** [`sync`] is
//! called from `capture_control::broadcast_event`, which almost always runs
//! with `CaptureController::toggle_lock` held (that lock is documented as the
//! outermost lock in its module). Everything here must therefore stay *leaf*:
//! the settings snapshot and the origin mutex below are taken alone and
//! released immediately, window calls are fire-and-forget, and nothing reaches
//! back for `toggle_lock` or for a blocking window getter such as `is_visible`.
//! Show and hide are idempotent, so no read-before-write is needed to stay
//! correct.
//!
//! Naming note: the `capture` here is *audio* capture, the same sense as
//! `capture_control.rs`. It is unrelated to `quick_capture.rs`'s text box.

use std::sync::Mutex;

use kodabi_core::overlay::{should_show_overlay, CaptureOrigin};
use tauri::{AppHandle, Manager};

use crate::audio_cmds::{CapturePhase, CaptureStateEvent};
use crate::capture_control::CaptureController;
use crate::settings_cmds::SettingsState;

/// Label of the statically-configured overlay window (`src-tauri/tauri.conf.json`).
/// Pre-created hidden at launch, like the quick-capture window, so a capture
/// start shows it with no webview cold start.
pub const WINDOW_LABEL: &str = "capture-overlay";

/// Per-session overlay state: how the capture that is running now began.
///
/// Leaf state by design (see the module-level lock discipline). There is no
/// session dismissal to track: the pill carries no controls, so the only ways to
/// hide one are the persistent setting and stopping the capture — which is what
/// keeps the visibility guarantee from being switchable off per recording.
#[derive(Default)]
pub struct OverlayController {
    origin: Mutex<CaptureOrigin>,
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

/// Bring the overlay window in line with one capture state.
///
/// Called from `capture_control::broadcast_event` — the single point every
/// capture-state change funnels through — so the pill tracks backend truth
/// whichever path (hotkey, tray, IPC, or the watchdog noticing a device died)
/// drove the transition.
pub(crate) fn sync(app: &AppHandle, event: &CaptureStateEvent) {
    // No controller means startup hasn't reached `.manage` yet; there is no
    // window to sync either.
    let Some(controller) = app.try_state::<OverlayController>() else {
        return;
    };

    let capture_active = event.phase != CapturePhase::Idle;

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

/// Park the pill near the top-center of the primary monitor, clear of the
/// title bars and menus most full-screen apps put in the corners.
///
/// Called once from setup rather than lazily on first show: at that point no
/// capture is running and no lock is held, so the monitor query — which does a
/// main-thread round trip — is free to block. Doing it from [`sync`] would put
/// exactly that round trip under `toggle_lock`. The window is only ever hidden,
/// never destroyed, so a position the user drags to survives for the run.
pub fn place_initially(app: &AppHandle) {
    let Some(window) = app.get_webview_window(WINDOW_LABEL) else {
        return;
    };
    // Every step here is best-effort: a monitor query that fails just leaves
    // the pill at the OS default placement, which is still usable and still
    // draggable.
    let Ok(Some(monitor)) = window.primary_monitor() else {
        return;
    };
    let Ok(size) = window.outer_size() else {
        return;
    };
    let screen = monitor.size();
    let origin = monitor.position();
    let margin = (screen.height as f64 * 0.04) as i32;
    let x = origin.x + ((screen.width as i32 - size.width as i32) / 2).max(0);
    let y = origin.y + margin;
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
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
}
