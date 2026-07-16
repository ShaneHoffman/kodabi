//! The two-channel capture session: owns the loopback and microphone
//! [`Capture`]s plus the [`Combiner`] that aligns them, and manages their
//! shared lifecycle (parallel start, per-source failure retention, combiner
//! spawn/finalize). This is the orchestration `src-tauri` used to inline —
//! keeping it here leaves the Tauri command layer a thin mapping to
//! serializable DTOs, per the crate's core-vs-shell split.

use std::sync::Mutex;
use std::thread;

use crate::capture::Capture;
use crate::combine::{AlignedSession, Combiner, SessionChannel};
use crate::error::{AudioError, Result};
use crate::format::AudioFormat;
use crate::resample::ResampleParams;
use crate::source::CaptureSource;
use crate::tuning::apply_positive_usize_override;

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
    /// The two-channel combiner, spawned by the *first* source to go live and
    /// then fed each source's item stream as it comes up (see
    /// [`DualCapture::attach_source`]). Taken and finalized in
    /// [`DualCapture::stop`].
    combiner: Option<CombinerState>,
}

/// The running combiner plus which channels have been attached to it this
/// session. Both flags gate two things: an attach happens at most once per
/// channel (a mid-session device rebuild re-runs `start` without re-attaching
/// an already-live source), and a two-channel [`AlignedSession`] is only
/// returned from [`DualCapture::stop`] once *both* channels attached — a
/// single source produces no two-channel artifact, matching the pre-existing
/// contract.
struct CombinerState {
    combiner: Combiner,
    mic_attached: bool,
    system_attached: bool,
}

impl CombinerState {
    fn attached_flag(&mut self, channel: SessionChannel) -> &mut bool {
        match channel {
            SessionChannel::Mic => &mut self.mic_attached,
            SessionChannel::System => &mut self.system_attached,
        }
    }

    fn both_attached(&self) -> bool {
        self.mic_attached && self.system_attached
    }
}

/// The aligned-session channel a capture source feeds: the microphone is the
/// "you" channel, the system-audio loopback is the "them" channel.
fn channel_of(source: CaptureSource) -> SessionChannel {
    match source {
        CaptureSource::Microphone => SessionChannel::Mic,
        CaptureSource::Loopback => SessionChannel::System,
    }
}

/// Runtime-tunable capture-path knobs (FOUNDING_DOC §3.7's resource budget,
/// `docs/RESOURCE_BUDGET.md`) — bundled so [`DualCapture::new`] takes one
/// value rather than an ever-growing parameter list as more knobs are added.
#[derive(Debug, Clone, Copy)]
pub struct CaptureTuning {
    /// Bounds each source's item channel — the slack between a cpal callback
    /// enqueuing a frame and the combiner's coordinator thread draining it.
    pub frame_capacity: usize,
    /// Common rate the combiner aligns mic and system audio to. Not
    /// env-overridable (a device-format concern, not a resource knob).
    pub target_rate: u32,
    /// Live resampler chunk size + sinc quality (see [`ResampleParams`]).
    pub resample: ResampleParams,
}

impl CaptureTuning {
    /// `frame_capacity`/`target_rate` as given; `resample` defaults to
    /// [`ResampleParams::default`]. The plain constructor callers (tests, the
    /// `record_meeting` example) use when they don't need env overrides.
    pub fn new(frame_capacity: usize, target_rate: u32) -> Self {
        CaptureTuning {
            frame_capacity,
            target_rate,
            resample: ResampleParams::default(),
        }
    }

    /// [`CaptureTuning::new`] with `KODABI_FRAME_CAPACITY` and the resample
    /// env overrides ([`ResampleParams::from_env`]) applied — the production
    /// entry point, so a resource-budget pass can iterate on real hardware
    /// without recompiling.
    pub fn from_env(frame_capacity: usize, target_rate: u32) -> Self {
        let mut tuning = CaptureTuning::new(frame_capacity, target_rate);
        tuning.frame_capacity = apply_positive_usize_override(
            tuning.frame_capacity,
            std::env::var("KODABI_FRAME_CAPACITY").ok(),
        );
        tuning.resample = ResampleParams::from_env();
        tuning
    }
}

/// A two-channel capture session: loopback (system audio) + microphone,
/// combined into one time-aligned [`AlignedSession`] at stop.
pub struct DualCapture {
    inner: Mutex<Inner>,
    frame_capacity: usize,
    target_rate: u32,
    resample: ResampleParams,
}

impl DualCapture {
    /// See [`CaptureTuning`] for what each knob controls.
    pub fn new(tuning: CaptureTuning) -> Self {
        DualCapture {
            inner: Mutex::new(Inner::default()),
            frame_capacity: tuning.frame_capacity,
            target_rate: tuning.target_rate,
            resample: tuning.resample,
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Inner>> {
        self.inner.lock().map_err(|_| AudioError::StatePoisoned)
    }

    /// Idempotently start both sources, attaching each to the combiner the
    /// moment it goes live.
    ///
    /// The two sources are negotiated concurrently: each start blocks until
    /// its stream's first segment is live (WASAPI negotiation is not instant)
    /// and the two devices are independent, so overlapping the negotiations
    /// keeps start latency at ~max(the two) instead of their sum. Each source
    /// attaches to the combiner as soon as *it* is live rather than waiting for
    /// its sibling, so the first source starts draining immediately and its
    /// bounded item channel never backs up over the other's negotiation window
    /// (which would drop real audio). A single source's failure (no mic,
    /// permission denied, unsupported format) is retained on its slot and
    /// reported in the returned status rather than failing the whole start —
    /// the two sources are independent, and one must never take down the other.
    pub fn start(&self) -> Result<DualStatus> {
        let (loopback, microphone) = thread::scope(|scope| {
            let loopback = scope.spawn(|| self.start_and_attach(CaptureSource::Loopback));
            let microphone = self.start_and_attach(CaptureSource::Microphone);
            (loopback.join(), microphone)
        });
        let loopback = loopback.map_err(|_| AudioError::StartThreadPanicked)??;
        let microphone = microphone?;

        // Reconcile any live-but-unattached source. The per-source path spawns
        // the combiner lazily on the first source to reach `attach_source`, so
        // if that spawn hit a transient failure the source ran before any
        // combiner existed and left itself unattached — while a sibling may
        // have since spawned one. `attach_source` is idempotent, so re-running
        // it here attaches such a stranded source (now that the combiner
        // exists) and is a no-op for an already-attached or failed source. This
        // keeps a spawn hiccup on one source from silently dropping the other's
        // stream (and, via `both_attached`, the whole session).
        self.attach_source(CaptureSource::Loopback)?;
        self.attach_source(CaptureSource::Microphone)?;

        Ok(DualStatus {
            loopback,
            microphone,
        })
    }

    /// Start one source and, if it came up live, attach its item stream to the
    /// combiner right away. Runs in the per-source concurrent path so the
    /// first source to negotiate begins draining into the combiner while the
    /// other is still coming up — its bounded item channel never backs up
    /// waiting for a shared "both live" barrier.
    fn start_and_attach(&self, source: CaptureSource) -> Result<SourceStatus> {
        let status = self.ensure_started(source)?;
        self.attach_source(source)?;
        Ok(status)
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

    /// Attach a freshly-live source's item stream to the combiner, spawning
    /// the combiner on the first source to attach. Idempotent per source: a
    /// source with no live capture (a failed start) has nothing to attach, and
    /// an already-attached source (an idempotent re-`start` of a still-live
    /// source) is not attached twice. A combiner spawn failure is left as "no
    /// combiner this session" rather than failing the whole start, matching how
    /// a single source's own failure never takes the other down.
    fn attach_source(&self, source: CaptureSource) -> Result<()> {
        let mut guard = self.lock()?;
        // A failed start left no capture in the slot — nothing to attach.
        let Some(capture) = slot_mut(&mut guard, source).capture.as_ref() else {
            return Ok(());
        };
        let items = capture.items();

        // Spawn the combiner lazily on the first source; a spawn failure means
        // no combiner this session (best-effort, as above).
        if guard.combiner.is_none() {
            match Combiner::start(self.target_rate, self.resample) {
                Ok(combiner) => {
                    guard.combiner = Some(CombinerState {
                        combiner,
                        mic_attached: false,
                        system_attached: false,
                    });
                }
                Err(_) => return Ok(()),
            }
        }

        let channel = channel_of(source);
        let state = guard
            .combiner
            .as_mut()
            .expect("combiner just ensured present");
        if *state.attached_flag(channel) {
            return Ok(());
        }
        // Record the channel as attached only once the hand-off actually lands,
        // so `both_attached` never reports a stream the coordinator never
        // received (and a failed hand-off stays retryable via `attached_flag`).
        if state.combiner.attach(channel, items) {
            *state.attached_flag(channel) = true;
        }
        Ok(())
    }

    /// Stop both sources and finalize the combiner, returning the aligned
    /// two-channel session only when both sources attached to it (a lone
    /// source yields no two-channel artifact). Idempotent: stopping while idle
    /// is a no-op that returns idle statuses and `None`.
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
        // thread and blocks until every attached receiver disconnects — cannot
        // hang waiting on a stream that was never told to stop.
        let loopback_status = stop_and_status(loopback);
        let microphone_status = stop_and_status(microphone);
        // Always finalize (to join the coordinator thread), but only surface a
        // two-channel session when both sources actually attached — a lone
        // source yields no two-channel artifact.
        let aligned_session = combiner.and_then(|state| {
            let both = state.both_attached();
            let session = state.combiner.finish();
            both.then_some(session)
        });

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
        let dual = DualCapture::new(CaptureTuning::new(4, 48_000));
        let status = dual.status().unwrap();
        assert!(!status.loopback.running);
        assert!(status.loopback.format.is_none());
        assert!(!status.microphone.running);
        assert!(status.microphone.format.is_none());
    }

    #[test]
    fn stop_while_idle_is_a_no_op() {
        let dual = DualCapture::new(CaptureTuning::new(4, 48_000));
        let (status, session) = dual.stop().unwrap();
        assert!(!status.loopback.running);
        assert!(!status.microphone.running);
        assert!(session.is_none());
    }

    #[test]
    fn is_active_is_false_before_any_start() {
        let dual = DualCapture::new(CaptureTuning::new(4, 48_000));
        assert!(!dual.is_active().unwrap());
    }

    #[test]
    fn persisted_start_error_is_reported_by_status_until_stopped() {
        let dual = DualCapture::new(CaptureTuning::new(4, 48_000));
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
    //   cargo test -p kodabi-audio -- --ignored --nocapture
    #[test]
    #[ignore = "starts real capture streams (mic/loopback) — requires audio hardware"]
    fn starting_twice_is_idempotent_per_stream() {
        let dual = DualCapture::new(CaptureTuning::new(256, 48_000));
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
