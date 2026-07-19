//! The shared capture toggle: the single decision point both the global
//! hotkey and the tray menu drive, so pressing either always flips the same
//! state. Inherently Tauri-coupled (AppHandle, tray, menu, events), so it
//! lives in the shell rather than `kodabi-audio` or `audio_cmds`'s thin
//! command wrappers.

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Wry};
use tauri_plugin_global_shortcut::Shortcut;

use crate::audio_cmds::{
    event_from_state, start_capture_impl, starting_event, stop_capture_and_transcribe,
    CapturePhase, CaptureSources, CaptureState, CaptureStateEvent, SourceState,
};
use crate::settings_cmds::SettingsState;

/// Event the frontend subscribes to for capture state changes — also the
/// consent signal (listening == mic/system audio is live).
pub const CAPTURE_STATE_EVENT: &str = "capture:state";

/// Event emitted when a capture is attempted before the user has acknowledged
/// the recording-consent nudge. The frontend opens the nudge in response; no
/// capture starts until the user acknowledges (which persists consent, then
/// starts capture via `start_capture`).
pub const CONSENT_REQUIRED_EVENT: &str = "consent:required";

/// Payload for [`CONSENT_REQUIRED_EVENT`]. An empty object (rather than no
/// payload) so a future field — which capture control triggered it, a reason —
/// can be added without breaking the frontend contract.
#[derive(Clone, serde::Serialize)]
pub struct ConsentRequiredEvent {}

/// Default global hotkey that starts/stops capture. OS-global — fires even
/// while Kodabi is unfocused. Not yet user-configurable.
pub const DEFAULT_TOGGLE_SHORTCUT: &str = "Ctrl+Shift+K";

/// Managed alongside [`CaptureState`]: serializes the toggle's
/// read-decide-act sequence (a hotkey press racing a tray click must not
/// double-toggle) and holds the dynamic Start/Stop menu item so the toggle
/// can relabel it.
pub struct CaptureController {
    /// Held for the whole read-decide-act of a toggle. Also the outermost lock
    /// in this module: the capture engine's own state lock and
    /// [`CaptureController::last_broadcast`] are only ever taken under it or
    /// alone, never the reverse.
    pub(crate) toggle_lock: Mutex<()>,
    toggle_item: MenuItem<Wry>,
    /// Set when a toggle press arrives while another toggle holds the lock.
    /// The in-flight toggle drains it before releasing, so any number of
    /// mid-flight presses coalesce into exactly one follow-up toggle instead
    /// of being silently dropped.
    pub(crate) pending_toggle: AtomicBool,
    /// The last event handed to [`broadcast_event`]. Seeds [`crate::audio_cmds::capture_phase`]
    /// so a frontend mounting mid-start sees `Starting` (which is never
    /// derivable from backend state), and gives the watchdog the baseline it
    /// compares observed truth against.
    last_broadcast: Mutex<CaptureStateEvent>,
}

impl CaptureController {
    /// The most recently broadcast capture state.
    pub(crate) fn last_broadcast(&self) -> CaptureStateEvent {
        *self
            .last_broadcast
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

enum ToggleAction {
    Start,
    Stop,
    /// Idle, but consent hasn't been acknowledged yet — surface the nudge
    /// instead of recording. Only ever reached on the very first capture.
    RequireConsent,
}

/// Pure toggle decision, factored out of [`toggle_capture`] so it's testable
/// without a running app. Stopping is always allowed; starting requires
/// acknowledged consent, so the first-ever capture surfaces the nudge instead.
fn next_action(active: bool, consent_acknowledged: bool) -> ToggleAction {
    match (active, consent_acknowledged) {
        (true, _) => ToggleAction::Stop,
        (false, true) => ToggleAction::Start,
        (false, false) => ToggleAction::RequireConsent,
    }
}

/// Flip capture state: stop if active, start if idle. Called from both the
/// global-shortcut handler and the tray's menu-item handler, so both
/// controls always drive the same decision.
///
/// Runs on a spawned thread (WASAPI negotiation can block for up to a
/// second). A press arriving while another toggle is in flight is *coalesced*,
/// not dropped: it sets `pending_toggle`, which the in-flight toggle drains
/// before releasing the lock. Any number of mid-flight presses therefore
/// collapse into exactly one follow-up toggle, and no press is ever silently
/// ignored.
pub fn toggle_capture(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        // `try_state` (not `state`) so a hotkey firing before the tray/
        // controller finished initializing is a no-op rather than a panic.
        let Some(controller) = app.try_state::<CaptureController>() else {
            return;
        };
        let Some(_guard) = acquire_for_press(&controller) else {
            // Another toggle holds the lock and has been told about this press.
            return;
        };
        drain_pending_toggles(&controller.pending_toggle, || perform_one_toggle(&app));
    });
}

/// Take the toggle lock for a press, recording the press for the in-flight
/// toggle to pick up if the lock is busy.
///
/// The retry closes the race where the holder releases the lock between our
/// failed `try_lock` and setting the flag: without it that press could be
/// stranded until the watchdog's next tick.
fn acquire_for_press<'a>(
    controller: &'a CaptureController,
) -> Option<std::sync::MutexGuard<'a, ()>> {
    match try_lock_toggle(controller) {
        Some(guard) => Some(guard),
        None => {
            controller.pending_toggle.store(true, Ordering::SeqCst);
            try_lock_toggle(controller)
        }
    }
}

/// `try_lock` the toggle lock, treating a poisoned lock as acquirable — a
/// panicked toggle must not wedge every later press (and leave the indicator
/// stuck on whatever it last broadcast).
pub(crate) fn try_lock_toggle(
    controller: &CaptureController,
) -> Option<std::sync::MutexGuard<'_, ()>> {
    match controller.toggle_lock.try_lock() {
        Ok(guard) => Some(guard),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

/// Run `do_toggle`, then keep running it while presses arrive mid-toggle.
///
/// The flag is cleared *before* each toggle so a press that lands during one
/// is always honored by the next pass; clearing after would let a press be
/// swallowed by the toggle it arrived during. Call with the toggle lock held.
pub(crate) fn drain_pending_toggles(pending: &AtomicBool, mut do_toggle: impl FnMut()) {
    loop {
        pending.store(false, Ordering::SeqCst);
        do_toggle();
        if !pending.load(Ordering::SeqCst) {
            break;
        }
    }
}

/// One read-decide-act toggle. Call with the toggle lock held.
pub(crate) fn perform_one_toggle(app: &AppHandle) {
    let state = app.state::<CaptureState>();

    // Decide from the TRUE backend state, act idempotently. Consent is read
    // fail-safe: a missing settings state (very early startup) counts as
    // NOT acknowledged, so nothing is ever recorded without consent.
    let active = state.is_active().unwrap_or(false);
    let consent = consent_acknowledged(app);
    let result = match next_action(active, consent) {
        ToggleAction::Start => {
            // Announce the start before negotiating: device negotiation can
            // block for ~1s, and an indicator that says nothing for that
            // window is indistinguishable from a press that didn't register.
            broadcast_starting(app, state.inner());
            start_capture_impl(app, &state)
        }
        ToggleAction::Stop => stop_capture_and_transcribe(app, &state),
        ToggleAction::RequireConsent => {
            // Surface the app window and let the frontend open the nudge.
            // No capture starts; the tray/indicator stay idle.
            show_main_window(app);
            let _ = app.emit(CONSENT_REQUIRED_EVENT, ConsentRequiredEvent {});
            broadcast_truth(app, state.inner());
            return;
        }
    };
    if let Err(err) = result {
        eprintln!("capture toggle failed: {err}");
    }

    // Re-derive the resulting state from the backend (not the intended
    // action — a start where both devices failed must still report idle)
    // and push it to the frontend + tray.
    broadcast_truth(app, state.inner());
}

/// Run `action` (a start/stop) under the toggle lock and broadcast the
/// resulting state. The `start_capture`/`stop_capture` IPC commands go
/// through here so they serialize with the hotkey/tray toggle (which holds
/// the same lock) and every path that changes capture state keeps the UI and
/// tray in sync. Unlike the toggle's coalescing, an explicit IPC command
/// blocks for an in-flight toggle rather than folding into it.
///
/// `announce_start` marks an action that begins a capture, so the in-flight
/// negotiation window is announced rather than looking like a dead press.
pub fn run_under_toggle_lock<T>(
    app: &AppHandle,
    state: &CaptureState,
    announce_start: bool,
    action: impl FnOnce(&AppHandle, &CaptureState) -> Result<T, String>,
) -> Result<T, String> {
    let announce = |app: &AppHandle, state: &CaptureState| {
        // Only when this really is a start: re-announcing a start over an
        // already-running capture would flash `Starting` over `Listening`.
        if announce_start && !state.is_active().unwrap_or(false) {
            broadcast_starting(app, state);
        }
    };
    // Serialize against the toggle when the controller is available; if it
    // isn't yet (very early startup), just act — there is no concurrent
    // toggle to race.
    let result = match app.try_state::<CaptureController>() {
        Some(controller) => {
            let _guard = controller
                .toggle_lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            announce(app, state);
            action(app, state)
        }
        None => {
            announce(app, state);
            action(app, state)
        }
    };
    broadcast_truth(app, state);
    result
}

/// Broadcast the true current capture state to the frontend and tray.
///
/// Derived from per-source liveness (not just "is a session installed"), so a
/// device that dropped out mid-session surfaces as degraded instead of the
/// indicator continuing to claim it is listening.
pub(crate) fn broadcast_truth(app: &AppHandle, state: &CaptureState) {
    // A health read that fails leaves us unable to prove anything is being
    // captured, so report idle rather than going silent and leaving a stale
    // "listening" on screen.
    let event = event_from_state(state).unwrap_or_else(|err| {
        eprintln!("capture health read failed: {err}");
        idle_event()
    });
    broadcast_event(app, event);
}

/// Announce an in-flight start: the per-source truth (nothing live yet) with
/// the phase overlaid, so the window and tray show a starting state for the
/// device-negotiation window instead of nothing at all.
fn broadcast_starting(app: &AppHandle, state: &CaptureState) {
    let Ok(health) = state.health() else {
        return;
    };
    broadcast_event(app, starting_event(&health));
}

/// Push one capture state to the frontend and sync the tray (menu label +
/// tooltip) to it. The single point where capture state reaches the UI, so the
/// two can never disagree.
pub(crate) fn broadcast_event(app: &AppHandle, event: CaptureStateEvent) {
    let (label, tooltip) = tray_copy(&event);
    if let Some(controller) = app.try_state::<CaptureController>() {
        *controller
            .last_broadcast
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = event;
        let _ = controller.toggle_item.set_text(label);
    }
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(tooltip));
    }
    let _ = app.emit(CAPTURE_STATE_EVENT, event);
}

/// The state broadcast when nothing is captured.
fn idle_event() -> CaptureStateEvent {
    CaptureStateEvent {
        phase: CapturePhase::Idle,
        sources: CaptureSources {
            loopback: SourceState::Off,
            microphone: SourceState::Off,
        },
    }
}

/// Tray menu label and tooltip for a capture state.
///
/// The tooltip never claims plain listening unless both sources are recording:
/// a degraded capture says which source is down, and a capture with nothing
/// live says it is reconnecting. The label stays a toggle verb in every phase
/// (pressing during a start coalesces into a stop).
fn tray_copy(event: &CaptureStateEvent) -> (&'static str, &'static str) {
    match event.phase {
        CapturePhase::Idle => ("Start capture", "Kodabi: idle"),
        CapturePhase::Starting => ("Stop capture", "Kodabi: starting capture"),
        CapturePhase::Listening => ("Stop capture", "Kodabi: listening"),
        CapturePhase::Degraded => {
            let loopback_live = matches!(event.sources.loopback, SourceState::Live);
            let microphone_live = matches!(event.sources.microphone, SourceState::Live);
            match (loopback_live, microphone_live) {
                (true, false) => (
                    "Stop capture",
                    "Kodabi: mic unavailable, capturing system audio",
                ),
                (false, true) => (
                    "Stop capture",
                    "Kodabi: system audio unavailable, capturing mic",
                ),
                _ => ("Stop capture", "Kodabi: reconnecting audio devices"),
            }
        }
    }
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
        pending_toggle: AtomicBool::new(false),
        // Matches the menu label and tooltip this tray is built with; the
        // first broadcast (or the watchdog's first tick) replaces it.
        last_broadcast: Mutex::new(idle_event()),
    });

    TrayIconBuilder::with_id("main")
        .icon(
            app.default_window_icon()
                .expect("tauri.conf.json bundles a default window icon")
                .clone(),
        )
        .tooltip("Kodabi: idle")
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

/// Whether the user has acknowledged the recording-consent nudge. Reads the
/// managed [`SettingsState`]; a missing state (very early startup, before the
/// settings load in `setup`) is treated as NOT acknowledged — the fail-safe
/// direction, so a capture can never slip through before consent exists.
///
/// `pub(crate)` so the `start_capture` IPC command applies the same gate.
pub(crate) fn consent_acknowledged(app: &AppHandle) -> bool {
    app.try_state::<SettingsState>()
        .map(|state| state.snapshot().consent_acknowledged)
        .unwrap_or(false)
}

pub(crate) fn show_main_window(app: &AppHandle) {
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
    fn next_action_decides_stop_start_or_consent() {
        // Stopping is always allowed, regardless of the consent flag.
        assert!(matches!(next_action(true, true), ToggleAction::Stop));
        assert!(matches!(next_action(true, false), ToggleAction::Stop));
        // Starting requires acknowledged consent; without it, surface the nudge.
        assert!(matches!(next_action(false, true), ToggleAction::Start));
        assert!(matches!(
            next_action(false, false),
            ToggleAction::RequireConsent
        ));
    }

    #[test]
    fn consent_required_event_is_an_empty_object() {
        // Locks the wire shape the frontend's consent listener expects.
        let payload = serde_json::to_string(&ConsentRequiredEvent {}).unwrap();
        assert_eq!(payload, "{}");
    }

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

    #[test]
    fn tray_copy_per_phase() {
        // Idle is the only phase offering "Start"; every other phase is
        // something a press should stop.
        assert_eq!(tray_copy(&idle_event()), ("Start capture", "Kodabi: idle"));
        assert_eq!(
            tray_copy(&event(
                CapturePhase::Starting,
                SourceState::Off,
                SourceState::Off
            )),
            ("Stop capture", "Kodabi: starting capture")
        );
        assert_eq!(
            tray_copy(&event(
                CapturePhase::Listening,
                SourceState::Live,
                SourceState::Live
            )),
            ("Stop capture", "Kodabi: listening")
        );

        // Degraded never says plain "listening" — it says which source is
        // down, so the tooltip can't imply the mic is being recorded when it
        // isn't (and vice versa).
        assert_eq!(
            tray_copy(&event(
                CapturePhase::Degraded,
                SourceState::Live,
                SourceState::Failed
            )),
            (
                "Stop capture",
                "Kodabi: mic unavailable, capturing system audio"
            )
        );
        assert_eq!(
            tray_copy(&event(
                CapturePhase::Degraded,
                SourceState::Stalled,
                SourceState::Live
            )),
            (
                "Stop capture",
                "Kodabi: system audio unavailable, capturing mic"
            )
        );
        // Nothing live at all: never claim any audio is being captured.
        assert_eq!(
            tray_copy(&event(
                CapturePhase::Degraded,
                SourceState::Stalled,
                SourceState::Stalled
            )),
            ("Stop capture", "Kodabi: reconnecting audio devices")
        );
    }

    #[test]
    fn drain_pending_toggles_runs_once_when_nothing_pending() {
        let pending = AtomicBool::new(false);
        let mut runs = 0;
        drain_pending_toggles(&pending, || runs += 1);
        assert_eq!(runs, 1);
        assert!(!pending.load(Ordering::SeqCst));
    }

    #[test]
    fn drain_pending_toggles_coalesces_presses_into_one_extra_toggle() {
        let pending = AtomicBool::new(false);
        let mut runs = 0;
        drain_pending_toggles(&pending, || {
            runs += 1;
            // Several presses land during the first toggle. They must produce
            // exactly one follow-up toggle, not one per press, and must not be
            // dropped.
            if runs == 1 {
                pending.store(true, Ordering::SeqCst);
                pending.store(true, Ordering::SeqCst);
            }
        });
        assert_eq!(runs, 2);
        assert!(!pending.load(Ordering::SeqCst));
    }

    #[test]
    fn default_toggle_shortcut_parses() {
        // Exercises the same parse `run()` relies on — a typo in the
        // constant should fail a test, not a runtime `.expect()`.
        let _ = default_toggle_shortcut();
    }
}
