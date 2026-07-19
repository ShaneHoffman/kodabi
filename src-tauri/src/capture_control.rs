//! The shared capture toggle: the single decision point both the global
//! hotkey and the tray menu drive, so pressing either always flips the same
//! state. Inherently Tauri-coupled (AppHandle, tray, menu, events), so it
//! lives in the shell rather than `kodabi-audio` or `audio_cmds`'s thin
//! command wrappers.

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tauri::image::Image;
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
    /// of being silently dropped. That follow-up may only ever *stop* — see
    /// [`TogglePress`].
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

#[derive(Debug, PartialEq, Eq)]
enum ToggleAction {
    Start,
    Stop,
    /// Idle, but consent hasn't been acknowledged yet — surface the nudge
    /// instead of recording. Only ever reached on the very first capture.
    RequireConsent,
    /// Do nothing at all. A replayed press that would have started a capture
    /// (see [`TogglePress::Coalesced`]).
    Nothing,
}

/// Where a toggle came from: a press acting on the state the user could see,
/// or one replayed after the fact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TogglePress {
    /// The press that acquired the toggle lock. Acts as a full toggle.
    Direct,
    /// A press that landed while another toggle held the lock and is being
    /// replayed once it finished.
    ///
    /// It may only ever *stop* capture, never start one. By the time it runs,
    /// the state it was aimed at is gone: a press meant to interrupt a stop
    /// would otherwise be replayed against the now-idle engine and begin a
    /// brand-new recording the user never asked for. Since the mark and tray
    /// are the consent signal, recording must only ever begin from a press
    /// acting on state the user was actually looking at.
    Coalesced,
}

/// Pure toggle decision, factored out of [`toggle_capture`] so it's testable
/// without a running app. Stopping is always allowed; starting requires
/// acknowledged consent (so the first-ever capture surfaces the nudge instead)
/// *and* a direct press.
fn next_action(active: bool, consent_acknowledged: bool, press: TogglePress) -> ToggleAction {
    match (active, consent_acknowledged, press) {
        // Stopping is always honored, however the press arrived — a press that
        // means "stop recording" must never be dropped.
        (true, _, _) => ToggleAction::Stop,
        // Nothing is running and this press is a replay: never start.
        (false, _, TogglePress::Coalesced) => ToggleAction::Nothing,
        (false, true, TogglePress::Direct) => ToggleAction::Start,
        (false, false, TogglePress::Direct) => ToggleAction::RequireConsent,
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
/// collapse into exactly one follow-up toggle, and a press that means "stop"
/// is never silently ignored. A coalesced press can only ever stop, so an
/// impatient double-press can't start a recording (see [`TogglePress`]).
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
        drain_pending_toggles(&controller.pending_toggle, TogglePress::Direct, |press| {
            perform_one_toggle(&app, press)
        });
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
/// `first` is how the press that acquired the lock arrived; every pass after
/// it is replaying a press that landed mid-toggle, so it is
/// [`TogglePress::Coalesced`] and can only stop.
///
/// The flag is cleared *before* each toggle so a press that lands during one
/// is always honored by the next pass; clearing after would let a press be
/// swallowed by the toggle it arrived during. Call with the toggle lock held.
pub(crate) fn drain_pending_toggles(
    pending: &AtomicBool,
    first: TogglePress,
    mut do_toggle: impl FnMut(TogglePress),
) {
    let mut press = first;
    loop {
        pending.store(false, Ordering::SeqCst);
        do_toggle(press);
        if !pending.load(Ordering::SeqCst) {
            break;
        }
        press = TogglePress::Coalesced;
    }
}

/// One read-decide-act toggle. Call with the toggle lock held.
pub(crate) fn perform_one_toggle(app: &AppHandle, press: TogglePress) {
    let state = app.state::<CaptureState>();

    // Decide from the TRUE backend state, act idempotently. Consent is read
    // fail-safe: a missing settings state (very early startup) counts as
    // NOT acknowledged, so nothing is ever recorded without consent.
    let active = state.is_active().unwrap_or(false);
    let consent = consent_acknowledged(app);
    let result = match next_action(active, consent, press) {
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
        // A replayed press with nothing to stop. Nothing changed, and the
        // caller already has the indicator converged, so don't re-broadcast.
        ToggleAction::Nothing => return,
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
        if should_announce_start(announce_start, state.is_active().unwrap_or(false)) {
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

/// Whether an action about to run under the toggle lock should announce a
/// start. Only when it really is one: re-announcing over an already-running
/// capture would flash `Starting` over `Listening`.
fn should_announce_start(announce_start: bool, active: bool) -> bool {
    announce_start && !active
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
///
/// A failed health read must not turn that announcement back into silence —
/// going quiet for the ~1s negotiation window is the exact failure `Starting`
/// exists to remove — so it falls back to announcing the start with no source
/// claimed live, which is the honest reading of "we can't tell yet".
fn broadcast_starting(app: &AppHandle, state: &CaptureState) {
    let event = match state.health() {
        Ok(health) => starting_event(&health),
        Err(err) => {
            eprintln!("capture health read failed while starting: {err}");
            starting_event_with_no_source_live()
        }
    };
    broadcast_event(app, event);
}

/// Push one capture state to the frontend and sync the tray (menu label,
/// tooltip and icon) to it. The single point where capture state reaches the
/// UI, so they can never disagree.
pub(crate) fn broadcast_event(app: &AppHandle, event: CaptureStateEvent) {
    let (label, tooltip) = tray_copy(&event);
    // No controller means no tray either (`build_tray` manages the controller
    // before building the tray), so the fallback is unreachable in practice
    // and merely redundant if it ever isn't.
    let mut update_icon = true;
    if let Some(controller) = app.try_state::<CaptureController>() {
        let previous = {
            let mut last = controller
                .last_broadcast
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            std::mem::replace(&mut *last, event)
        };
        update_icon = icon_needs_update(&previous, &event);
        let _ = controller.toggle_item.set_text(label);
    }
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(tooltip));
        if update_icon {
            let _ = tray.set_icon(Some(tray_icon_image(tray_icon_kind(&event))));
        }
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

/// [`idle_event`]'s `Starting` counterpart: a start is under way but the
/// per-source truth couldn't be read. Claims no source is live, so it can
/// only ever under-report what is being recorded.
fn starting_event_with_no_source_live() -> CaptureStateEvent {
    CaptureStateEvent {
        phase: CapturePhase::Starting,
        ..idle_event()
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

/// Which SpiritMark the tray wears. The window can be hidden or closed, so
/// the tray icon is often the only surface answering "am I being recorded?" —
/// a tooltip needs a hover and a menu label needs a click, but the icon is
/// read at a glance.
///
/// The three marks are `src/captureLabel.ts`'s `markMode` collapsed to what a
/// static icon can carry: `Starting` and `reconnecting` are one ink mark here
/// because both mean "engaged, nothing recorded yet" and neither may wear the
/// green (`docs/SPIRIT_MARK.md` — the green means audio is actually being
/// recorded, and nothing else).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrayIconKind {
    /// No capture is running: the still ink disc, the resting logo.
    Idle,
    /// A capture is engaged but nothing reaches disk (starting, or every
    /// source dropped and is rebuilding): the hollow ink ring. Visibly
    /// neither idle nor on air.
    Engaged,
    /// Audio is genuinely being recorded: the green disc with its aura.
    Recording,
}

/// The mark a capture state calls for.
///
/// Degraded splits: with a source still live, audio *is* being captured, so
/// the green stays (withdrawing it would falsely suggest privacy); with
/// nothing live, nothing is recorded, so the green goes.
fn tray_icon_kind(event: &CaptureStateEvent) -> TrayIconKind {
    match event.phase {
        CapturePhase::Idle => TrayIconKind::Idle,
        CapturePhase::Starting => TrayIconKind::Engaged,
        CapturePhase::Listening => TrayIconKind::Recording,
        CapturePhase::Degraded => {
            let any_live = matches!(event.sources.loopback, SourceState::Live)
                || matches!(event.sources.microphone, SourceState::Live);
            if any_live {
                TrayIconKind::Recording
            } else {
                TrayIconKind::Engaged
            }
        }
    }
}

/// The 32x32 asset for a mark, embedded at compile time.
///
/// Drawn by `scripts/generate-tray-icons.ps1` from the `design/tokens.css`
/// values — regenerate there rather than editing the PNGs, and touch this
/// file afterwards (cargo doesn't track `include_image!` inputs, so a stale
/// embedding otherwise survives the rebuild).
fn tray_icon_image(kind: TrayIconKind) -> Image<'static> {
    match kind {
        TrayIconKind::Idle => tauri::include_image!("icons/tray/tray-idle.png"),
        TrayIconKind::Engaged => tauri::include_image!("icons/tray/tray-engaged.png"),
        TrayIconKind::Recording => tauri::include_image!("icons/tray/tray-recording.png"),
    }
}

/// Whether a broadcast actually changes the mark.
///
/// Most broadcasts don't: every IPC command re-broadcasts the truth, and the
/// watchdog re-broadcasts whenever per-source health shifts (a degraded
/// capture flapping stalled/failed keeps the same mark). Setting the icon
/// rebuilds the tray's native icon handle, which can blink it, so a
/// same-mark broadcast leaves it alone.
fn icon_needs_update(previous: &CaptureStateEvent, next: &CaptureStateEvent) -> bool {
    tray_icon_kind(previous) != tray_icon_kind(next)
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
        // Matches the menu label, tooltip and icon this tray is built with —
        // also the baseline `icon_needs_update` compares the first broadcast
        // against, so the stored state and the shown mark agree from the
        // start. The first broadcast (or the watchdog's first tick) replaces it.
        last_broadcast: Mutex::new(idle_event()),
    });

    TrayIconBuilder::with_id("main")
        // The idle SpiritMark, not the window icon: the tray is the capture
        // indicator, so it starts on the same mark `broadcast_event` keeps
        // truthful from here on.
        .icon(tray_icon_image(TrayIconKind::Idle))
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
        use TogglePress::Direct;
        // Stopping is always allowed, regardless of the consent flag.
        assert_eq!(next_action(true, true, Direct), ToggleAction::Stop);
        assert_eq!(next_action(true, false, Direct), ToggleAction::Stop);
        // Starting requires acknowledged consent; without it, surface the nudge.
        assert_eq!(next_action(false, true, Direct), ToggleAction::Start);
        assert_eq!(
            next_action(false, false, Direct),
            ToggleAction::RequireConsent
        );
    }

    #[test]
    fn a_coalesced_press_can_stop_but_never_start() {
        use TogglePress::Coalesced;
        // A press that means "stop" is honored however it arrived — dropping
        // it would leave a meeting recording after the user asked it not to.
        assert_eq!(next_action(true, true, Coalesced), ToggleAction::Stop);

        // But a replayed press must never begin a recording. This is the
        // impatient double-press during a stop: by the time the press is
        // replayed the engine is idle, and a plain toggle would start a brand
        // new capture the user never asked for.
        assert_eq!(next_action(false, true, Coalesced), ToggleAction::Nothing);
        // Not even the consent nudge — the user is not looking at a press
        // they made against this state.
        assert_eq!(next_action(false, false, Coalesced), ToggleAction::Nothing);
    }

    #[test]
    fn drain_replays_only_the_first_press_as_direct() {
        // The press that took the lock acts on state the user could see; every
        // replayed press after it is coalesced, so it can only ever stop.
        let pending = AtomicBool::new(false);
        let mut presses = Vec::new();
        drain_pending_toggles(&pending, TogglePress::Direct, |press| {
            presses.push(press);
            if presses.len() == 1 {
                pending.store(true, Ordering::SeqCst);
            }
        });
        assert_eq!(presses, vec![TogglePress::Direct, TogglePress::Coalesced]);
    }

    #[test]
    fn watchdog_drain_treats_every_press_as_coalesced() {
        // The watchdog only ever drains presses stranded by someone else, so
        // none of them may start a capture.
        let pending = AtomicBool::new(false);
        let mut presses = Vec::new();
        drain_pending_toggles(&pending, TogglePress::Coalesced, |press| {
            presses.push(press);
            if presses.len() == 1 {
                pending.store(true, Ordering::SeqCst);
            }
        });
        assert_eq!(
            presses,
            vec![TogglePress::Coalesced, TogglePress::Coalesced]
        );
    }

    #[test]
    fn start_is_announced_only_for_a_start_that_isnt_already_running() {
        assert!(should_announce_start(true, false));
        // Re-announcing over a live capture would flash `Starting` over
        // `Listening`.
        assert!(!should_announce_start(true, true));
        // A stop never announces a start.
        assert!(!should_announce_start(false, false));
        assert!(!should_announce_start(false, true));
    }

    #[test]
    fn a_failed_health_read_still_announces_the_start() {
        // Going silent for the ~1s negotiation window is the exact failure
        // `Starting` exists to remove, so the fallback announces the start
        // while claiming no source is live.
        let event = starting_event_with_no_source_live();
        assert_eq!(event.phase, CapturePhase::Starting);
        assert_eq!(event.sources.loopback, SourceState::Off);
        assert_eq!(event.sources.microphone, SourceState::Off);
        assert_eq!(
            tray_copy(&event),
            ("Stop capture", "Kodabi: starting capture")
        );
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
    fn tray_icon_kind_per_phase() {
        // Mirrors `markMode` in src/captureLabel.ts: the in-window mark and
        // the tray mark must not tell different stories about the same state.
        assert_eq!(tray_icon_kind(&idle_event()), TrayIconKind::Idle);
        assert_eq!(
            tray_icon_kind(&event(
                CapturePhase::Starting,
                SourceState::Off,
                SourceState::Off
            )),
            TrayIconKind::Engaged
        );
        assert_eq!(
            tray_icon_kind(&event(
                CapturePhase::Listening,
                SourceState::Live,
                SourceState::Live
            )),
            TrayIconKind::Recording
        );

        // Degraded with one source still live IS on air — the green stays,
        // because withdrawing it would falsely suggest privacy.
        for sources in [
            (SourceState::Live, SourceState::Failed),
            (SourceState::Stalled, SourceState::Live),
        ] {
            assert_eq!(
                tray_icon_kind(&event(CapturePhase::Degraded, sources.0, sources.1)),
                TrayIconKind::Recording
            );
        }

        // Degraded with nothing live records nothing, so it wears no green.
        // But it isn't the idle mark either: the session is still engaged and
        // will resume with no further press.
        for sources in [
            (SourceState::Stalled, SourceState::Stalled),
            (SourceState::Failed, SourceState::Failed),
            (SourceState::Stalled, SourceState::Failed),
        ] {
            assert_eq!(
                tray_icon_kind(&event(CapturePhase::Degraded, sources.0, sources.1)),
                TrayIconKind::Engaged
            );
        }
    }

    #[test]
    fn tray_icon_kind_never_disagrees_with_tray_copy() {
        // The icon, the menu label and the tooltip come from the same event,
        // so no combination of them may contradict another: a green mark over
        // an "idle" tooltip would be the exact failure the mark exists to
        // prevent.
        //
        // Swept over every event the two broadcasters can actually produce —
        // the derived truth and its `Starting` overlay for every per-source
        // health pair — rather than the phase x sources cross product, most of
        // which `phase_of` can never derive.
        let healths = [
            kodabi_audio::SourceHealth::Off,
            kodabi_audio::SourceHealth::Live,
            kodabi_audio::SourceHealth::Stalled,
            kodabi_audio::SourceHealth::Failed("device gone".into()),
        ];
        for loopback in &healths {
            for microphone in &healths {
                let health = kodabi_audio::DualHealth {
                    loopback: loopback.clone(),
                    microphone: microphone.clone(),
                };
                for event in [
                    crate::audio_cmds::event_from_health(&health),
                    starting_event(&health),
                ] {
                    let (label, tooltip) = tray_copy(&event);
                    let kind = tray_icon_kind(&event);

                    // The idle mark shows exactly when the menu offers a
                    // start, and never over any other tooltip.
                    assert_eq!(
                        kind == TrayIconKind::Idle,
                        label == "Start capture",
                        "{event:?} disagrees on idle"
                    );
                    match kind {
                        TrayIconKind::Idle => assert_eq!(tooltip, "Kodabi: idle", "{event:?}"),
                        // The green only ever sits over copy that names
                        // audio being captured.
                        TrayIconKind::Recording => assert!(
                            [
                                "Kodabi: listening",
                                "Kodabi: mic unavailable, capturing system audio",
                                "Kodabi: system audio unavailable, capturing mic",
                            ]
                            .contains(&tooltip),
                            "{event:?} shows the green over {tooltip:?}"
                        ),
                        // And the ink ring only ever over copy that claims
                        // nothing is being captured yet.
                        TrayIconKind::Engaged => assert!(
                            [
                                "Kodabi: starting capture",
                                "Kodabi: reconnecting audio devices",
                            ]
                            .contains(&tooltip),
                            "{event:?} shows the engaged mark over {tooltip:?}"
                        ),
                    }
                }
            }
        }
    }

    #[test]
    fn tray_icon_assets_are_32px_rgba() {
        // A regenerated asset at the wrong size would ship a blurry or
        // cropped mark; the wrong colour type fails the compile instead.
        for kind in [
            TrayIconKind::Idle,
            TrayIconKind::Engaged,
            TrayIconKind::Recording,
        ] {
            let image = tray_icon_image(kind);
            assert_eq!(image.width(), 32, "{kind:?}");
            assert_eq!(image.height(), 32, "{kind:?}");
            assert_eq!(image.rgba().len(), 32 * 32 * 4, "{kind:?}");
        }
    }

    #[test]
    fn tray_icon_assets_are_distinct() {
        // Three states that render the same pixels would satisfy every other
        // test here and still leave the tray ambiguous.
        let idle = tray_icon_image(TrayIconKind::Idle);
        let engaged = tray_icon_image(TrayIconKind::Engaged);
        let recording = tray_icon_image(TrayIconKind::Recording);
        assert_ne!(idle.rgba(), engaged.rgba());
        assert_ne!(idle.rgba(), recording.rgba());
        assert_ne!(engaged.rgba(), recording.rgba());
    }

    #[test]
    fn icon_updates_only_when_the_mark_changes() {
        let idle = idle_event();
        let starting = event(CapturePhase::Starting, SourceState::Off, SourceState::Off);
        let listening = event(
            CapturePhase::Listening,
            SourceState::Live,
            SourceState::Live,
        );
        let one_live = event(
            CapturePhase::Degraded,
            SourceState::Live,
            SourceState::Failed,
        );
        let none_live = event(
            CapturePhase::Degraded,
            SourceState::Stalled,
            SourceState::Stalled,
        );

        // Every real transition repaints.
        assert!(icon_needs_update(&idle, &starting));
        assert!(icon_needs_update(&starting, &listening));
        assert!(icon_needs_update(&listening, &none_live));
        assert!(icon_needs_update(&none_live, &idle));

        // Re-broadcasts that don't change the mark leave the icon alone: a
        // no-op toggle, and the watchdog noticing one source drop out of a
        // capture that is still recording the other.
        assert!(!icon_needs_update(&idle, &idle));
        assert!(!icon_needs_update(&listening, &one_live));
    }

    #[test]
    fn drain_pending_toggles_runs_once_when_nothing_pending() {
        let pending = AtomicBool::new(false);
        let mut runs = 0;
        drain_pending_toggles(&pending, TogglePress::Direct, |_| runs += 1);
        assert_eq!(runs, 1);
        assert!(!pending.load(Ordering::SeqCst));
    }

    #[test]
    fn drain_pending_toggles_coalesces_presses_into_one_extra_toggle() {
        let pending = AtomicBool::new(false);
        let mut runs = 0;
        drain_pending_toggles(&pending, TogglePress::Direct, |_| {
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
