//! Thin Tauri command wrappers over `kodama_audio::Capture`. All capture
//! logic lives in the `kodama-audio` crate; these commands only manage two
//! independent sessions (loopback + microphone) and map them to a
//! serializable status.

use std::sync::Mutex;

use kodama_audio::{AudioFormat, Capture, CaptureSource};

const FRAME_CAPACITY: usize = 256;

/// One source's live session plus the error (if any) from its last start
/// attempt. `last_error` is retained after a failed `start` — with no
/// `capture` installed there would otherwise be nothing left for
/// `capture_status` to recall the failure from — and cleared on the next
/// successful start or an explicit stop.
#[derive(Default)]
struct SessionSlot {
    capture: Option<Capture>,
    last_error: Option<String>,
}

#[derive(Default)]
struct Sessions {
    loopback: SessionSlot,
    microphone: SessionSlot,
}

#[derive(Default)]
pub struct CaptureState(Mutex<Sessions>);

fn slot_mut(sessions: &mut Sessions, source: CaptureSource) -> &mut SessionSlot {
    match source {
        CaptureSource::Loopback => &mut sessions.loopback,
        CaptureSource::Microphone => &mut sessions.microphone,
    }
}

/// The status a source's slot reports when it is not actively capturing:
/// the persisted start error if the last attempt failed, otherwise idle.
fn idle_status(slot: &SessionSlot) -> StreamStatus {
    match &slot.last_error {
        Some(message) => StreamStatus::failed(message.clone()),
        None => StreamStatus::idle(),
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
    /// unsupported format). The error is persisted in the source's slot, so
    /// both the `start_capture` response and later `capture_status` polls
    /// report it until the next successful start or an explicit stop clears
    /// it.
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
        // Any installed session is a live one — the capture thread only
        // exits on `stop` (which clears the slot), and a mid-session device
        // rebuild keeps the session installed while it re-negotiates. So key
        // idempotency off *presence*, not `is_running()`: a brief rebuild
        // window must not be mistaken for "stopped" and trigger a redundant
        // second start that tears the recovering session down.
        if let Some(capture) = slot_mut(&mut guard, source).capture.as_ref() {
            return Ok(status_of(capture));
        }
    }
    // Negotiate the device *without* holding the state lock: `Capture::start`
    // blocks until the first segment is live, and concurrent commands (or
    // the other source's own start) must not stall behind that negotiation.
    let capture = match Capture::start(source, FRAME_CAPACITY) {
        Ok(capture) => capture,
        Err(err) => {
            let message = err.to_string();
            let mut guard = state
                .0
                .lock()
                .map_err(|_| "capture state poisoned".to_string())?;
            // Persist the failure so later `capture_status` polls can still
            // surface it — with no session installed there is nothing else
            // left to recall the error from.
            slot_mut(&mut guard, source).last_error = Some(message.clone());
            return Ok(StreamStatus::failed(message));
        }
    };
    let status = status_of(&capture);

    let mut guard = state
        .0
        .lock()
        .map_err(|_| "capture state poisoned".to_string())?;
    // A concurrent start may have won the race while we were unlocked; if a
    // session already exists for this source, keep it and discard the one we
    // just built.
    let slot = slot_mut(&mut guard, source);
    if let Some(existing) = slot.capture.as_ref() {
        let existing_status = status_of(existing);
        drop(guard);
        capture.stop();
        return Ok(existing_status);
    }
    slot.last_error = None;
    slot.capture = Some(capture);
    Ok(status)
}

fn start_capture_impl(state: &CaptureState) -> Result<CaptureStatus, String> {
    // Negotiate the two sources concurrently. Each `ensure_started` blocks
    // until its stream's first segment is live (WASAPI negotiation is not
    // instant) and the two devices are independent, so overlapping the two
    // negotiations keeps start latency at ~max(the two) instead of their sum
    // — matching the parallel-capture intent. They contend only on the brief
    // state-lock sections and touch disjoint slots.
    let (loopback, microphone) = std::thread::scope(|scope| {
        let loopback = scope.spawn(|| ensure_started(state, CaptureSource::Loopback));
        let microphone = ensure_started(state, CaptureSource::Microphone);
        (loopback.join(), microphone)
    });
    let loopback = loopback.map_err(|_| "loopback start thread panicked".to_string())??;
    let microphone = microphone?;
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

/// Take a slot's live capture (if any) and clear its persisted start error,
/// so an explicit stop resets the slot to a clean idle rather than leaving a
/// stale failure for `capture_status` to keep reporting.
fn take_slot(slot: &mut SessionSlot) -> Option<Capture> {
    slot.last_error = None;
    slot.capture.take()
}

/// Idempotent: stopping a stream that is already idle is a no-op.
fn stop_capture_impl(state: &CaptureState) -> Result<CaptureStatus, String> {
    let (loopback, microphone) = {
        let mut guard = state
            .0
            .lock()
            .map_err(|_| "capture state poisoned".to_string())?;
        (
            take_slot(&mut guard.loopback),
            take_slot(&mut guard.microphone),
        )
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
            .capture
            .as_ref()
            .map(status_of)
            .unwrap_or_else(|| idle_status(&guard.loopback)),
        microphone: guard
            .microphone
            .capture
            .as_ref()
            .map(status_of)
            .unwrap_or_else(|| idle_status(&guard.microphone)),
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

    #[test]
    fn persisted_start_error_is_reported_by_status_until_stopped() {
        let state = CaptureState::default();
        // Simulate a failed start: the source has no live capture, only the
        // error its last start attempt recorded.
        state.0.lock().unwrap().microphone.last_error = Some("no default microphone".to_string());

        // A later `capture_status` poll must still surface that failure,
        // rather than reporting a bare idle stream.
        let status = capture_status_impl(&state).unwrap();
        assert!(!status.microphone.running);
        assert_eq!(
            status.microphone.error.as_deref(),
            Some("no default microphone")
        );
        assert!(status.loopback.error.is_none());

        // An explicit stop resets the slot to a clean idle.
        let stopped = stop_capture_impl(&state).unwrap();
        assert!(stopped.microphone.error.is_none());
        assert!(capture_status_impl(&state)
            .unwrap()
            .microphone
            .error
            .is_none());
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
