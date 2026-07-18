//! The shared capture toggle: the single decision point both the global
//! hotkey and the tray menu drive, so pressing either always flips the same
//! state. Inherently Tauri-coupled (AppHandle, tray, menu, events), so it
//! lives in the shell rather than `kodabi-audio` or `audio_cmds`'s thin
//! command wrappers.

use std::str::FromStr;
use std::sync::Mutex;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Wry};
use tauri_plugin_global_shortcut::Shortcut;

use crate::audio_cmds::{
    start_capture_impl, stop_capture_and_transcribe, CapturePhase, CaptureState, CaptureStateEvent,
};

/// Event the frontend subscribes to for capture state changes — also the
/// consent signal (listening == mic/system audio is live).
pub const CAPTURE_STATE_EVENT: &str = "capture:state";

/// Default global hotkey that starts/stops capture. OS-global — fires even
/// while Kodabi is unfocused. Not yet user-configurable.
pub const DEFAULT_TOGGLE_SHORTCUT: &str = "Ctrl+Shift+K";

/// Managed alongside [`CaptureState`]: serializes the toggle's
/// read-decide-act sequence (a hotkey press racing a tray click must not
/// double-toggle) and holds the dynamic Start/Stop menu item so the toggle
/// can relabel it.
pub struct CaptureController {
    toggle_lock: Mutex<()>,
    toggle_item: MenuItem<Wry>,
}

enum ToggleAction {
    Start,
    Stop,
}

/// Pure toggle decision, factored out of [`toggle_capture`] so it's testable
/// without a running app: stop while active, start while idle.
fn next_action(active: bool) -> ToggleAction {
    if active {
        ToggleAction::Stop
    } else {
        ToggleAction::Start
    }
}

/// Flip capture state: stop if active, start if idle. Called from both the
/// global-shortcut handler and the tray's menu-item handler, so both
/// controls always drive the same decision.
///
/// Runs on a spawned thread (WASAPI negotiation can block for up to a
/// second) and coalesces overlapping presses via `try_lock` — a press that
/// arrives mid-toggle is dropped rather than queued, since re-toggling
/// before the in-flight one settles has no well-defined outcome.
pub fn toggle_capture(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        // `try_state` (not `state`) so a hotkey firing before the tray/
        // controller finished initializing is a no-op rather than a panic.
        let Some(controller) = app.try_state::<CaptureController>() else {
            return;
        };
        let Ok(_guard) = controller.toggle_lock.try_lock() else {
            return;
        };
        let state = app.state::<CaptureState>();

        // Decide from the TRUE backend state, act idempotently.
        let active = state.is_active().unwrap_or(false);
        let result = match next_action(active) {
            ToggleAction::Start => start_capture_impl(&state),
            ToggleAction::Stop => stop_capture_and_transcribe(&app, &state),
        };
        if let Err(err) = result {
            eprintln!("capture toggle failed: {err}");
        }

        // Re-derive the resulting phase from the backend (not the intended
        // action — a start where both devices failed must still report idle)
        // and push it to the frontend + tray.
        broadcast_capture_phase(&app, state.inner());
    });
}

/// Run `action` (a start/stop) under the toggle lock and broadcast the
/// resulting phase. The `start_capture`/`stop_capture` IPC commands go
/// through here so they serialize with the hotkey/tray toggle (which holds
/// the same lock) and every path that changes capture state keeps the UI and
/// tray in sync. Unlike the toggle's `try_lock` coalescing, an explicit IPC
/// command blocks for an in-flight toggle rather than being dropped.
pub fn run_under_toggle_lock<T>(
    app: &AppHandle,
    state: &CaptureState,
    action: impl FnOnce(&AppHandle, &CaptureState) -> Result<T, String>,
) -> Result<T, String> {
    // Serialize against the toggle when the controller is available; if it
    // isn't yet (very early startup), just act — there is no concurrent
    // toggle to race.
    let result = match app.try_state::<CaptureController>() {
        Some(controller) => {
            let _guard = controller
                .toggle_lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            action(app, state)
        }
        None => action(app, state),
    };
    broadcast_capture_phase(app, state);
    result
}

/// Broadcast the current capture phase to the frontend and sync the tray
/// (menu label + tooltip) to it. Derives the phase from the true backend
/// state so it can never disagree with what is actually being captured.
fn broadcast_capture_phase(app: &AppHandle, state: &CaptureState) {
    let phase = if state.is_active().unwrap_or(false) {
        CapturePhase::Listening
    } else {
        CapturePhase::Idle
    };

    let (label, tooltip) = match phase {
        CapturePhase::Listening => ("Stop capture", "Kodabi — listening"),
        CapturePhase::Idle => ("Start capture", "Kodabi — idle"),
    };
    if let Some(controller) = app.try_state::<CaptureController>() {
        let _ = controller.toggle_item.set_text(label);
    }
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(tooltip));
    }
    let _ = app.emit(CAPTURE_STATE_EVENT, CaptureStateEvent { phase });
}

/// Build the tray icon, its Start/Stop + Show + Quit menu, and manage the
/// [`CaptureController`] the shared toggle needs. Called once from `setup`.
pub fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let toggle_item =
        MenuItem::with_id(app, "toggle_capture", "Start capture", true, None::<&str>)?;
    let quick_capture_item =
        MenuItem::with_id(app, "quick_capture", "Quick capture", true, None::<&str>)?;
    let show_item = MenuItem::with_id(app, "show", "Show Kodabi", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &toggle_item,
            &quick_capture_item,
            &show_item,
            &PredefinedMenuItem::separator(app)?,
            &quit_item,
        ],
    )?;

    app.manage(CaptureController {
        toggle_lock: Mutex::new(()),
        toggle_item: toggle_item.clone(),
    });

    TrayIconBuilder::with_id("main")
        .icon(
            app.default_window_icon()
                .expect("tauri.conf.json bundles a default window icon")
                .clone(),
        )
        .tooltip("Kodabi — idle")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "toggle_capture" => toggle_capture(app),
            "quick_capture" => crate::quick_capture::show_window(app),
            "show" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // Left-click shows/focuses the window; toggling capture stays a
            // deliberate act (menu item or hotkey only) since it's also the
            // recording-consent signal.
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Parse [`DEFAULT_TOGGLE_SHORTCUT`] into a registerable [`Shortcut`].
pub fn default_toggle_shortcut() -> Shortcut {
    Shortcut::from_str(DEFAULT_TOGGLE_SHORTCUT)
        .expect("DEFAULT_TOGGLE_SHORTCUT is a valid accelerator")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_action_stops_when_active_starts_when_idle() {
        assert!(matches!(next_action(true), ToggleAction::Stop));
        assert!(matches!(next_action(false), ToggleAction::Start));
    }

    #[test]
    fn default_toggle_shortcut_parses() {
        // Exercises the same parse `run()` relies on — a typo in the
        // constant should fail a test, not a runtime `.expect()`.
        let _ = default_toggle_shortcut();
    }
}
