//! Thin Tauri command wrappers over `kodabi_audio::DualCapture`. All capture
//! and two-channel orchestration logic lives in the `kodabi-audio` crate;
//! these commands only own the managed [`DualCapture`] and map its status /
//! aligned-session outputs to serializable IPC DTOs.

use std::sync::Mutex;

use chrono::Utc;
use kodabi_audio::{
    AudioFormat, CaptureTuning, CombinedSession, DualCapture, DualStatus, SourceStatus, SpillConfig,
};
use kodabi_core::device::DeviceId;
use kodabi_core::inflight::{self, InflightSession};
use tauri::Manager;

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
/// also the consent signal (a meeting is only ever recorded while
/// `Listening`).
#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CapturePhase {
    Idle,
    Listening,
}

/// Event payload for [`crate::capture_control::CAPTURE_STATE_EVENT`], and the
/// response of [`capture_phase`]. An object rather than a bare string so a
/// future field (per-source breakdown, a since-timestamp) can be added
/// without breaking the contract `feat/listening-indicator` depends on.
#[derive(Clone, serde::Serialize)]
pub struct CaptureStateEvent {
    pub phase: CapturePhase,
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

fn capture_phase_impl(state: &CaptureState) -> Result<CaptureStateEvent, String> {
    let phase = if state.is_active()? {
        CapturePhase::Listening
    } else {
        CapturePhase::Idle
    };
    Ok(CaptureStateEvent { phase })
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
    // Route through the shared toggle path so this serializes with the
    // hotkey/tray toggle and broadcasts the resulting phase (relabelling the
    // tray + emitting `capture:state`) — otherwise the UI would go stale
    // whenever capture is driven over IPC instead of the toggle.
    crate::capture_control::run_under_toggle_lock(&app, state.inner(), |app, state| {
        start_capture_impl(app, state)
    })
}

#[tauri::command]
pub fn stop_capture(
    app: tauri::AppHandle,
    state: tauri::State<'_, CaptureState>,
) -> Result<CaptureStatus, String> {
    crate::capture_control::run_under_toggle_lock(&app, state.inner(), stop_capture_and_transcribe)
}

#[tauri::command]
pub fn capture_status(state: tauri::State<'_, CaptureState>) -> Result<CaptureStatus, String> {
    capture_status_impl(&state)
}

/// The unambiguous idle/listening phase, derived from the true backend
/// state. The frontend calls this once on mount to seed its state before
/// subscribing to [`crate::capture_control::CAPTURE_STATE_EVENT`], so it
/// can't miss a transition that happened before the listener attached.
#[tauri::command]
pub fn capture_phase(state: tauri::State<'_, CaptureState>) -> Result<CaptureStateEvent, String> {
    capture_phase_impl(&state)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let event = capture_phase_impl(&state).unwrap();
        assert!(matches!(event.phase, CapturePhase::Idle));
    }

    #[test]
    fn capture_state_event_wire_contract() {
        // Locks the JSON shape `feat/listening-indicator` depends on: an
        // object with a lowercase `phase` string, not a bare string.
        let idle = serde_json::to_string(&CaptureStateEvent {
            phase: CapturePhase::Idle,
        })
        .unwrap();
        assert_eq!(idle, r#"{"phase":"idle"}"#);

        let listening = serde_json::to_string(&CaptureStateEvent {
            phase: CapturePhase::Listening,
        })
        .unwrap();
        assert_eq!(listening, r#"{"phase":"listening"}"#);
    }
}
