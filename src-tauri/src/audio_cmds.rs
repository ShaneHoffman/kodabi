//! Thin Tauri command wrappers over `kodabi_audio::DualCapture`. All capture
//! and two-channel orchestration logic lives in the `kodabi-audio` crate;
//! these commands only own the managed [`DualCapture`] and map its status /
//! aligned-session outputs to serializable IPC DTOs.
//!
//! `run_mic_test` is the one command here that isn't capture-status
//! plumbing: it wraps `kodabi_audio::run_mic_test`'s device I/O and persists
//! the result through the managed `SettingsState` (`settings_cmds.rs`),
//! since a mic-test result is exactly that — a stored setting.

use std::sync::Mutex;

use chrono::Utc;
use kodabi_audio::{
    AudioFormat, CaptureTuning, CombinedSession, DualCapture, DualHealth, DualStatus, SourceHealth,
    SourceStatus, SpillConfig,
};
use kodabi_core::device::DeviceId;
use kodabi_core::inflight::{self, InflightSession};
use kodabi_core::settings::{MicCheckOutcome, MicCheckResult, Settings as CoreSettings};
use tauri::{Emitter, Manager};

/// Default bound on each source's capture-item channel — the slack between a
/// cpal callback enqueuing a frame and the combiner's coordinator thread
/// draining it. Each source is attached to the combiner the instant it goes
/// live (`DualCapture::start_and_attach`), so a source is drained
/// continuously and never has to buffer a whole slow-negotiation window — the
/// channel only has to absorb ordinary scheduling jitter, for which a few
/// frames is ample. 256 leaves generous headroom while keeping the buffered
/// PCM bounded; a full channel drops the frame (`try_send` fails) and bumps
/// `frames_dropped`. Overridable via `KODABI_FRAME_CAPACITY`
/// (`CaptureTuning::from_env`) for a resource-budget tuning pass
/// (`docs/RESOURCE_BUDGET.md`).
const FRAME_CAPACITY: usize = 256;

/// Common rate the two-channel combiner aligns mic and system audio to.
/// 48 kHz preserves fidelity (Windows loopback is almost always already
/// 48 kHz, so resampling it is near-identity); a transcription engine's
/// 16 kHz mono need is a downstream downsample, not this layer's concern.
const TWO_CHANNEL_SAMPLE_RATE: u32 = 48_000;

/// The managed capture state: the two-channel capture engine plus the
/// currently-open in-flight session (the on-disk spill it streams to). The
/// in-flight session is set at `start` and taken at `stop`; `None` between
/// captures, or when spill setup failed and capture ran in memory.
pub struct CaptureState(DualCapture, Mutex<Option<InflightSession>>);

impl Default for CaptureState {
    fn default() -> Self {
        CaptureState(
            DualCapture::new(CaptureTuning::from_env(
                FRAME_CAPACITY,
                TWO_CHANNEL_SAMPLE_RATE,
            )),
            Mutex::new(None),
        )
    }
}

impl CaptureState {
    /// True while either capture source is live. Delegates to
    /// `DualCapture::is_active` — the true backend state a toggle must act
    /// on, not a UI guess.
    pub(crate) fn is_active(&self) -> Result<bool, String> {
        self.0.is_active().map_err(|e| e.to_string())
    }

    /// Per-source health: what is *actually* recording right now. Delegates to
    /// `DualCapture::health`, the liveness truth an honest indicator needs
    /// (unlike `is_active`, which stays true through a device rebuild).
    pub(crate) fn health(&self) -> Result<DualHealth, String> {
        self.0.health().map_err(|e| e.to_string())
    }

    /// Store the in-flight session for the current capture, returning any
    /// previously-stashed one (there should be none) so its lock is released.
    fn set_inflight(&self, session: Option<InflightSession>) -> Option<InflightSession> {
        std::mem::replace(
            &mut self.1.lock().unwrap_or_else(|p| p.into_inner()),
            session,
        )
    }

    /// Take the in-flight session out (at stop).
    fn take_inflight(&self) -> Option<InflightSession> {
        self.1.lock().unwrap_or_else(|p| p.into_inner()).take()
    }
}

/// Unambiguous capture state broadcast to the frontend on every start/stop —
/// also the consent signal (a meeting is only ever recorded while `Listening`
/// or a `Degraded` state with a live source).
///
/// Richer than a start/stop boolean so an indicator can never claim capture is
/// running when it isn't: the toggle path overlays `Starting` for the WASAPI
/// negotiation window, and `Degraded` covers a source that failed to start or
/// died mid-session (see [`phase_of`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CapturePhase {
    Idle,
    /// A start is in flight (device negotiation can take ~1s). Never derived
    /// from backend state — only overlaid by the path performing the start.
    Starting,
    /// Both sources are live.
    Listening,
    /// Capture is engaged but not fully healthy: a source failed to start, or
    /// its device dropped out and is being rebuilt. `sources` says which.
    Degraded,
}

/// Whether one capture source is recording, and if not, why. Mirrors
/// `kodabi_audio::SourceHealth`, minus the failure message (that stays
/// available via [`capture_status`]) — the indicator's copy is derived from
/// *which* source is down, not from the raw device error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceState {
    Off,
    Live,
    Stalled,
    Failed,
}

impl From<&SourceHealth> for SourceState {
    fn from(health: &SourceHealth) -> Self {
        match health {
            SourceHealth::Off => SourceState::Off,
            SourceHealth::Failed(_) => SourceState::Failed,
            SourceHealth::Live => SourceState::Live,
            SourceHealth::Stalled => SourceState::Stalled,
        }
    }
}

/// Per-source breakdown carried by [`CaptureStateEvent`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct CaptureSources {
    pub loopback: SourceState,
    pub microphone: SourceState,
}

/// Event payload for [`crate::capture_control::CAPTURE_STATE_EVENT`], and the
/// response of [`capture_phase`]. An object rather than a bare string so a
/// future field could be added without breaking the contract
/// `feat/listening-indicator` depends on — which is exactly what `sources` is:
/// the per-source breakdown that lets the UI say *which* source is down
/// instead of a bare "degraded".
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct CaptureStateEvent {
    pub phase: CapturePhase,
    pub sources: CaptureSources,
}

/// The phase implied by both sources' health.
///
/// `Listening` requires *both* sources live, so the plain listening indicator
/// only ever shows when everything the session expects is actually recording.
/// A start where both sources failed derives `Idle` (nothing is installed, and
/// the toggle contract is that such a start reports idle), with the failures
/// still visible in `sources`.
pub(crate) fn phase_of(health: &DualHealth) -> CapturePhase {
    let engaged =
        |source: &SourceHealth| matches!(source, SourceHealth::Live | SourceHealth::Stalled);
    match (&health.loopback, &health.microphone) {
        (SourceHealth::Live, SourceHealth::Live) => CapturePhase::Listening,
        (loopback, microphone) if engaged(loopback) || engaged(microphone) => {
            CapturePhase::Degraded
        }
        _ => CapturePhase::Idle,
    }
}

fn sources_of(health: &DualHealth) -> CaptureSources {
    CaptureSources {
        loopback: (&health.loopback).into(),
        microphone: (&health.microphone).into(),
    }
}

/// The event describing the true current state.
pub(crate) fn event_from_health(health: &DualHealth) -> CaptureStateEvent {
    CaptureStateEvent {
        phase: phase_of(health),
        sources: sources_of(health),
    }
}

/// The same per-source truth, but announcing an in-flight start. Used only by
/// the path holding the toggle lock, which knows a start it just began has not
/// finished negotiating yet.
pub(crate) fn starting_event(health: &DualHealth) -> CaptureStateEvent {
    CaptureStateEvent {
        phase: CapturePhase::Starting,
        sources: sources_of(health),
    }
}

/// The event describing the true state of the managed capture engine.
pub(crate) fn event_from_state(state: &CaptureState) -> Result<CaptureStateEvent, String> {
    Ok(event_from_health(&state.health()?))
}

/// Serializable per-source status for IPC. Mirrors `kodabi_audio::SourceStatus`
/// — this crate owns the wire shape (serde) so the audio crate stays free of a
/// serialization concern.
#[derive(serde::Serialize)]
pub struct StreamStatus {
    running: bool,
    format: Option<AudioFormat>,
    frames_captured: u64,
    frames_dropped: u64,
    segments: u32,
    peak: f32,
    rms: f32,
    /// Set when this stream failed to start (no device, permission denied,
    /// unsupported format). Persisted in the source's slot, so both the
    /// `start_capture` response and later `capture_status` polls report it
    /// until the next successful start or an explicit stop clears it.
    error: Option<String>,
}

impl From<SourceStatus> for StreamStatus {
    fn from(status: SourceStatus) -> Self {
        StreamStatus {
            running: status.running,
            format: status.format,
            frames_captured: status.frames_captured,
            frames_dropped: status.frames_dropped,
            segments: status.segments,
            peak: status.peak,
            rms: status.rms,
            error: status.error,
        }
    }
}

/// A verification summary of a finalized two-channel session: enough to
/// confirm a stopped session produced one time-aligned artifact with both
/// channels present, without exposing the raw PCM over IPC. Works whether the
/// session streamed to disk or stayed in memory.
#[derive(serde::Serialize)]
pub struct AlignedSessionStats {
    sample_rate: u32,
    frames: usize,
    duration_ms: u64,
}

impl From<&CombinedSession> for AlignedSessionStats {
    fn from(session: &CombinedSession) -> Self {
        match session {
            CombinedSession::InMemory(session) => AlignedSessionStats {
                sample_rate: session.sample_rate(),
                frames: session.frames(),
                duration_ms: session.duration().as_millis() as u64,
            },
            CombinedSession::Spilled(spilled) => AlignedSessionStats {
                sample_rate: spilled.sample_rate,
                frames: spilled.frames as usize,
                duration_ms: spilled.duration().as_millis() as u64,
            },
        }
    }
}

#[derive(serde::Serialize)]
pub struct CaptureStatus {
    loopback: StreamStatus,
    microphone: StreamStatus,
    /// The finalized two-channel session, present only in `stop_capture`'s
    /// response once both sources were live long enough for the combiner to
    /// run — `None` while capture is starting/running, and `None` if only one
    /// source ever came up (there's no two-channel session to combine).
    aligned_session: Option<AlignedSessionStats>,
}

impl CaptureStatus {
    fn from_parts(status: DualStatus, aligned_session: Option<AlignedSessionStats>) -> Self {
        CaptureStatus {
            loopback: status.loopback.into(),
            microphone: status.microphone.into(),
            aligned_session,
        }
    }
}

/// `pub(crate)` so `capture_control`'s shared toggle can drive start/stop
/// through the same path `start_capture`/`stop_capture` use over IPC.
///
/// Creates an in-flight spill directory so capture streams to disk (bounded
/// memory + crash recovery). A failure to set that up never fails the capture:
/// it logs and falls back to capturing in memory, exactly as before this
/// feature. The `AppHandle` resolves the sessions directory and device identity
/// for the spill.
pub(crate) fn start_capture_impl(
    app: &tauri::AppHandle,
    state: &CaptureState,
) -> Result<CaptureStatus, String> {
    // Only set up a fresh spill for a genuinely new capture. An idempotent
    // re-start of an already-live session (e.g. a mid-session device rebuild)
    // must keep the existing spill, not mint a second directory.
    let spill = if state.is_active()? {
        None
    } else {
        create_inflight_spill(app, state)
    };
    let status = state.0.start(spill).map_err(|e| e.to_string())?;
    Ok(CaptureStatus::from_parts(status, None))
}

/// Create the in-flight session for a new capture, stash it on `state`, and
/// return the [`SpillConfig`] the combiner streams through. Returns `None`
/// (capture proceeds in memory) if the directory can't be created — capture
/// must never fail because durability couldn't be arranged.
fn create_inflight_spill(app: &tauri::AppHandle, state: &CaptureState) -> Option<SpillConfig> {
    let sessions_dir = match crate::transcribe::knowledge_base_dir(app) {
        Ok(kb) => kb.join("sessions"),
        Err(err) => {
            eprintln!("capture spill setup skipped: {err}");
            return None;
        }
    };
    let device = app.state::<DeviceId>().inner().clone();
    let session =
        match inflight::create_session(&sessions_dir, &device, Utc::now(), TWO_CHANNEL_SAMPLE_RATE)
        {
            Ok(session) => session,
            Err(err) => {
                eprintln!("capture spill setup failed, capturing in memory: {err}");
                return None;
            }
        };
    let spill = SpillConfig {
        mic_path: session.mic_path(),
        system_path: session.system_path(),
        flush_threshold_samples: state.0.flush_threshold_samples(),
    };
    // Any previously-stashed session (there should be none) is dropped, freeing
    // its lock.
    let _ = state.set_inflight(Some(session));
    Some(spill)
}

/// Stops capture, and — when both sources ran long enough to produce a
/// finalized two-channel session — spawns the transcribe → clean → persist
/// pipeline on it (`crate::transcribe`), which removes the in-flight directory
/// once the transcript lands. A lone-source / no-combiner stop removes the
/// in-flight directory directly (there is nothing to transcribe).
///
/// Takes the `AppHandle` the pipeline needs to resolve the app-data
/// directory, read the managed device identity, and emit
/// `transcription:state` events; the stopping itself is factored into
/// [`stop_and_finalize`], which stays app-free and unit-testable without a
/// running Tauri app.
pub(crate) fn stop_capture_and_transcribe(
    app: &tauri::AppHandle,
    state: &CaptureState,
) -> Result<CaptureStatus, String> {
    let (status, session) = stop_and_finalize(state)?;
    let inflight = state.take_inflight();
    let aligned_session = session.as_ref().map(AlignedSessionStats::from);
    match session {
        Some(session) => crate::transcribe::spawn_transcription(app, session, inflight),
        None => {
            // No two-channel artifact (a lone source, or a combiner that never
            // spawned). Nothing to transcribe — drop the spill directory.
            if let Some(inflight) = inflight {
                if let Err(err) = inflight.remove() {
                    eprintln!("capture: failed to remove in-flight session: {err}");
                }
            }
        }
    }
    Ok(CaptureStatus::from_parts(status, aligned_session))
}

fn stop_and_finalize(
    state: &CaptureState,
) -> Result<(DualStatus, Option<CombinedSession>), String> {
    state.0.stop().map_err(|e| e.to_string())
}

fn capture_status_impl(state: &CaptureState) -> Result<CaptureStatus, String> {
    // The aligned session only exists once `stop_capture` finalizes it; a
    // status poll while running has nothing to report yet.
    let status = state.0.status().map_err(|e| e.to_string())?;
    Ok(CaptureStatus::from_parts(status, None))
}

#[tauri::command]
pub fn start_capture(
    app: tauri::AppHandle,
    state: tauri::State<'_, CaptureState>,
) -> Result<CaptureStatus, String> {
    // Same consent gate as the hotkey/tray toggle: recording never starts
    // before the nudge is acknowledged. The consent nudge persists consent
    // *before* calling this, so its own start passes; a stale frontend calling
    // start directly gets bounced back to the nudge. (Defense in depth — the
    // toggle path already gates.)
    if !crate::capture_control::consent_acknowledged(&app) {
        use tauri::Emitter;
        let _ = app.emit(
            crate::capture_control::CONSENT_REQUIRED_EVENT,
            crate::capture_control::ConsentRequiredEvent {},
        );
        return Err("consent required before first capture".to_string());
    }
    // An IPC start is still a start the user asked for (the consent nudge and
    // the in-window control both land here), so the pill reads the manual
    // setting. Recorded before the toggle path broadcasts.
    crate::overlay::note_capture_start(&app, kodabi_core::overlay::CaptureOrigin::Manual);
    // Route through the shared toggle path so this serializes with the
    // hotkey/tray toggle and broadcasts the resulting phase (relabelling the
    // tray + emitting `capture:state`) — otherwise the UI would go stale
    // whenever capture is driven over IPC instead of the toggle.
    crate::capture_control::run_under_toggle_lock(&app, state.inner(), true, |app, state| {
        start_capture_impl(app, state)
    })
}

#[tauri::command]
pub fn stop_capture(
    app: tauri::AppHandle,
    state: tauri::State<'_, CaptureState>,
) -> Result<CaptureStatus, String> {
    crate::capture_control::run_under_toggle_lock(
        &app,
        state.inner(),
        false,
        stop_capture_and_transcribe,
    )
}

#[tauri::command]
pub fn capture_status(state: tauri::State<'_, CaptureState>) -> Result<CaptureStatus, String> {
    capture_status_impl(&state)
}

/// The unambiguous capture phase. The frontend calls this once on mount to
/// seed its state before subscribing to
/// [`crate::capture_control::CAPTURE_STATE_EVENT`], so it can't miss a
/// transition that happened before the listener attached.
///
/// Returns the last broadcast event rather than re-deriving, so a mount during
/// an in-flight start correctly seeds `Starting` (which is never derivable
/// from backend state). The broadcaster keeps that value converged with truth,
/// so this can't go stale. Before the controller exists (very early startup)
/// there is nothing broadcast yet, so derive.
#[tauri::command]
pub fn capture_phase(
    app: tauri::AppHandle,
    state: tauri::State<'_, CaptureState>,
) -> Result<CaptureStateEvent, String> {
    let last_broadcast = app
        .try_state::<crate::capture_control::CaptureController>()
        .map(|controller| controller.last_broadcast());
    seed_event(last_broadcast, &state)
}

/// The seed a mounting frontend gets, split from [`capture_phase`] so the
/// choice is testable without a running Tauri app.
///
/// Prefers what was last broadcast (only that carries `Starting`, which is
/// never derivable from backend state); falls back to deriving from the engine
/// when nothing has been broadcast yet.
pub(crate) fn seed_event(
    last_broadcast: Option<CaptureStateEvent>,
    state: &CaptureState,
) -> Result<CaptureStateEvent, String> {
    match last_broadcast {
        Some(event) => Ok(event),
        None => event_from_state(state),
    }
}

/// Maps `kodabi_audio::MicTestOutcome` onto the persisted
/// `kodabi_core::settings::MicCheckOutcome` — can't be a `From` impl since
/// neither type is local to this crate (the orphan rule), so this is the one
/// place both are in scope. `kodabi-core` stays free of `kodabi-audio`, a
/// platform-IO crate.
fn to_settings_outcome(outcome: kodabi_audio::MicTestOutcome) -> MicCheckOutcome {
    match outcome {
        kodabi_audio::MicTestOutcome::Headphones => MicCheckOutcome::Headphones,
        kodabi_audio::MicTestOutcome::Speakers { echo_db, delay_ms } => {
            MicCheckOutcome::Speakers { echo_db, delay_ms }
        }
        kodabi_audio::MicTestOutcome::MicSilent => MicCheckOutcome::MicSilent,
    }
}

/// Runs the Settings mic test: plays a short tone through the default output
/// device while recording the default microphone, classifies whether (and
/// how strongly) the mic heard it, and persists the result. Refuses while a
/// capture is in progress — a second stream on the mic device would fight
/// the live one for it, and the tone would land in the meeting's own
/// recording.
#[tauri::command]
pub fn run_mic_test(
    app: tauri::AppHandle,
    capture: tauri::State<'_, CaptureState>,
    settings: tauri::State<'_, crate::settings_cmds::SettingsState>,
) -> Result<CoreSettings, String> {
    if capture.is_active()? {
        return Err("stop the current capture before running the mic test".to_string());
    }
    let outcome = kodabi_audio::run_mic_test().map_err(|err| err.to_string())?;
    let result = MicCheckResult {
        outcome: to_settings_outcome(outcome),
        measured_at: Utc::now(),
    };
    let updated = settings.update(|s| s.mic_check = Some(result))?;
    let _ = app.emit(crate::settings_cmds::SETTINGS_CHANGED_EVENT, updated);
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mic_test_outcome_maps_onto_the_stored_shape() {
        assert_eq!(
            to_settings_outcome(kodabi_audio::MicTestOutcome::Headphones),
            MicCheckOutcome::Headphones
        );
        assert_eq!(
            to_settings_outcome(kodabi_audio::MicTestOutcome::MicSilent),
            MicCheckOutcome::MicSilent
        );
        assert_eq!(
            to_settings_outcome(kodabi_audio::MicTestOutcome::Speakers {
                echo_db: 12.5,
                delay_ms: 85.0,
            }),
            MicCheckOutcome::Speakers {
                echo_db: 12.5,
                delay_ms: 85.0,
            }
        );
    }

    #[test]
    fn capture_status_is_idle_before_any_start() {
        let state = CaptureState::default();
        let status = capture_status_impl(&state).unwrap();
        assert!(!status.loopback.running);
        assert!(status.loopback.format.is_none());
        assert!(!status.microphone.running);
        assert!(status.microphone.format.is_none());
    }

    #[test]
    fn stop_capture_while_idle_is_a_no_op() {
        let state = CaptureState::default();
        let (status, session) = stop_and_finalize(&state).unwrap();
        assert!(!status.loopback.running);
        assert!(!status.microphone.running);
        assert!(session.is_none());
    }

    #[test]
    fn capture_phase_is_idle_before_any_start() {
        let state = CaptureState::default();
        let event = event_from_state(&state).unwrap();
        assert!(matches!(event.phase, CapturePhase::Idle));
        assert!(matches!(event.sources.loopback, SourceState::Off));
        assert!(matches!(event.sources.microphone, SourceState::Off));
    }

    #[test]
    fn a_mounting_frontend_is_seeded_from_the_last_broadcast() {
        let state = CaptureState::default();
        // `Starting` is never derivable from backend state, so a mount during
        // an in-flight start would seed `Idle` if this re-derived instead of
        // returning what was last shown.
        let starting = starting_event(&health(SourceHealth::Off, SourceHealth::Off));
        assert_eq!(seed_event(Some(starting), &state).unwrap(), starting);

        // Before anything has been broadcast (very early startup, no
        // controller yet) there is nothing to return, so derive the truth.
        let derived = seed_event(None, &state).unwrap();
        assert_eq!(derived.phase, CapturePhase::Idle);
        assert_eq!(derived.sources.loopback, SourceState::Off);
        assert_eq!(derived.sources.microphone, SourceState::Off);
    }

    fn health(loopback: SourceHealth, microphone: SourceHealth) -> DualHealth {
        DualHealth {
            loopback,
            microphone,
        }
    }

    #[test]
    fn phase_of_derivation() {
        // Nothing installed: idle, whether or not a start ever failed.
        assert_eq!(
            phase_of(&health(SourceHealth::Off, SourceHealth::Off)),
            CapturePhase::Idle
        );
        assert_eq!(
            phase_of(&health(
                SourceHealth::Failed("no device".into()),
                SourceHealth::Failed("no device".into())
            )),
            CapturePhase::Idle
        );

        // Plain listening requires BOTH sources actually recording.
        assert_eq!(
            phase_of(&health(SourceHealth::Live, SourceHealth::Live)),
            CapturePhase::Listening
        );

        // Anything installed but not fully live is degraded — never listening,
        // which is the lie this task exists to kill.
        assert_eq!(
            phase_of(&health(SourceHealth::Live, SourceHealth::Stalled)),
            CapturePhase::Degraded
        );
        assert_eq!(
            phase_of(&health(SourceHealth::Stalled, SourceHealth::Stalled)),
            CapturePhase::Degraded
        );
        assert_eq!(
            phase_of(&health(
                SourceHealth::Live,
                SourceHealth::Failed("no default microphone".into())
            )),
            CapturePhase::Degraded
        );
        assert_eq!(
            phase_of(&health(SourceHealth::Live, SourceHealth::Off)),
            CapturePhase::Degraded
        );
        assert_eq!(
            phase_of(&health(
                SourceHealth::Stalled,
                SourceHealth::Failed("no default microphone".into())
            )),
            CapturePhase::Degraded
        );
    }

    #[test]
    fn capture_state_event_wire_contract() {
        // Locks the JSON shape `feat/listening-indicator` depends on: an
        // object with a lowercase `phase` string, not a bare string, plus the
        // per-source breakdown the indicator derives its copy from.
        let idle = serde_json::to_string(&event_from_health(&health(
            SourceHealth::Off,
            SourceHealth::Off,
        )))
        .unwrap();
        assert_eq!(
            idle,
            r#"{"phase":"idle","sources":{"loopback":"off","microphone":"off"}}"#
        );

        let listening = serde_json::to_string(&event_from_health(&health(
            SourceHealth::Live,
            SourceHealth::Live,
        )))
        .unwrap();
        assert_eq!(
            listening,
            r#"{"phase":"listening","sources":{"loopback":"live","microphone":"live"}}"#
        );

        // A start where the mic failed but system audio came up.
        let degraded = serde_json::to_string(&event_from_health(&health(
            SourceHealth::Live,
            SourceHealth::Failed("no default microphone".into()),
        )))
        .unwrap();
        assert_eq!(
            degraded,
            r#"{"phase":"degraded","sources":{"loopback":"live","microphone":"failed"}}"#
        );

        // A device that dropped out mid-session and is being rebuilt.
        let stalled = serde_json::to_string(&event_from_health(&health(
            SourceHealth::Stalled,
            SourceHealth::Live,
        )))
        .unwrap();
        assert_eq!(
            stalled,
            r#"{"phase":"degraded","sources":{"loopback":"stalled","microphone":"live"}}"#
        );

        // `Starting` is overlaid, not derived: same sources, announcing phase.
        let starting = serde_json::to_string(&starting_event(&health(
            SourceHealth::Off,
            SourceHealth::Off,
        )))
        .unwrap();
        assert_eq!(
            starting,
            r#"{"phase":"starting","sources":{"loopback":"off","microphone":"off"}}"#
        );
    }
}
