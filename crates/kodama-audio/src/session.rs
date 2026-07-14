//! The two-channel capture session: owns the loopback and microphone
//! [`Capture`]s plus the [`Combiner`] that aligns them, and manages their
//! shared lifecycle (parallel start, per-source failure retention, combiner
//! spawn/finalize). This is the orchestration `src-tauri` used to inline —
//! keeping it here leaves the Tauri command layer a thin mapping to
//! serializable DTOs, per the crate's core-vs-shell split.

use std::sync::Mutex;
use std::thread;

use crate::capture::Capture;
use crate::combine::{AlignedSession, Combiner};
use crate::error::{AudioError, Result};
use crate::format::AudioFormat;
use crate::source::CaptureSource;

/// One source's live capture plus the error (if any) from its last start
/// attempt. `last_error` is retained after a failed start — with no `capture`
/// installed there would otherwise be nothing left for a status query to
/// recall the failure from — and cleared on the next successful start or an
/// explicit stop.
#[derive(Default)]
struct Slot {
    capture: Option<Capture>,
    last_error: Option<String>,
}

/// Point-in-time status of one capture source: live meter readings when it is
/// capturing, otherwise idle (carrying the persisted start error, if any).
#[derive(Clone, Debug)]
pub struct SourceStatus {
    pub running: bool,
    pub format: Option<AudioFormat>,
    pub frames_captured: u64,
    pub frames_dropped: u64,
    pub segments: u32,
    pub peak: f32,
    pub rms: f32,
    /// Set when this source's last start attempt failed (no device,
    /// permission denied, unsupported format), until the next successful
    /// start or an explicit stop clears it.
    pub error: Option<String>,
}

impl SourceStatus {
    fn idle() -> Self {
        SourceStatus {
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
        SourceStatus {
            error: Some(message),
            ..SourceStatus::idle()
        }
    }

    fn live(capture: &Capture) -> Self {
        let snapshot = capture.snapshot();
        SourceStatus {
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
}

/// Status of both capture sources.
#[derive(Clone, Debug)]
pub struct DualStatus {
    pub loopback: SourceStatus,
    pub microphone: SourceStatus,
}

#[derive(Default)]
struct Inner {
    loopback: Slot,
    microphone: Slot,
    /// The two-channel combiner, spawned once both sources are confirmed
    /// live (see [`DualCapture::ensure_combiner_started`]). Taken and
    /// finalized in [`DualCapture::stop`].
    combiner: Option<Combiner>,
}

/// A two-channel capture session: loopback (system audio) + microphone,
/// combined into one time-aligned [`AlignedSession`] at stop.
pub struct DualCapture {
    inner: Mutex<Inner>,
    frame_capacity: usize,
    target_rate: u32,
}

impl DualCapture {
    /// `frame_capacity` bounds each source's item channel; `target_rate` is
    /// the common rate the combiner aligns both channels to.
    pub fn new(frame_capacity: usize, target_rate: u32) -> Self {
        DualCapture {
            inner: Mutex::new(Inner::default()),
            frame_capacity,
            target_rate,
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Inner>> {
        self.inner.lock().map_err(|_| AudioError::StatePoisoned)
    }

    /// Idempotently start both sources and, once both are live, the combiner.
    ///
    /// The two sources are negotiated concurrently: each start blocks until
    /// its stream's first segment is live (WASAPI negotiation is not instant)
    /// and the two devices are independent, so overlapping the negotiations
    /// keeps start latency at ~max(the two) instead of their sum. A single
    /// source's failure (no mic, permission denied, unsupported format) is
    /// retained on its slot and reported in the returned status rather than
    /// failing the whole start — the two sources are independent, and one
    /// must never take down the other.
    pub fn start(&self) -> Result<DualStatus> {
        let (loopback, microphone) = thread::scope(|scope| {
            let loopback = scope.spawn(|| self.ensure_started(CaptureSource::Loopback));
            let microphone = self.ensure_started(CaptureSource::Microphone);
            (loopback.join(), microphone)
        });
        let loopback = loopback.map_err(|_| AudioError::StartThreadPanicked)??;
        let microphone = microphone?;

        self.ensure_combiner_started()?;

        Ok(DualStatus {
            loopback,
            microphone,
        })
    }

    /// Idempotent per source: if that stream is already installed, return its
    /// current status instead of starting a second one.
    fn ensure_started(&self, source: CaptureSource) -> Result<SourceStatus> {
        {
            let mut guard = self.lock()?;
            // Any installed session is a live one — the capture thread only
            // exits on `stop` (which clears the slot), and a mid-session
            // device rebuild keeps the session installed while it
            // re-negotiates. So key idempotency off *presence*, not
            // `is_running()`: a brief rebuild window must not be mistaken for
            // "stopped" and trigger a redundant second start that tears the
            // recovering session down.
            if let Some(capture) = slot_mut(&mut guard, source).capture.as_ref() {
                return Ok(SourceStatus::live(capture));
            }
        }
        // Negotiate the device *without* holding the state lock: `Capture::start`
        // blocks until the first segment is live, and concurrent commands (or
        // the other source's own start) must not stall behind that negotiation.
        let capture = match Capture::start(source, self.frame_capacity) {
            Ok(capture) => capture,
            Err(err) => {
                let message = err.to_string();
                let mut guard = self.lock()?;
                slot_mut(&mut guard, source).last_error = Some(message.clone());
                return Ok(SourceStatus::failed(message));
            }
        };
        let status = SourceStatus::live(&capture);

        let mut guard = self.lock()?;
        // A concurrent start may have won the race while we were unlocked; if
        // a session already exists for this source, keep it and discard the
        // one we just built.
        let slot = slot_mut(&mut guard, source);
        if let Some(existing) = slot.capture.as_ref() {
            let existing_status = SourceStatus::live(existing);
            drop(guard);
            capture.stop();
            return Ok(existing_status);
        }
        slot.last_error = None;
        slot.capture = Some(capture);
        Ok(status)
    }

    /// Spawn the two-channel combiner once both sources are confirmed live,
    /// if one isn't already running. A partial start (only one source live)
    /// leaves no combiner — there is simply no two-channel session to combine
    /// yet; a later successful start picks it up. A spawn failure is likewise
    /// left as "no combiner this session" rather than failing the whole
    /// start, matching how a single source's own failure never takes the
    /// other down.
    fn ensure_combiner_started(&self) -> Result<()> {
        let mut guard = self.lock()?;
        if guard.combiner.is_some() {
            return Ok(());
        }
        let (mic_items, loopback_items) = match (&guard.microphone.capture, &guard.loopback.capture)
        {
            (Some(mic), Some(loopback)) => (mic.items(), loopback.items()),
            _ => return Ok(()),
        };
        if let Ok(combiner) = Combiner::start(mic_items, loopback_items, self.target_rate) {
            guard.combiner = Some(combiner);
        }
        Ok(())
    }

    /// Stop both sources and finalize the combiner, returning the aligned
    /// two-channel session if one ran (both sources were live long enough for
    /// the combiner to be spawned). Idempotent: stopping while idle is a
    /// no-op that returns idle statuses and `None`.
    pub fn stop(&self) -> Result<(DualStatus, Option<AlignedSession>)> {
        let (loopback, microphone, combiner) = {
            let mut guard = self.lock()?;
            (
                take_slot(&mut guard.loopback),
                take_slot(&mut guard.microphone),
                guard.combiner.take(),
            )
        };
        // Stop off-lock: `Capture::stop` joins the capture thread, which must
        // never happen while holding the state mutex. Both captures are fully
        // stopped (and their frame-channel senders dropped) before the
        // combiner is finalized, so `Combiner::finish` — which also joins a
        // thread and blocks until both of its receivers disconnect — cannot
        // hang waiting on a stream that was never told to stop.
        let loopback_status = stop_and_status(loopback);
        let microphone_status = stop_and_status(microphone);
        let aligned_session = combiner.map(Combiner::finish);

        Ok((
            DualStatus {
                loopback: loopback_status,
                microphone: microphone_status,
            },
            aligned_session,
        ))
    }

    /// Current per-source status, without disturbing capture.
    pub fn status(&self) -> Result<DualStatus> {
        let guard = self.lock()?;
        Ok(DualStatus {
            loopback: status_of_slot(&guard.loopback),
            microphone: status_of_slot(&guard.microphone),
        })
    }

    /// True while either source has a live capture installed (started, not
    /// yet stopped). Keys off slot *presence*, not `is_running()`, matching
    /// `ensure_started`'s idempotency — a mid-session device-rebuild window
    /// still reports active rather than being mistaken for "stopped".
    pub fn is_active(&self) -> Result<bool> {
        let guard = self.lock()?;
        Ok(guard.loopback.capture.is_some() || guard.microphone.capture.is_some())
    }
}

fn slot_mut(inner: &mut Inner, source: CaptureSource) -> &mut Slot {
    match source {
        CaptureSource::Loopback => &mut inner.loopback,
        CaptureSource::Microphone => &mut inner.microphone,
    }
}

/// Live status if the slot is capturing, otherwise idle — carrying the
/// persisted start error when the last attempt failed.
fn status_of_slot(slot: &Slot) -> SourceStatus {
    match &slot.capture {
        Some(capture) => SourceStatus::live(capture),
        None => match &slot.last_error {
            Some(message) => SourceStatus::failed(message.clone()),
            None => SourceStatus::idle(),
        },
    }
}

/// Take a slot's live capture (if any) and clear its persisted start error,
/// so an explicit stop resets the slot to a clean idle rather than leaving a
/// stale failure for a later status query to keep reporting.
fn take_slot(slot: &mut Slot) -> Option<Capture> {
    slot.last_error = None;
    slot.capture.take()
}

/// Report a stopped source's final counts, but with `running: false` — we are
/// stopping it, so the returned status must not claim it is still live.
fn stop_and_status(capture: Option<Capture>) -> SourceStatus {
    match capture {
        Some(capture) => {
            let mut status = SourceStatus::live(&capture);
            capture.stop();
            status.running = false;
            status
        }
        None => SourceStatus::idle(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_idle_before_any_start() {
        let dual = DualCapture::new(4, 48_000);
        let status = dual.status().unwrap();
        assert!(!status.loopback.running);
        assert!(status.loopback.format.is_none());
        assert!(!status.microphone.running);
        assert!(status.microphone.format.is_none());
    }

    #[test]
    fn stop_while_idle_is_a_no_op() {
        let dual = DualCapture::new(4, 48_000);
        let (status, session) = dual.stop().unwrap();
        assert!(!status.loopback.running);
        assert!(!status.microphone.running);
        assert!(session.is_none());
    }

    #[test]
    fn is_active_is_false_before_any_start() {
        let dual = DualCapture::new(4, 48_000);
        assert!(!dual.is_active().unwrap());
    }

    #[test]
    fn persisted_start_error_is_reported_by_status_until_stopped() {
        let dual = DualCapture::new(4, 48_000);
        // Simulate a failed start: the source has no live capture, only the
        // error its last start attempt recorded.
        dual.inner.lock().unwrap().microphone.last_error =
            Some("no default microphone".to_string());

        // A later status query must still surface that failure, rather than
        // reporting a bare idle stream.
        let status = dual.status().unwrap();
        assert!(!status.microphone.running);
        assert_eq!(
            status.microphone.error.as_deref(),
            Some("no default microphone")
        );
        assert!(status.loopback.error.is_none());

        // An explicit stop resets the slot to a clean idle.
        let (stopped, _) = dual.stop().unwrap();
        assert!(stopped.microphone.error.is_none());
        assert!(dual.status().unwrap().microphone.error.is_none());
    }

    // Starts real loopback + mic streams, so it needs actual audio hardware
    // and is excluded from CI's `cargo test --workspace --locked` (which runs
    // on hosted Windows runners with no guaranteed audio devices) and from the
    // local pre-commit gate. Run manually:
    //   cargo test -p kodama-audio -- --ignored --nocapture
    #[test]
    #[ignore = "starts real capture streams (mic/loopback) — requires audio hardware"]
    fn starting_twice_is_idempotent_per_stream() {
        let dual = DualCapture::new(256, 48_000);
        let first = dual.start().unwrap();
        let second = dual.start().unwrap();

        assert_eq!(first.loopback.running, second.loopback.running);
        assert_eq!(first.microphone.running, second.microphone.running);
        assert!(dual.is_active().unwrap());

        let (stopped, _) = dual.stop().unwrap();
        assert!(!stopped.loopback.running);
        assert!(!stopped.microphone.running);
        assert!(!dual.is_active().unwrap());
    }
}
