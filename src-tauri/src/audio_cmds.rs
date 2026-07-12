//! Thin Tauri command wrappers over `kodama_audio::LoopbackCapture`. All
//! capture logic lives in the `kodama-audio` crate; these commands only
//! manage the session lifecycle and map it to a serializable status.

use std::sync::Mutex;

use kodama_audio::{AudioFormat, LoopbackCapture};

const FRAME_CAPACITY: usize = 256;

#[derive(Default)]
pub struct CaptureState(Mutex<Option<LoopbackCapture>>);

#[derive(serde::Serialize)]
pub struct CaptureStatus {
    running: bool,
    format: Option<AudioFormat>,
    frames_captured: u64,
    frames_dropped: u64,
    segments: u32,
    peak: f32,
    rms: f32,
}

impl CaptureStatus {
    fn idle() -> Self {
        CaptureStatus {
            running: false,
            format: None,
            frames_captured: 0,
            frames_dropped: 0,
            segments: 0,
            peak: 0.0,
            rms: 0.0,
        }
    }
}

fn status_of(capture: &LoopbackCapture) -> CaptureStatus {
    let snapshot = capture.snapshot();
    CaptureStatus {
        running: snapshot.running,
        format: Some(capture.format()),
        frames_captured: snapshot.frames_captured,
        frames_dropped: snapshot.frames_dropped,
        segments: snapshot.segments,
        peak: snapshot.peak,
        rms: snapshot.rms,
    }
}

/// Idempotent: if capture is already running, returns its current status
/// instead of starting a second stream.
fn start_capture_impl(state: &CaptureState) -> Result<CaptureStatus, String> {
    {
        let guard = state
            .0
            .lock()
            .map_err(|_| "capture state poisoned".to_string())?;
        if let Some(capture) = guard.as_ref() {
            if capture.is_running() {
                return Ok(status_of(capture));
            }
        }
    }
    // Negotiate the device *without* holding the state lock: `start()` blocks
    // until the first segment is live, and concurrent `capture_status` /
    // `stop_capture` commands must not stall behind that negotiation.
    let capture = LoopbackCapture::start(FRAME_CAPACITY).map_err(|e| e.to_string())?;
    let status = status_of(&capture);

    let mut guard = state
        .0
        .lock()
        .map_err(|_| "capture state poisoned".to_string())?;
    // A concurrent start may have won the race while we were unlocked; if a
    // live session already exists, keep it and discard the one we just built.
    if let Some(existing) = guard.as_ref() {
        if existing.is_running() {
            let existing_status = status_of(existing);
            drop(guard);
            capture.stop();
            return Ok(existing_status);
        }
    }
    *guard = Some(capture);
    Ok(status)
}

/// Idempotent: stopping while already idle returns an idle status rather
/// than erroring.
fn stop_capture_impl(state: &CaptureState) -> Result<CaptureStatus, String> {
    let mut guard = state
        .0
        .lock()
        .map_err(|_| "capture state poisoned".to_string())?;
    match guard.take() {
        Some(capture) => {
            // Report the session's final counts, but with `running: false` —
            // we are stopping it, so the returned status must not claim it is
            // still live.
            let mut status = status_of(&capture);
            capture.stop();
            status.running = false;
            Ok(status)
        }
        None => Ok(CaptureStatus::idle()),
    }
}

fn capture_status_impl(state: &CaptureState) -> Result<CaptureStatus, String> {
    let guard = state
        .0
        .lock()
        .map_err(|_| "capture state poisoned".to_string())?;
    Ok(guard
        .as_ref()
        .map(status_of)
        .unwrap_or_else(CaptureStatus::idle))
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
        assert!(!status.running);
        assert!(status.format.is_none());
    }

    #[test]
    fn stop_capture_while_idle_is_a_no_op() {
        let state = CaptureState::default();
        let status = stop_capture_impl(&state).unwrap();
        assert!(!status.running);
    }
}
