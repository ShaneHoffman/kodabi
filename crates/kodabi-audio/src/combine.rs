//! Combines the mic and system-audio capture streams into a single
//! time-aligned two-channel session: channel 0 = mic = "you", channel 1 =
//! system = "them" (FOUNDING_DOC §3.3's "two-channel bonus").
//!
//! `Combiner` is the sole consumer of each `Capture::items()` stream (that
//! channel is MPMC — see `capture.rs` — so a second independent sink such as
//! persistence would need its own broadcast layer; nothing needs one yet).
//! A single coordinator thread drains both streams, downmixes each frame to
//! mono, resamples it to a common target rate, and appends it to that
//! source's timeline. Sources are handed to it one at a time via
//! [`Combiner::attach`] — the moment each `Capture` goes live — so the first
//! source starts draining immediately instead of letting its bounded item
//! channel back up (and drop real audio) while its sibling is still
//! negotiating. `finish()` closes the roster and blocks until every attached
//! `Capture` has stopped (their senders drop, disconnecting these receivers),
//! then returns the finalized [`AlignedSession`].

use std::path::PathBuf;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvError, Select, Sender};

use crate::drift::{DriftController, GapCorrection};
use crate::error::Result;
use crate::frame::{AudioFrame, CaptureItem};
use crate::mix::{downmix_to_mono_into, interleave_stereo};
use crate::resample::{MonoResampler, ResampleParams};
use crate::spill::{ChannelSpillWriter, SpillConfig, SpillWriters};

/// How often (of wall-clock time) a source's drift is re-evaluated. See
/// `drift.rs` for what the correction itself does.
const DRIFT_CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// Once a single source's accumulated audio reaches this many seconds without
/// its sibling ever starting, the shared origin `t0` is frozen so the lone
/// channel can begin spilling to disk (and stop growing in memory). A sibling
/// whose stream went live earlier can have its first frame dequeued only after
/// a bounded item-channel backlog — a few seconds at most — so 60 s is far
/// beyond any window in which `t0` could still legitimately be lowered.
const FORCE_T0_FINAL_SECS: u64 = 60;

/// One capture source's position in the aligned session: channel 0 = mic =
/// "you", channel 1 = system = "them" (`docs/MCP_TOOL_SURFACE.md`'s
/// `Channel` enum). Kept positional here — mapping to the wire-level
/// you/them label happens at the transcription/persistence boundary, so
/// `kodabi-audio` doesn't need a dependency on `kodabi-core` for it.
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

    /// Borrow one channel's mono samples resampled to `target_rate` — e.g.
    /// downsampling from this session's rate to the 16 kHz mono `f32` a
    /// transcription engine requires. Returns a plain copy of the channel,
    /// unresampled, when `target_rate` already matches.
    pub fn channel_resampled(&self, ch: SessionChannel, target_rate: u32) -> Result<Vec<f32>> {
        let source = self.channel(ch);
        if target_rate == self.sample_rate {
            return Ok(source.to_vec());
        }
        let mut resampler = MonoResampler::new(self.sample_rate, target_rate)?;
        let mut out = resampler.push(source);
        out.extend(resampler.flush());
        Ok(out)
    }
}

/// The finalized output of a spilling [`Combiner`]: both channels streamed to
/// their own raw PCM files during capture (48 kHz little-endian `f32`, the
/// same timeline [`AlignedSession`] would have held in memory), so nothing but
/// the tail was ever resident. `frames` is per channel — the files are padded
/// to equal length at finalize, matching `AlignedSession`'s contract. See
/// [`crate::SpillReader`] for reading them back.
#[derive(Clone, Debug)]
pub struct SpilledSession {
    pub sample_rate: u32,
    pub frames: u64,
    pub mic_path: PathBuf,
    pub system_path: PathBuf,
}

impl SpilledSession {
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.frames as f64 / self.sample_rate.max(1) as f64)
    }
}

/// What a [`Combiner`] finalizes to: the whole session in memory when no spill
/// was configured (tests, the `record_meeting` example, and the degraded
/// fallback when spill files can't be created), or paths to the per-channel
/// spill files when it streamed to disk.
#[derive(Clone, Debug)]
pub enum CombinedSession {
    InMemory(AlignedSession),
    Spilled(SpilledSession),
}

/// One source's downmix -> resample -> drift-correct -> accumulate pipeline.
/// Not `pub`: an internal building block of the coordinator loop, not part
/// of the combiner's public surface.
struct SourcePipeline {
    target_rate: u32,
    resample_params: ResampleParams,
    resampler: Option<MonoResampler>,
    /// The segment this pipeline's resampler was built for. `AudioFrame`
    /// carries its own `segment_id`, so a change here (rather than relying
    /// on the separately delivered, droppable `SegmentStarted` marker) is
    /// the authoritative signal that the format may have changed.
    segment_id: Option<u32>,
    drift: DriftController,
    out: Vec<f32>,
    /// This channel's spill file, when the session is streaming to disk.
    /// `None` keeps the whole timeline in `out` (the in-memory mode).
    spill: Option<ChannelSpillWriter>,
    /// Samples already drained from `out` to the spill file. The drift math
    /// and finalize padding reason about the *effective* length —
    /// `flushed + out.len()` — so a spilled prefix is accounted for exactly
    /// as if it were still resident.
    flushed: u64,
    /// One-shot guard so a spill I/O failure (e.g. a full disk) logs once and
    /// then degrades to in-memory growth rather than spamming per flush.
    spill_error_logged: bool,
    /// Whether this pipeline has processed its first frame yet — gates the
    /// one-time leading-silence padding that aligns it to the session
    /// origin `t0`.
    started: bool,
    last_drift_check: Option<Instant>,
    /// Reused mono downmix buffer, so `handle_frame` doesn't allocate a
    /// fresh `Vec` per captured frame.
    mono_scratch: Vec<f32>,
}

impl SourcePipeline {
    fn new(
        target_rate: u32,
        resample_params: ResampleParams,
        spill: Option<ChannelSpillWriter>,
    ) -> Self {
        SourcePipeline {
            target_rate,
            resample_params,
            resampler: None,
            segment_id: None,
            drift: DriftController::new(target_rate),
            out: Vec::new(),
            spill,
            flushed: 0,
            spill_error_logged: false,
            started: false,
            last_drift_check: None,
            mono_scratch: Vec::new(),
        }
    }

    /// Total samples this channel represents so far, whether still resident in
    /// `out` or already streamed to the spill file.
    fn effective_len(&self) -> u64 {
        self.flushed + self.out.len() as u64
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
        self.resampler =
            MonoResampler::with_params(sample_rate, self.target_rate, self.resample_params).ok();
    }

    /// Prepend `n` samples of leading silence, shifting all existing output
    /// later. Used when the shared session origin is lowered after this
    /// pipeline was already placed against a later one (see [`process_frame`]).
    ///
    /// Only ever called before `t0` is final, which is also before any samples
    /// have spilled, so `flushed` is always zero here: shifting `out` can never
    /// desync it from a prefix already committed to disk. The guard makes that
    /// invariant explicit and refuses (rather than silently corrupting the
    /// timeline) if it were ever violated.
    fn prepend_silence(&mut self, n: usize) {
        if n == 0 {
            return;
        }
        debug_assert_eq!(
            self.flushed, 0,
            "prepend_silence after a spill would desync the flushed prefix"
        );
        if self.flushed > 0 {
            return;
        }
        let mut shifted = vec![0.0; n];
        shifted.extend_from_slice(&self.out);
        self.out = shifted;
    }

    /// Process one captured frame: rebuild the resampler if the segment
    /// changed, align to the shared origin `t0` on the very first frame,
    /// re-evaluate drift, then downmix + resample + accumulate. All timing
    /// is keyed off the frame's own `capture_time` (stamped in the cpal
    /// callback), not the dequeue instant, so a burst-drained backlog lands
    /// on the real timeline.
    fn handle_frame(&mut self, frame: AudioFrame, t0: Instant) {
        let now = frame.capture_time;
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

        // Re-evaluate drift *before* appending this frame's audio, so a
        // one-off gap fill lands as leading silence ahead of the just-arrived
        // frame — placing it at the offset real time implies — rather than
        // splicing it in early and pushing the silence behind it.
        self.apply_drift(now, t0);

        self.mono_scratch.clear();
        downmix_to_mono_into(
            &frame.samples,
            frame.format.channels,
            &mut self.mono_scratch,
        );
        if let Some(resampler) = self.resampler.as_mut() {
            let resampled = resampler.push(&self.mono_scratch);
            self.out.extend(resampled);
        }
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
        // Reason about the whole timeline, not just what's still resident: a
        // spilled prefix (`flushed`) is as real as `out` for measuring how far
        // this source has fallen behind or run ahead of real time.
        let output_written = self.effective_len();

        if let Some(GapCorrection::InsertSilence(n)) =
            self.drift.gap_correction(elapsed, output_written)
        {
            // A deficit too large for the gentle nudge to close in reasonable
            // time (e.g. the silent window while a source rebuilds after a
            // device change): back-fill it in one shot as leading silence and
            // leave the resample ratio neutral. Applying the proportional
            // nudge on top of a full fill would double-correct the same error
            // and overshoot into a surplus.
            let new_len = self.out.len() + n;
            self.out.resize(new_len, 0.0);
            if let Some(resampler) = self.resampler.as_mut() {
                resampler.set_ratio_relative(1.0);
            }
            return;
        }

        // Otherwise the gentle proportional nudge is the whole correction. A
        // surplus (`TrimSamples`, running ahead of real time) is deliberately
        // *not* truncated — `out`'s tail is the most recently captured audio,
        // so trimming it would delete real samples and splice a discontinuity
        // — it's left for this nudge to claw back over the next checks. Steady
        // clock drift is tens of ppm, far inside the ±2% the ratio absorbs.
        let ratio = self.drift.correction(elapsed, output_written);
        if let Some(resampler) = self.resampler.as_mut() {
            resampler.set_ratio_relative(ratio);
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

    /// If spilling and `out` has reached the flush threshold, stream it to
    /// disk and clear it (retaining capacity, so no realloc churn). On an I/O
    /// error the samples stay in `out` and are retried at the next threshold
    /// crossing — a full disk degrades to the old in-memory growth rather than
    /// dropping audio or killing the session. Only safe to call once `t0` is
    /// final (no more `prepend_silence`); the coordinator gates on that.
    fn maybe_spill(&mut self, threshold: usize) {
        if self.out.len() >= threshold {
            self.drain_out();
        }
    }

    /// Drain whatever remains in `out` to the spill file at session end.
    /// A no-op (leaving `out` intact) when not spilling or on I/O error.
    fn final_spill(&mut self) {
        self.drain_out();
    }

    /// Stream the whole of `out` to the spill file and clear it (retaining
    /// capacity). A no-op — leaving `out` intact — when not spilling, when `out`
    /// is empty, or on I/O error (the caller degrades to in-memory growth).
    fn drain_out(&mut self) {
        if self.out.is_empty() {
            return;
        }
        let Some(writer) = self.spill.as_mut() else {
            return;
        };
        match writer.append(&self.out) {
            Ok(()) => {
                self.flushed += self.out.len() as u64;
                self.out.clear();
            }
            Err(err) => self.log_spill_error(err),
        }
    }

    /// Append `n` samples of trailing silence directly to the spill file, to
    /// pad the shorter channel up to the session's frame count at finalize
    /// (the on-disk equivalent of `AlignedSession`'s equal-length padding).
    ///
    /// Written in bounded chunks from a reused buffer rather than one
    /// `vec![0.0; n]`: when the two channels diverge sharply — a capture device
    /// that dropped mid-meeting while its sibling ran on for an hour, or a
    /// lone source's never-started sibling — `n` can be many minutes of audio,
    /// and a single allocation of that size would spike memory (defeating the
    /// bounded-memory guarantee) or OOM.
    fn pad_spill(&mut self, n: usize) {
        if n == 0 {
            return;
        }
        let Some(writer) = self.spill.as_mut() else {
            return;
        };
        // One second of silence per write at most, reused across the loop, so a
        // large pad never allocates more than this.
        const PAD_CHUNK: usize = 48_000;
        let silence = vec![0.0; n.min(PAD_CHUNK)];
        let mut remaining = n;
        while remaining > 0 {
            let take = remaining.min(silence.len());
            if let Err(err) = writer.append(&silence[..take]) {
                self.log_spill_error(err);
                return;
            }
            self.flushed += take as u64;
            remaining -= take;
        }
    }

    fn log_spill_error(&mut self, err: std::io::Error) {
        if !self.spill_error_logged {
            self.spill_error_logged = true;
            eprintln!(
                "kodabi-audio: spill write failed ({err}); keeping audio in memory \
                 for the rest of this session"
            );
        }
    }
}

/// Route one drained item into `this` pipeline, maintaining the shared
/// session origin `t0` as the *earliest* `capture_time` seen across both
/// sources — not merely the first frame this loop happened to dequeue.
///
/// `Select` gives no ordering guarantee between the two receivers, so a
/// source whose stream went live earlier can still have its first frame
/// dequeued second. When that happens (`now < origin`), the origin is lowered
/// to it and the shortfall is prepended as leading silence to `other`, which
/// was already placed against the higher origin — keeping both channels on a
/// common sample zero. A subsequent frame can never predate `t0` (frames are
/// FIFO per source, so a source's own first frame is its earliest), so `this`
/// here is always unstarted and needs no back-fill of its own.
fn process_frame(
    this: &mut SourcePipeline,
    other: &mut SourcePipeline,
    item: CaptureItem,
    t0: &mut Option<Instant>,
    target_rate: u32,
    t0_final: bool,
) {
    // Frames are self-describing (`frame.format`, `frame.segment_id`), so a
    // dropped `SegmentStarted` marker loses nothing — this coordinator only
    // reacts to segment/format changes it observes on the frames themselves.
    let CaptureItem::Frame(frame) = item else {
        return;
    };
    let now = frame.capture_time;
    let origin = match *t0 {
        // Once `t0` is final, spilling may already have committed a prefix of
        // `other` to disk, so lowering the origin (and the `prepend_silence`
        // back-fill it triggers) is no longer safe. This only fires on the
        // lone-source force-final fallback — with both sources started, `t0`
        // can never legitimately be lowered anyway — so clamp the stray early
        // frame to the frozen origin rather than corrupt the timeline.
        Some(origin) if now < origin && t0_final => origin,
        Some(origin) if now < origin => {
            let delta = origin.saturating_duration_since(now);
            let delta_samples = (delta.as_secs_f64() * target_rate as f64).round() as usize;
            other.prepend_silence(delta_samples);
            *t0 = Some(now);
            now
        }
        Some(origin) => origin,
        None => {
            *t0 = Some(now);
            now
        }
    };
    this.handle_frame(frame, origin);
}

fn finalize(
    mut mic: SourcePipeline,
    mut system: SourcePipeline,
    target_rate: u32,
) -> CombinedSession {
    mic.flush();
    system.flush();

    // A session is spilling iff its writers were created (both or neither).
    if mic.spill.is_some() {
        mic.final_spill();
        system.final_spill();

        // Pad the shorter channel's file to the longer's length, matching
        // `AlignedSession`'s equal-length contract. Best-effort: if a pad
        // write fails the files may end unequal, which downstream tolerates
        // (each channel is transcribed independently).
        let frames = mic.effective_len().max(system.effective_len());
        mic.pad_spill((frames - mic.effective_len()) as usize);
        system.pad_spill((frames - system.effective_len()) as usize);

        let mic_writer = mic.spill.expect("mic spill present in spilling session");
        let system_writer = system
            .spill
            .expect("system spill present in spilling session");
        return CombinedSession::Spilled(SpilledSession {
            sample_rate: target_rate,
            frames,
            mic_path: mic_writer.into_path(),
            system_path: system_writer.into_path(),
        });
    }

    let frames = mic.out.len().max(system.out.len());
    mic.out.resize(frames, 0.0);
    system.out.resize(frames, 0.0);

    CombinedSession::InMemory(AlignedSession {
        sample_rate: target_rate,
        mic: mic.out,
        system: system.out,
    })
}

/// A source's channel label plus its item-stream receiver, handed to the
/// running coordinator so a source can be attached after the combiner has
/// already started draining its sibling.
type SourceAttach = (SessionChannel, Receiver<CaptureItem>);

/// The ready operation a coordinator-loop `select` resolved to, lifted out of
/// the `select` borrow scope so the receivers it borrowed can be reassigned
/// before the operation is applied.
enum Action {
    Control(std::result::Result<SourceAttach, RecvError>),
    Mic(std::result::Result<CaptureItem, RecvError>),
    System(std::result::Result<CaptureItem, RecvError>),
}

/// Drains attached capture item streams until every stream disconnects (i.e.
/// until each `Capture` is stopped) and no more sources can attach, aligning
/// and resampling as it goes.
///
/// Sources arrive over `control_rx` rather than being passed up front: the
/// combiner starts draining the first source the instant it goes live, so a
/// slow-negotiating sibling never backs up that source's bounded item channel
/// (which would drop real audio). A source arriving mid-flight is simply added
/// to the select set; [`Combiner::finish`] drops the control sender to signal
/// that the roster is closed.
fn coordinator_loop(
    control_rx: Receiver<SourceAttach>,
    target_rate: u32,
    resample_params: ResampleParams,
    spill: Option<SpillConfig>,
) -> CombinedSession {
    // Create both channels' spill writers up front so a path/permission
    // problem surfaces immediately; a creation failure degrades the whole
    // session to in-memory (both writers `None`) rather than spilling one
    // channel and losing the other.
    let (mic_spill, system_spill, spill_threshold) = match spill {
        Some(config) => match SpillWriters::create(&config) {
            Ok(writers) => (Some(writers.mic), Some(writers.system), writers.threshold),
            Err(err) => {
                eprintln!(
                    "kodabi-audio: could not create spill files ({err}); \
                     capturing to memory for this session"
                );
                (None, None, 0)
            }
        },
        None => (None, None, 0),
    };
    let spilling = mic_spill.is_some();
    let force_t0_final_samples = FORCE_T0_FINAL_SECS * target_rate as u64;

    let mut mic_pipeline = SourcePipeline::new(target_rate, resample_params, mic_spill);
    let mut system_pipeline = SourcePipeline::new(target_rate, resample_params, system_spill);
    let mut t0: Option<Instant> = None;
    // Whether the shared origin is frozen. Until then a later-dequeued earlier
    // frame may still lower it (back-filling the sibling), so spilling — which
    // commits a prefix to disk — must wait. Set once both sources have started,
    // or, for a lone source that never gets a sibling, after a bounded amount
    // of its audio has accumulated (see [`FORCE_T0_FINAL_SECS`]).
    let mut t0_final = false;

    let mut mic_rx: Option<Receiver<CaptureItem>> = None;
    let mut system_rx: Option<Receiver<CaptureItem>> = None;
    let mut control_open = true;

    // Keep going while a source could still attach (control open) or an
    // attached source is still delivering. Once the roster is closed and both
    // source channels have disconnected there is nothing left to select on.
    while control_open || mic_rx.is_some() || system_rx.is_some() {
        let mut select = Select::new();
        let control_index = control_open.then(|| select.recv(&control_rx));
        let mic_index = mic_rx.as_ref().map(|rx| select.recv(rx));
        let system_index = system_rx.as_ref().map(|rx| select.recv(rx));

        // Resolve the ready operation into an owned `Action`, then drop
        // `select` so its borrows of the receivers end before the match below
        // reassigns them.
        let oper = select.select();
        let index = oper.index();
        let action = if Some(index) == control_index {
            Action::Control(oper.recv(&control_rx))
        } else if Some(index) == mic_index {
            Action::Mic(oper.recv(mic_rx.as_ref().expect("mic op registered only when Some")))
        } else if Some(index) == system_index {
            Action::System(
                oper.recv(
                    system_rx
                        .as_ref()
                        .expect("system op registered only when Some"),
                ),
            )
        } else {
            unreachable!("select returned an index that was never registered")
        };
        drop(select);

        match action {
            Action::Control(Ok((SessionChannel::Mic, rx))) => mic_rx = Some(rx),
            Action::Control(Ok((SessionChannel::System, rx))) => system_rx = Some(rx),
            // `finish()` dropped the control sender: no more sources attach.
            Action::Control(Err(_)) => control_open = false,
            Action::Mic(Ok(item)) => {
                process_frame(
                    &mut mic_pipeline,
                    &mut system_pipeline,
                    item,
                    &mut t0,
                    target_rate,
                    t0_final,
                );
                post_frame_spill(
                    spilling,
                    &mut t0_final,
                    &mut mic_pipeline,
                    &mut system_pipeline,
                    force_t0_final_samples,
                    spill_threshold,
                );
            }
            // This source's `Capture` stopped and dropped its sender.
            Action::Mic(Err(_)) => mic_rx = None,
            Action::System(Ok(item)) => {
                process_frame(
                    &mut system_pipeline,
                    &mut mic_pipeline,
                    item,
                    &mut t0,
                    target_rate,
                    t0_final,
                );
                post_frame_spill(
                    spilling,
                    &mut t0_final,
                    &mut mic_pipeline,
                    &mut system_pipeline,
                    force_t0_final_samples,
                    spill_threshold,
                );
            }
            Action::System(Err(_)) => system_rx = None,
        }
    }

    finalize(mic_pipeline, system_pipeline, target_rate)
}

/// After a frame is routed, advance the shared-origin latch and — once it is
/// frozen — flush either channel that has crossed the spill threshold. A no-op
/// when the session isn't spilling. Extracted so the mic and system arms of the
/// coordinator loop can't drift apart. See [`update_t0_final`] for the latch.
fn post_frame_spill(
    spilling: bool,
    t0_final: &mut bool,
    mic: &mut SourcePipeline,
    system: &mut SourcePipeline,
    force_samples: u64,
    threshold: usize,
) {
    if !spilling {
        return;
    }
    update_t0_final(t0_final, mic, system, force_samples);
    if *t0_final {
        mic.maybe_spill(threshold);
        system.maybe_spill(threshold);
    }
}

/// Freeze the shared origin once it can no longer legitimately move: both
/// sources have started (so no unstarted sibling can still lower `t0`), or a
/// lone source has accumulated enough audio that a sibling's first frame could
/// no longer predate the origin (see [`FORCE_T0_FINAL_SECS`]). Latches — once
/// true it stays true.
fn update_t0_final(
    t0_final: &mut bool,
    mic: &SourcePipeline,
    system: &SourcePipeline,
    force_samples: u64,
) {
    if *t0_final {
        return;
    }
    if mic.started && system.started {
        *t0_final = true;
        return;
    }
    if mic.effective_len() >= force_samples || system.effective_len() >= force_samples {
        *t0_final = true;
    }
}

/// Combines a mic and a system-audio capture stream into one time-aligned
/// two-channel [`AlignedSession`]. Owns one `items()` receiver from each
/// `Capture` (handed over via [`Combiner::attach`]) and is the sole consumer
/// of both.
pub struct Combiner {
    handle: JoinHandle<CombinedSession>,
    /// Delivers a source's receiver to the running coordinator. Dropped by
    /// [`Combiner::finish`] to close the roster so the coordinator terminates.
    control_tx: Sender<SourceAttach>,
}

impl Combiner {
    /// Spawn the coordinator thread. It starts with no sources attached —
    /// each is added by [`Combiner::attach`] the moment it goes live, so the
    /// first source begins draining immediately instead of buffering until
    /// its sibling finishes negotiating. `resample_params` tunes the live
    /// per-source resamplers' chunk size and sinc quality (see
    /// [`ResampleParams`]). When `spill` is `Some`, each channel is streamed to
    /// its own file during capture so nothing but the tail stays resident and a
    /// crash loses at most the last flush interval; `None` keeps the whole
    /// session in memory (tests and the `record_meeting` example).
    pub fn start(
        target_rate: u32,
        resample_params: ResampleParams,
        spill: Option<SpillConfig>,
    ) -> Result<Combiner> {
        let (control_tx, control_rx) = crossbeam_channel::unbounded();
        let handle = thread::Builder::new()
            .name("kodabi-audio-combiner".to_string())
            .spawn(move || coordinator_loop(control_rx, target_rate, resample_params, spill))
            .map_err(|_| crate::error::AudioError::ThreadStart)?;
        Ok(Combiner { handle, control_tx })
    }

    /// Attach one source's item stream, labelled with the channel it feeds.
    /// The coordinator adds it to its select set on the next iteration and
    /// begins draining it. Returns `true` when the stream was handed off, and
    /// `false` if the coordinator has already finished (the source is then
    /// simply not combined) — so a caller tracking which channels attached
    /// records only confirmed hand-offs. In practice `attach` only ever runs
    /// between `start` and `finish`, so it returns `true`.
    #[must_use]
    pub fn attach(&self, channel: SessionChannel, items: Receiver<CaptureItem>) -> bool {
        self.control_tx.send((channel, items)).is_ok()
    }

    /// Close the roster and block until every attached stream disconnects
    /// (i.e. after each `Capture` is stopped), then return the finalized
    /// session — in memory, or as spill-file paths when a spill was configured.
    pub fn finish(self) -> CombinedSession {
        let Combiner { handle, control_tx } = self;
        // Drop the control sender first so the coordinator learns the roster
        // is closed; otherwise it would keep selecting on `control_rx` and
        // never terminate.
        drop(control_tx);
        handle.join().expect("combiner coordinator thread panicked")
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

    /// Unwrap the in-memory session the non-spilling combiner tests expect.
    fn expect_in_memory(session: CombinedSession) -> AlignedSession {
        match session {
            CombinedSession::InMemory(session) => session,
            CombinedSession::Spilled(_) => {
                panic!("expected an in-memory session, got a spilled one")
            }
        }
    }

    #[test]
    fn first_arriving_pipeline_gets_no_leading_silence() {
        let mut pipeline = SourcePipeline::new(48_000, ResampleParams::default(), None);
        let t0 = Instant::now();
        let frame = AudioFrame {
            segment_id: 0,
            samples: vec![0.5; 480],
            format: mono_format(48_000),
            capture_time: t0,
        };
        pipeline.handle_frame(frame, t0);
        // 480 samples is short of the resampler's 1024-frame chunk, so
        // nothing has been produced yet — only the (zero-length) origin
        // padding applies.
        assert_eq!(pipeline.out.len(), 0);
    }

    #[test]
    fn later_arriving_pipeline_gets_leading_silence_padding() {
        let mut pipeline = SourcePipeline::new(48_000, ResampleParams::default(), None);
        let t0 = Instant::now();
        let arrival = t0 + Duration::from_millis(50);
        let frame = AudioFrame {
            segment_id: 0,
            samples: vec![0.5; 480],
            format: mono_format(48_000),
            capture_time: arrival,
        };
        pipeline.handle_frame(frame, t0);
        // 50ms at 48kHz = 2400 samples of leading silence; the frame itself
        // produces no output yet (short of a full resample chunk).
        assert_eq!(pipeline.out.len(), 2_400);
        assert!(pipeline.out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn origin_is_lowered_and_other_channel_backfilled_when_an_earlier_frame_arrives_second() {
        let mut mic = SourcePipeline::new(48_000, ResampleParams::default(), None);
        let mut system = SourcePipeline::new(48_000, ResampleParams::default(), None);
        let mut t0: Option<Instant> = None;
        let base = Instant::now();

        // Mic's first frame is dequeued first, at +30ms: it provisionally
        // anchors the session origin.
        let mic_frame = AudioFrame {
            segment_id: 0,
            samples: vec![0.0; 480],
            format: mono_format(48_000),
            capture_time: base + Duration::from_millis(30),
        };
        process_frame(
            &mut mic,
            &mut system,
            CaptureItem::Frame(mic_frame),
            &mut t0,
            48_000,
            false,
        );
        assert_eq!(t0, Some(base + Duration::from_millis(30)));
        assert_eq!(
            mic.out.len(),
            0,
            "the origin source gets no leading silence"
        );

        // System's stream actually went live earlier; its first frame's
        // capture_time predates the provisional origin (Select just dequeued
        // it second). The origin must drop to it, and the mic channel —
        // already placed against +30ms — must be back-filled with 30ms of
        // leading silence so both channels share sample zero.
        let system_frame = AudioFrame {
            segment_id: 0,
            samples: vec![0.0; 480],
            format: mono_format(48_000),
            capture_time: base,
        };
        process_frame(
            &mut system,
            &mut mic,
            CaptureItem::Frame(system_frame),
            &mut t0,
            48_000,
            false,
        );
        assert_eq!(t0, Some(base));
        assert_eq!(mic.out.len(), 1_440, "mic back-filled with 30ms @ 48kHz");
        assert!(mic.out.iter().all(|&s| s == 0.0));
        assert_eq!(
            system.out.len(),
            0,
            "system is now the origin — it gets no leading silence"
        );
    }

    #[test]
    fn segment_change_to_a_new_rate_rebuilds_the_resampler_and_keeps_output() {
        let mut pipeline = SourcePipeline::new(48_000, ResampleParams::default(), None);
        let t0 = Instant::now();

        let frame_a = AudioFrame {
            segment_id: 0,
            samples: vec![0.1; 2_000],
            format: mono_format(44_100),
            capture_time: t0,
        };
        pipeline.handle_frame(frame_a, t0);
        assert_eq!(pipeline.segment_id, Some(0));
        let after_first_segment = pipeline.out.len();
        assert!(
            after_first_segment > 0,
            "2000 input frames should exceed one resample chunk"
        );

        // Device changed mid-session: new segment_id, new sample rate.
        let later = t0 + Duration::from_millis(500);
        let frame_b = AudioFrame {
            segment_id: 1,
            samples: vec![0.2; 2_000],
            format: mono_format(48_000),
            capture_time: later,
        };
        pipeline.handle_frame(frame_b, t0);
        assert_eq!(pipeline.segment_id, Some(1));
        assert!(
            pipeline.out.len() > after_first_segment,
            "output should keep accumulating across the rebuild"
        );
    }

    #[test]
    fn apply_drift_inserts_silence_when_far_behind_real_time() {
        let mut pipeline = SourcePipeline::new(48_000, ResampleParams::default(), None);
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

        let capture_time = Instant::now();
        for _ in 0..5 {
            mic_tx
                .send(CaptureItem::Frame(AudioFrame {
                    segment_id: 0,
                    samples: vec![0.1; 480],
                    format: mono_format(48_000),
                    capture_time,
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
                    capture_time,
                }))
                .unwrap();
        }
        drop(mic_tx);
        drop(system_tx);

        let combiner = Combiner::start(48_000, ResampleParams::default(), None)
            .expect("combiner should start");
        assert!(combiner.attach(SessionChannel::Mic, mic_rx));
        assert!(combiner.attach(SessionChannel::System, system_rx));
        let session = expect_in_memory(combiner.finish());

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
    fn combiner_drains_a_source_attached_before_its_sibling() {
        // Mic goes live first and streams a burst while system is still
        // "negotiating"; system attaches only afterwards. The mic audio
        // captured during that window must survive into the aligned session —
        // this is the regression the incremental-attach design fixes.
        let combiner = Combiner::start(48_000, ResampleParams::default(), None)
            .expect("combiner should start");

        let capture_time = Instant::now();
        let (mic_tx, mic_rx) = crossbeam_channel::bounded(64);
        assert!(combiner.attach(SessionChannel::Mic, mic_rx));
        for _ in 0..5 {
            mic_tx
                .send(CaptureItem::Frame(AudioFrame {
                    segment_id: 0,
                    samples: vec![0.3; 480],
                    format: mono_format(48_000),
                    capture_time,
                }))
                .unwrap();
        }

        let (system_tx, system_rx) = crossbeam_channel::bounded(64);
        assert!(combiner.attach(SessionChannel::System, system_rx));
        for _ in 0..3 {
            system_tx
                .send(CaptureItem::Frame(AudioFrame {
                    segment_id: 0,
                    samples: vec![0.2; 960],
                    format: stereo_format(48_000),
                    capture_time,
                }))
                .unwrap();
        }

        drop(mic_tx);
        drop(system_tx);
        let session = expect_in_memory(combiner.finish());

        assert!(session.frames() > 0, "no aligned audio was produced");
        assert_eq!(
            session.channel(SessionChannel::Mic).len(),
            session.channel(SessionChannel::System).len()
        );
        assert!(
            session
                .channel(SessionChannel::Mic)
                .iter()
                .any(|&s| s != 0.0),
            "the mic burst captured before system attached was lost"
        );
    }

    #[test]
    fn channel_resampled_downsamples_48k_to_16k() {
        let session = AlignedSession {
            sample_rate: 48_000,
            mic: vec![0.1; 48_000],
            system: vec![0.2; 48_000],
        };

        let resampled = session
            .channel_resampled(SessionChannel::Mic, 16_000)
            .expect("resample should succeed");

        let diff = (resampled.len() as i64 - 16_000).abs();
        assert!(
            diff < 300,
            "expected roughly a third of the input length, got {}",
            resampled.len()
        );
    }

    #[test]
    fn channel_resampled_is_a_no_op_copy_at_the_same_rate() {
        let session = AlignedSession {
            sample_rate: 48_000,
            mic: vec![0.3; 480],
            system: vec![0.0; 480],
        };

        let resampled = session
            .channel_resampled(SessionChannel::Mic, 48_000)
            .expect("resample should succeed");

        assert_eq!(resampled, session.mic);
    }

    #[test]
    fn combiner_with_no_frames_produces_an_empty_session() {
        let (mic_tx, mic_rx) = crossbeam_channel::bounded::<CaptureItem>(1);
        let (system_tx, system_rx) = crossbeam_channel::bounded::<CaptureItem>(1);
        drop(mic_tx);
        drop(system_tx);

        let combiner = Combiner::start(48_000, ResampleParams::default(), None)
            .expect("combiner should start");
        assert!(combiner.attach(SessionChannel::Mic, mic_rx));
        assert!(combiner.attach(SessionChannel::System, system_rx));
        let session = expect_in_memory(combiner.finish());

        assert_eq!(session.frames(), 0);
        assert_eq!(session.duration(), Duration::from_secs(0));
    }

    // --- Spill / durability ---

    fn read_f32_file(path: &std::path::Path) -> Vec<f32> {
        let bytes = std::fs::read(path).expect("read spill file");
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect()
    }

    /// Feed identical mono streams to both channels through a combiner and
    /// return its finalized session. `mic_frames`/`system_frames` bound how
    /// many 480-sample frames each channel gets (equal by default).
    fn feed_and_finish(
        spill: Option<SpillConfig>,
        mic_frames: usize,
        system_frames: usize,
    ) -> CombinedSession {
        let (mic_tx, mic_rx) = crossbeam_channel::bounded(4096);
        let (system_tx, system_rx) = crossbeam_channel::bounded(4096);
        let capture_time = Instant::now();
        for _ in 0..mic_frames {
            mic_tx
                .send(CaptureItem::Frame(AudioFrame {
                    segment_id: 0,
                    samples: vec![0.25; 480],
                    format: mono_format(48_000),
                    capture_time,
                }))
                .unwrap();
        }
        for _ in 0..system_frames {
            system_tx
                .send(CaptureItem::Frame(AudioFrame {
                    segment_id: 0,
                    samples: vec![-0.5; 480],
                    format: mono_format(48_000),
                    capture_time,
                }))
                .unwrap();
        }
        drop(mic_tx);
        drop(system_tx);

        let combiner = Combiner::start(48_000, ResampleParams::default(), spill)
            .expect("combiner should start");
        assert!(combiner.attach(SessionChannel::Mic, mic_rx));
        assert!(combiner.attach(SessionChannel::System, system_rx));
        combiner.finish()
    }

    #[test]
    fn spilled_channels_match_the_in_memory_session_byte_for_byte() {
        // The spilled files, read back, must reproduce exactly the samples the
        // in-memory session would have held — the whole point of unifying the
        // two paths on one timeline.
        let in_memory = expect_in_memory(feed_and_finish(None, 300, 300));

        let dir = tempfile::tempdir().unwrap();
        let config = SpillConfig {
            mic_path: dir.path().join("mic.f32le"),
            system_path: dir.path().join("system.f32le"),
            flush_threshold_samples: 4096,
        };
        let spilled = match feed_and_finish(Some(config), 300, 300) {
            CombinedSession::Spilled(spilled) => spilled,
            CombinedSession::InMemory(_) => panic!("expected a spilled session"),
        };

        assert_eq!(spilled.sample_rate, 48_000);
        assert_eq!(spilled.frames as usize, in_memory.frames());
        assert_eq!(
            read_f32_file(&spilled.mic_path),
            in_memory.channel(SessionChannel::Mic)
        );
        assert_eq!(
            read_f32_file(&spilled.system_path),
            in_memory.channel(SessionChannel::System)
        );
    }

    #[test]
    fn finalize_pads_the_shorter_channel_file_to_equal_length() {
        let dir = tempfile::tempdir().unwrap();
        let config = SpillConfig {
            mic_path: dir.path().join("mic.f32le"),
            system_path: dir.path().join("system.f32le"),
            flush_threshold_samples: 4096,
        };
        // Mic gets more audio than system; the shorter file must be padded up.
        let spilled = match feed_and_finish(Some(config), 60, 20) {
            CombinedSession::Spilled(spilled) => spilled,
            CombinedSession::InMemory(_) => panic!("expected a spilled session"),
        };

        let mic = read_f32_file(&spilled.mic_path);
        let system = read_f32_file(&spilled.system_path);
        assert_eq!(mic.len(), system.len(), "both files padded to equal length");
        assert_eq!(mic.len(), spilled.frames as usize);
        // The system file's tail is the zero padding.
        assert!(system[system.len() - 1] == 0.0);
    }

    #[test]
    fn spill_creation_failure_falls_back_to_in_memory() {
        // An unwritable spill path must not fail the session: the combiner
        // degrades to in-memory rather than losing the capture.
        let config = SpillConfig {
            mic_path: PathBuf::from("Z:/kodabi-nonexistent-drive/mic.f32le"),
            system_path: PathBuf::from("Z:/kodabi-nonexistent-drive/system.f32le"),
            flush_threshold_samples: 4096,
        };
        let session = feed_and_finish(Some(config), 10, 10);
        assert!(matches!(session, CombinedSession::InMemory(_)));
    }

    #[test]
    fn spilling_keeps_the_in_memory_buffer_bounded() {
        // The bounded-memory guarantee: no matter how long the capture runs,
        // resident audio never exceeds roughly one flush threshold. Drive a
        // single pipeline directly with ~20s of frames and watch `out`.
        let dir = tempfile::tempdir().unwrap();
        let writer = ChannelSpillWriter::create(&dir.path().join("mic.f32le")).unwrap();
        let mut pipeline = SourcePipeline::new(48_000, ResampleParams::default(), Some(writer));
        let threshold = 4096;
        let t0 = Instant::now();
        let mut max_out = 0;
        for i in 0..2_000 {
            let frame = AudioFrame {
                segment_id: 0,
                samples: vec![0.1; 480],
                format: mono_format(48_000),
                capture_time: t0 + Duration::from_millis(10 * i),
            };
            pipeline.handle_frame(frame, t0);
            pipeline.maybe_spill(threshold);
            max_out = max_out.max(pipeline.out.len());
        }
        // One resample chunk of slack above the threshold; nowhere near the
        // ~960k samples an unbounded buffer would have grown to.
        assert!(
            max_out <= threshold + 1024,
            "resident buffer grew to {max_out}, expected <= {}",
            threshold + 1024
        );
    }

    #[test]
    fn apply_drift_accounts_for_already_spilled_samples() {
        // With a spilled prefix, the drift math must measure the whole
        // timeline (`flushed + out`), not just what's still resident — else a
        // spilled-away deficit would be mis-corrected.
        let mut pipeline = SourcePipeline::new(48_000, ResampleParams::default(), None);
        let t0 = Instant::now();
        // 1000 samples already spilled, 500 resident, but 2s have elapsed —
        // a large deficit that must be silence-filled into `out`.
        pipeline.flushed = 1_000;
        pipeline.out = vec![0.0; 500];
        let now = t0 + Duration::from_secs(2);
        pipeline.apply_drift(now, t0);
        assert!(
            pipeline.out.len() > 500,
            "the deficit should be silence-filled into the resident buffer"
        );
        assert_eq!(pipeline.flushed, 1_000, "spilled prefix is never rewound");
    }

    #[test]
    fn a_stray_earlier_frame_is_clamped_once_t0_is_final() {
        // After `t0` is frozen, a late-dequeued earlier frame must not lower
        // the origin or back-fill the sibling (which may already have spilled);
        // it is clamped to the frozen origin instead.
        let mut this = SourcePipeline::new(48_000, ResampleParams::default(), None);
        let mut other = SourcePipeline::new(48_000, ResampleParams::default(), None);
        let base = Instant::now();
        let mut t0 = Some(base + Duration::from_millis(30));

        let earlier = AudioFrame {
            segment_id: 0,
            samples: vec![0.0; 480],
            format: mono_format(48_000),
            capture_time: base,
        };
        process_frame(
            &mut this,
            &mut other,
            CaptureItem::Frame(earlier),
            &mut t0,
            48_000,
            true,
        );

        assert_eq!(
            t0,
            Some(base + Duration::from_millis(30)),
            "the frozen origin must not be lowered"
        );
        assert_eq!(other.out.len(), 0, "the sibling must not be back-filled");
    }
}
