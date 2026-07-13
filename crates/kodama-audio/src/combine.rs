//! Combines the mic and system-audio capture streams into a single
//! time-aligned two-channel session: channel 0 = mic = "you", channel 1 =
//! system = "them" (FOUNDING_DOC §3.3's "two-channel bonus").
//!
//! `Combiner` is the sole consumer of each `Capture::items()` stream (that
//! channel is MPMC — see `capture.rs` — so a second independent sink such as
//! persistence would need its own broadcast layer; nothing needs one yet).
//! A single coordinator thread drains both streams, downmixes each frame to
//! mono, resamples it to a common target rate, and appends it to that
//! source's timeline. `finish()` blocks until both `Capture`s have stopped
//! (their senders drop, disconnecting these receivers) and returns the
//! finalized [`AlignedSession`].

use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Select};

use crate::drift::{DriftController, GapCorrection};
use crate::error::Result;
use crate::frame::{AudioFrame, CaptureItem};
use crate::mix::{downmix_to_mono, interleave_stereo};
use crate::resample::MonoResampler;

/// How often (of wall-clock time) a source's drift is re-evaluated. See
/// `drift.rs` for what the correction itself does.
const DRIFT_CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// One capture source's position in the aligned session: channel 0 = mic =
/// "you", channel 1 = system = "them" (`docs/MCP_TOOL_SURFACE.md`'s
/// `Channel` enum). Kept positional here — mapping to the wire-level
/// you/them label happens at the transcription/persistence boundary, so
/// `kodama-audio` doesn't need a dependency on `kodama-core` for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionChannel {
    Mic,
    System,
}

/// The finalized output of a [`Combiner`]: two mono channels, resampled to a
/// common rate and aligned to a shared origin, padded to equal length.
#[derive(Clone, Debug)]
pub struct AlignedSession {
    sample_rate: u32,
    mic: Vec<f32>,
    system: Vec<f32>,
}

impl AlignedSession {
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Borrow one channel's mono samples. Both channels are the same length
    /// (the shorter was zero-padded at finalize), so callers can treat them
    /// as parallel — e.g. feed each straight to a transcription engine as a
    /// mono `AudioChunk`, or interleave for a stereo WAV.
    pub fn channel(&self, ch: SessionChannel) -> &[f32] {
        match ch {
            SessionChannel::Mic => &self.mic,
            SessionChannel::System => &self.system,
        }
    }

    /// Frame count per channel (both channels share this length).
    pub fn frames(&self) -> usize {
        self.mic.len()
    }

    /// Interleave both channels into one stereo buffer: `L = mic/you, R =
    /// system/them`.
    pub fn interleaved_stereo(&self) -> Vec<f32> {
        interleave_stereo(&self.mic, &self.system)
    }

    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.frames() as f64 / self.sample_rate.max(1) as f64)
    }
}

/// One source's downmix -> resample -> drift-correct -> accumulate pipeline.
/// Not `pub`: an internal building block of the coordinator loop, not part
/// of the combiner's public surface.
struct SourcePipeline {
    target_rate: u32,
    resampler: Option<MonoResampler>,
    /// The segment this pipeline's resampler was built for. `AudioFrame`
    /// carries its own `segment_id`, so a change here (rather than relying
    /// on the separately delivered, droppable `SegmentStarted` marker) is
    /// the authoritative signal that the format may have changed.
    segment_id: Option<u32>,
    drift: DriftController,
    out: Vec<f32>,
    /// Whether this pipeline has processed its first frame yet — gates the
    /// one-time leading-silence padding that aligns it to the session
    /// origin `t0`.
    started: bool,
    last_drift_check: Option<Instant>,
}

impl SourcePipeline {
    fn new(target_rate: u32) -> Self {
        SourcePipeline {
            target_rate,
            resampler: None,
            segment_id: None,
            drift: DriftController::new(target_rate),
            out: Vec::new(),
            started: false,
            last_drift_check: None,
        }
    }

    /// Rebuild the resampler if `segment_id` indicates the source's format
    /// may have changed (a fresh segment, including the very first one).
    /// Flushes the outgoing resampler's buffered tail first so no audio is
    /// lost across the switch. A construction failure (e.g. a malformed
    /// `sample_rate` of `0`) leaves `resampler` `None` rather than
    /// panicking — frames are then silently dropped until the next segment
    /// gives the format another chance to resolve, since one bad segment
    /// must never take down the whole session.
    fn ensure_resampler_for(&mut self, sample_rate: u32, segment_id: u32) {
        if self.segment_id == Some(segment_id) && self.resampler.is_some() {
            return;
        }
        if let Some(mut old) = self.resampler.take() {
            self.out.extend(old.flush());
        }
        self.segment_id = Some(segment_id);
        self.resampler = MonoResampler::new(sample_rate, self.target_rate).ok();
    }

    /// Process one captured frame: rebuild the resampler if the segment
    /// changed, align to the shared origin `t0` on the very first frame,
    /// downmix + resample + accumulate, then re-evaluate drift.
    fn handle_frame(&mut self, frame: AudioFrame, now: Instant, t0: Instant) {
        self.ensure_resampler_for(frame.format.sample_rate, frame.segment_id);

        if !self.started {
            self.started = true;
            // The earlier of the two sources has offset zero by
            // definition (it *is* `t0`); the later source gets leading
            // silence so both channels share the same origin sample.
            let offset = now.saturating_duration_since(t0);
            let offset_samples = (offset.as_secs_f64() * self.target_rate as f64).round() as usize;
            self.out.resize(offset_samples, 0.0);
        }

        let mono = downmix_to_mono(&frame.samples, frame.format.channels);
        if let Some(resampler) = self.resampler.as_mut() {
            let resampled = resampler.push(&mono);
            self.out.extend(resampled);
        }

        self.apply_drift(now, t0);
    }

    /// On a coarse (~1s) cadence, compare this source's accumulated output
    /// against what elapsed wall-clock time since `t0` implies it should
    /// have, and apply the resulting ratio nudge / silence-fill / trim. See
    /// `drift.rs` for the "crude but useful" rationale.
    fn apply_drift(&mut self, now: Instant, t0: Instant) {
        let due = match self.last_drift_check {
            None => true,
            Some(last) => now.duration_since(last) >= DRIFT_CHECK_INTERVAL,
        };
        if !due {
            return;
        }
        self.last_drift_check = Some(now);

        let elapsed = now.duration_since(t0);
        let output_written = self.out.len() as u64;

        let ratio = self.drift.correction(elapsed, output_written);
        if let Some(resampler) = self.resampler.as_mut() {
            resampler.set_ratio_relative(ratio);
        }

        match self.drift.gap_correction(elapsed, output_written) {
            Some(GapCorrection::InsertSilence(n)) => {
                let new_len = self.out.len() + n;
                self.out.resize(new_len, 0.0);
            }
            Some(GapCorrection::TrimSamples(n)) => {
                let new_len = self.out.len().saturating_sub(n);
                self.out.truncate(new_len);
            }
            None => {}
        }
    }

    /// Drain any buffered tail from the current resampler. Called once at
    /// session end.
    fn flush(&mut self) {
        if let Some(resampler) = self.resampler.as_mut() {
            let tail = resampler.flush();
            self.out.extend(tail);
        }
    }
}

fn process_item(
    pipeline: &mut SourcePipeline,
    item: CaptureItem,
    now: Instant,
    t0: &mut Option<Instant>,
) {
    match item {
        // Frames are self-describing (`frame.format`, `frame.segment_id`),
        // so a dropped `SegmentStarted` marker loses nothing — this
        // coordinator only reacts to segment/format changes it observes on
        // the frames themselves.
        CaptureItem::SegmentStarted { .. } => {}
        CaptureItem::Frame(frame) => {
            let origin = *t0.get_or_insert(now);
            pipeline.handle_frame(frame, now, origin);
        }
    }
}

fn finalize(
    mut mic: SourcePipeline,
    mut system: SourcePipeline,
    target_rate: u32,
) -> AlignedSession {
    mic.flush();
    system.flush();

    let frames = mic.out.len().max(system.out.len());
    mic.out.resize(frames, 0.0);
    system.out.resize(frames, 0.0);

    AlignedSession {
        sample_rate: target_rate,
        mic: mic.out,
        system: system.out,
    }
}

/// Drains both capture item streams until each disconnects (i.e. until both
/// `Capture`s are stopped), aligning and resampling as it goes.
fn coordinator_loop(
    mic_rx: Receiver<CaptureItem>,
    system_rx: Receiver<CaptureItem>,
    target_rate: u32,
) -> AlignedSession {
    let mut mic_pipeline = SourcePipeline::new(target_rate);
    let mut system_pipeline = SourcePipeline::new(target_rate);
    let mut t0: Option<Instant> = None;

    let mut mic_open = true;
    let mut system_open = true;

    while mic_open || system_open {
        let mut select = Select::new();
        let mic_index = mic_open.then(|| select.recv(&mic_rx));
        let system_index = system_open.then(|| select.recv(&system_rx));

        let oper = select.select();
        let index = oper.index();
        let now = Instant::now();

        if Some(index) == mic_index {
            match oper.recv(&mic_rx) {
                Ok(item) => process_item(&mut mic_pipeline, item, now, &mut t0),
                Err(_) => mic_open = false,
            }
        } else if Some(index) == system_index {
            match oper.recv(&system_rx) {
                Ok(item) => process_item(&mut system_pipeline, item, now, &mut t0),
                Err(_) => system_open = false,
            }
        }
    }

    finalize(mic_pipeline, system_pipeline, target_rate)
}

/// Combines a mic and a system-audio capture stream into one time-aligned
/// two-channel [`AlignedSession`]. Owns one `items()` receiver from each
/// `Capture` and is the sole consumer of both.
pub struct Combiner {
    handle: JoinHandle<AlignedSession>,
}

impl Combiner {
    /// Spawn the coordinator thread and start draining both streams
    /// immediately.
    pub fn start(
        mic: Receiver<CaptureItem>,
        system: Receiver<CaptureItem>,
        target_rate: u32,
    ) -> Result<Combiner> {
        let handle = thread::Builder::new()
            .name("kodama-audio-combiner".to_string())
            .spawn(move || coordinator_loop(mic, system, target_rate))
            .map_err(|_| crate::error::AudioError::ThreadStart)?;
        Ok(Combiner { handle })
    }

    /// Block until both capture streams disconnect (i.e. after both
    /// `Capture`s are stopped) and return the finalized aligned session.
    pub fn finish(self) -> AlignedSession {
        self.handle
            .join()
            .expect("combiner coordinator thread panicked")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{AudioFormat, SampleTag};

    fn mono_format(sample_rate: u32) -> AudioFormat {
        AudioFormat {
            sample_rate,
            channels: 1,
            source_format: SampleTag::F32,
        }
    }

    fn stereo_format(sample_rate: u32) -> AudioFormat {
        AudioFormat {
            sample_rate,
            channels: 2,
            source_format: SampleTag::F32,
        }
    }

    #[test]
    fn first_arriving_pipeline_gets_no_leading_silence() {
        let mut pipeline = SourcePipeline::new(48_000);
        let t0 = Instant::now();
        let frame = AudioFrame {
            segment_id: 0,
            samples: vec![0.5; 480],
            format: mono_format(48_000),
        };
        pipeline.handle_frame(frame, t0, t0);
        // 480 samples is short of the resampler's 1024-frame chunk, so
        // nothing has been produced yet — only the (zero-length) origin
        // padding applies.
        assert_eq!(pipeline.out.len(), 0);
    }

    #[test]
    fn later_arriving_pipeline_gets_leading_silence_padding() {
        let mut pipeline = SourcePipeline::new(48_000);
        let t0 = Instant::now();
        let arrival = t0 + Duration::from_millis(50);
        let frame = AudioFrame {
            segment_id: 0,
            samples: vec![0.5; 480],
            format: mono_format(48_000),
        };
        pipeline.handle_frame(frame, arrival, t0);
        // 50ms at 48kHz = 2400 samples of leading silence; the frame itself
        // produces no output yet (short of a full resample chunk).
        assert_eq!(pipeline.out.len(), 2_400);
        assert!(pipeline.out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn segment_change_to_a_new_rate_rebuilds_the_resampler_and_keeps_output() {
        let mut pipeline = SourcePipeline::new(48_000);
        let t0 = Instant::now();

        let frame_a = AudioFrame {
            segment_id: 0,
            samples: vec![0.1; 2_000],
            format: mono_format(44_100),
        };
        pipeline.handle_frame(frame_a, t0, t0);
        assert_eq!(pipeline.segment_id, Some(0));
        let after_first_segment = pipeline.out.len();
        assert!(
            after_first_segment > 0,
            "2000 input frames should exceed one resample chunk"
        );

        // Device changed mid-session: new segment_id, new sample rate.
        let frame_b = AudioFrame {
            segment_id: 1,
            samples: vec![0.2; 2_000],
            format: mono_format(48_000),
        };
        let later = t0 + Duration::from_millis(500);
        pipeline.handle_frame(frame_b, later, t0);
        assert_eq!(pipeline.segment_id, Some(1));
        assert!(
            pipeline.out.len() > after_first_segment,
            "output should keep accumulating across the rebuild"
        );
    }

    #[test]
    fn apply_drift_inserts_silence_when_far_behind_real_time() {
        let mut pipeline = SourcePipeline::new(48_000);
        let t0 = Instant::now();
        // Simulate a source that has fallen far behind real time (e.g. a
        // device-change gap): 1000 samples written but 2s have elapsed.
        pipeline.out = vec![0.0; 1_000];
        let now = t0 + Duration::from_secs(2);
        pipeline.apply_drift(now, t0);
        assert!(
            pipeline.out.len() > 1_000,
            "a large deficit should be silence-filled, got {} samples",
            pipeline.out.len()
        );
    }

    #[test]
    fn combiner_produces_equal_length_aligned_channels_from_synthetic_streams() {
        let (mic_tx, mic_rx) = crossbeam_channel::bounded(64);
        let (system_tx, system_rx) = crossbeam_channel::bounded(64);

        for _ in 0..5 {
            mic_tx
                .send(CaptureItem::Frame(AudioFrame {
                    segment_id: 0,
                    samples: vec![0.1; 480],
                    format: mono_format(48_000),
                }))
                .unwrap();
        }
        for _ in 0..3 {
            // Stereo: 480 frames = 960 interleaved samples per send.
            system_tx
                .send(CaptureItem::Frame(AudioFrame {
                    segment_id: 0,
                    samples: vec![0.2; 960],
                    format: stereo_format(48_000),
                }))
                .unwrap();
        }
        drop(mic_tx);
        drop(system_tx);

        let combiner = Combiner::start(mic_rx, system_rx, 48_000).expect("combiner should start");
        let session = combiner.finish();

        assert_eq!(session.sample_rate(), 48_000);
        assert_eq!(
            session.channel(SessionChannel::Mic).len(),
            session.channel(SessionChannel::System).len()
        );
        assert_eq!(session.frames(), session.channel(SessionChannel::Mic).len());
        assert_eq!(
            session.duration(),
            Duration::from_secs_f64(session.frames() as f64 / 48_000.0)
        );
    }

    #[test]
    fn combiner_with_no_frames_produces_an_empty_session() {
        let (mic_tx, mic_rx) = crossbeam_channel::bounded::<CaptureItem>(1);
        let (system_tx, system_rx) = crossbeam_channel::bounded::<CaptureItem>(1);
        drop(mic_tx);
        drop(system_tx);

        let combiner = Combiner::start(mic_rx, system_rx, 48_000).expect("combiner should start");
        let session = combiner.finish();

        assert_eq!(session.frames(), 0);
        assert_eq!(session.duration(), Duration::from_secs(0));
    }
}
