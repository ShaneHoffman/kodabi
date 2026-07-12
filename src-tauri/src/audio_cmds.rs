//! Thin Tauri command wrappers over `kodama_audio::Capture`. All capture
//! logic lives in the `kodama-audio` crate; these commands only manage two
//! independent sessions (loopback + microphone) and map them to a
//! serializable status.

use std::sync::Mutex;

use kodama_audio::{AudioFormat, Capture, CaptureSource};

const FRAME_CAPACITY: usize = 256;

#[derive(Default)]
struct Sessions {
    loopback: Option<Capture>,
    microphone: Option<Capture>,
}

#[derive(Default)]
pub struct CaptureState(Mutex<Sessions>);

fn slot_mut(sessions: &mut Sessions, source: CaptureSource) -> &mut Option<Capture> {
    match source {
        CaptureSource::Loopback => &mut sessions.loopback,
        CaptureSource::Microphone => &mut sessions.microphone,
    }
}

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
    /// unsupported format). Only ever populated in the `start_capture`
    /// response — a failed stream is never installed, so `capture_status`
    /// has no session left to recall the error from.
    error: Option<String>,
}

impl StreamStatus {
    fn idle() -> Self {
        StreamStatus {
            running: false,
            format: None,
            frames_captured: 0,
            frames_dropped: 0,
            segments: 0,
            peak: 0.0,
            rms: 0.0,
            error: None,
        }
    }

    fn failed(message: String) -> Self {
        StreamStatus {
            error: Some(message),
            ..StreamStatus::idle()
        }
    }
}

#[derive(serde::Serialize)]
pub struct CaptureStatus {
    loopback: StreamStatus,
    microphone: StreamStatus,
}

fn status_of(capture: &Capture) -> StreamStatus {
    let snapshot = capture.snapshot();
    StreamStatus {
        running: snapshot.running,
        format: Some(capture.format()),
        frames_captured: snapshot.frames_captured,
        frames_dropped: snapshot.frames_dropped,
        segments: snapshot.segments,
        peak: snapshot.peak,
        rms: snapshot.rms,
        error: None,
    }
}

/// Idempotent per source: if that stream is already running, returns its
/// current status instead of starting a second one. A negotiation failure
/// degrades to a `StreamStatus` carrying the error rather than failing the
/// whole command — loopback and microphone are independent, and one
/// stream's failure (no mic, permission denied, unsupported format) must
/// never take down the other.
fn ensure_started(state: &CaptureState, source: CaptureSource) -> Result<StreamStatus, String> {
    {
        let mut guard = state
            .0
            .lock()
            .map_err(|_| "capture state poisoned".to_string())?;
        let running_status = slot_mut(&mut guard, source)
            .as_ref()
            .filter(|existing| existing.is_running())
            .map(status_of);
        if let Some(status) = running_status {
            return Ok(status);
        }
    }
    // Negotiate the device *without* holding the state lock: `Capture::start`
    // blocks until the first segment is live, and concurrent commands (or
    // the other source's own start) must not stall behind that negotiation.
    let capture = match Capture::start(source, FRAME_CAPACITY) {
        Ok(capture) => capture,
        Err(err) => return Ok(StreamStatus::failed(err.to_string())),
    };
    let status = status_of(&capture);

    let mut guard = state
        .0
        .lock()
        .map_err(|_| "capture state poisoned".to_string())?;
    // A concurrent start may have won the race while we were unlocked; if a
    // live session already exists for this source, keep it and discard the
    // one we just built.
    let existing_status = slot_mut(&mut guard, source)
        .as_ref()
        .filter(|existing| existing.is_running())
        .map(status_of);
    if let Some(existing_status) = existing_status {
        drop(guard);
        capture.stop();
        return Ok(existing_status);
    }
    *slot_mut(&mut guard, source) = Some(capture);
    Ok(status)
}

fn start_capture_impl(state: &CaptureState) -> Result<CaptureStatus, String> {
    let loopback = ensure_started(state, CaptureSource::Loopback)?;
    let microphone = ensure_started(state, CaptureSource::Microphone)?;
    Ok(CaptureStatus {
        loopback,
        microphone,
    })
}

/// Reports the session's final counts, but with `running: false` — we are
/// stopping it, so the returned status must not claim it is still live.
fn stop_and_status(capture: Option<Capture>) -> StreamStatus {
    match capture {
        Some(capture) => {
            let mut status = status_of(&capture);
            capture.stop();
            status.running = false;
            status
        }
        None => StreamStatus::idle(),
    }
}

/// Idempotent: stopping a stream that is already idle is a no-op.
fn stop_capture_impl(state: &CaptureState) -> Result<CaptureStatus, String> {
    let (loopback, microphone) = {
        let mut guard = state
            .0
            .lock()
            .map_err(|_| "capture state poisoned".to_string())?;
        (guard.loopback.take(), guard.microphone.take())
    };
    // Stop off-lock: `Capture::stop` joins the capture thread, which must
    // never happen while holding the state mutex.
    Ok(CaptureStatus {
        loopback: stop_and_status(loopback),
        microphone: stop_and_status(microphone),
    })
}

fn capture_status_impl(state: &CaptureState) -> Result<CaptureStatus, String> {
    let guard = state
        .0
        .lock()
        .map_err(|_| "capture state poisoned".to_string())?;
    Ok(CaptureStatus {
        loopback: guard
            .loopback
            .as_ref()
            .map(status_of)
            .unwrap_or_else(StreamStatus::idle),
        microphone: guard
            .microphone
            .as_ref()
            .map(status_of)
            .unwrap_or_else(StreamStatus::idle),
    })
}

#[tauri::command]
pub fn start_capture(state: tauri::State<'_, CaptureState>) -> Result<CaptureStatus, String> {
    start_capture_impl(&state)
}

#[tauri::command]
pub fn stop_capture(state: tauri::State<'_, CaptureState>) -> Result<CaptureStatus, String> {
    stop_capture_impl(&state)
}

#[tauri::command]
pub fn capture_status(state: tauri::State<'_, CaptureState>) -> Result<CaptureStatus, String> {
    capture_status_impl(&state)
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
        let status = stop_capture_impl(&state).unwrap();
        assert!(!status.loopback.running);
        assert!(!status.microphone.running);
    }

    // Starts real loopback + mic streams, so it needs actual audio hardware
    // and is excluded from CI's `cargo test --workspace --locked` (which
    // runs on hosted Windows runners with no guaranteed audio devices) and
    // from the local pre-commit gate. Run manually:
    //   cargo test -p kodama -- --ignored --nocapture
    #[test]
    #[ignore = "starts real capture streams (mic/loopback) — requires audio hardware"]
    fn starting_twice_is_idempotent_per_stream() {
        let state = CaptureState::default();
        let first = start_capture_impl(&state).unwrap();
        let second = start_capture_impl(&state).unwrap();

        assert_eq!(first.loopback.running, second.loopback.running);
        assert_eq!(first.microphone.running, second.microphone.running);

        let stopped = stop_capture_impl(&state).unwrap();
        assert!(!stopped.loopback.running);
        assert!(!stopped.microphone.running);
    }
}
