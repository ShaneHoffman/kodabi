//! A streaming mono resampler wrapping rubato's asynchronous sinc
//! resampler. Cpal's callback buffer size varies (and differs between
//! capture sources), while rubato's `Async` resampler wants a fixed-size
//! input chunk, so this buffers arbitrary-sized `push`es into that chunk
//! size. It also trims the filter's startup delay from the front of the
//! output, so a fresh resampler's first returned sample lines up with its
//! first real input sample rather than a run of near-silence.

use audioadapter_buffers::direct::SequentialSlice;
use rubato::{
    Adjustable, Async, FixedAsync, Indexing, Resampler, SincInterpolationParameters, WindowFunction,
};

use crate::error::{AudioError, Result};
use crate::tuning::apply_positive_usize_override;

/// Default input frames per resample call. Independent of any capture
/// source's actual cpal buffer size — `push` queues arbitrary-sized input
/// and drains full chunks of this size into the resampler. Overridable via
/// `KODABI_RESAMPLE_CHUNK` (see [`ResampleParams::from_env`]).
const CHUNK_SIZE: usize = 1024;

/// Default sinc filter taps: a quality/CPU tradeoff for two live resamplers
/// running concurrently (the combiner's mic and system pipelines) — the
/// resource budget's main capture-path CPU lever (FOUNDING_DOC §3.7,
/// `docs/RESOURCE_BUDGET.md`). Overridable via `KODABI_RESAMPLE_TAPS`.
const SINC_TAPS: usize = 128;

/// Max relative ratio the resampler is built to tolerate. `DriftController`
/// only ever asks for up to ±2% (see `drift.rs`), so this leaves headroom.
const MAX_RELATIVE_RATIO: f64 = 1.05;

/// Runtime-tunable resample quality/chunk knobs — see [`ResampleParams::from_env`].
#[derive(Debug, Clone, Copy)]
pub struct ResampleParams {
    /// Input frames per resample call ([`CHUNK_SIZE`] by default).
    pub chunk: usize,
    /// Sinc filter taps ([`SINC_TAPS`] by default). More taps means higher
    /// quality and more CPU.
    pub taps: usize,
}

impl Default for ResampleParams {
    fn default() -> Self {
        ResampleParams {
            chunk: CHUNK_SIZE,
            taps: SINC_TAPS,
        }
    }
}

impl ResampleParams {
    /// [`ResampleParams::default`] with `KODABI_RESAMPLE_CHUNK` and
    /// `KODABI_RESAMPLE_TAPS` overrides applied, if set and valid — lets a
    /// resource-budget pass iterate on real hardware without recompiling.
    pub fn from_env() -> Self {
        let mut params = ResampleParams::default();
        params.chunk = apply_positive_usize_override(
            params.chunk,
            std::env::var("KODABI_RESAMPLE_CHUNK").ok(),
        );
        params.taps =
            apply_positive_usize_override(params.taps, std::env::var("KODABI_RESAMPLE_TAPS").ok());
        params
    }
}

/// A live, single-channel resampler from `src_rate` to `target_rate`, with a
/// runtime-adjustable ratio for clock-drift correction.
pub struct MonoResampler {
    inner: Async<f32>,
    pending: Vec<f32>,
    /// Output frames still to trim to compensate the sinc filter's startup
    /// delay. Counted down to zero as chunks are processed, then never
    /// consulted again — trimming only ever applies once, at stream start.
    delay_remaining: usize,
    scratch_out: Vec<f32>,
    /// Reused input-chunk buffer, so `push` doesn't heap-allocate a fresh
    /// `Vec` per 1024-frame chunk on the audio path.
    chunk_scratch: Vec<f32>,
}

impl MonoResampler {
    /// Build a resampler from `src_rate` to `target_rate` with the default
    /// [`ResampleParams`]. Fails only if rubato rejects the construction
    /// parameters (e.g. a zero rate).
    pub fn new(src_rate: u32, target_rate: u32) -> Result<Self> {
        Self::with_params(src_rate, target_rate, ResampleParams::default())
    }

    /// Build a resampler from `src_rate` to `target_rate`, with `params`
    /// controlling the resample chunk size and sinc filter quality (see
    /// [`ResampleParams`]). Blackman-Harris windowed, cubic-interpolated.
    /// Fails only if rubato rejects the construction parameters (e.g. a zero
    /// rate).
    pub fn with_params(src_rate: u32, target_rate: u32, params: ResampleParams) -> Result<Self> {
        let sinc_params =
            SincInterpolationParameters::new(params.taps, WindowFunction::BlackmanHarris2);
        let ratio = target_rate as f64 / src_rate as f64;
        let inner = Async::<f32>::new_sinc(
            ratio,
            MAX_RELATIVE_RATIO,
            &sinc_params,
            params.chunk,
            1,
            FixedAsync::Input,
        )
        .map_err(|e| AudioError::Resample(e.to_string()))?;
        let delay_remaining = inner.output_delay();
        let scratch_out = vec![0.0; inner.output_frames_max()];
        Ok(MonoResampler {
            inner,
            pending: Vec::new(),
            delay_remaining,
            scratch_out,
            chunk_scratch: Vec::new(),
        })
    }

    /// Queue `mono` input and return any newly resampled output. Input
    /// short of a full chunk is held until a later call (or `flush`)
    /// completes it.
    pub fn push(&mut self, mono: &[f32]) -> Vec<f32> {
        self.pending.extend_from_slice(mono);
        let chunk_size = self.inner.input_frames_next();
        let mut produced = Vec::new();
        // Take the reusable chunk buffer out so `process_chunk` can borrow
        // `&mut self` without aliasing it; put it back (capacity retained)
        // when done, so no per-chunk allocation happens on the audio path.
        let mut chunk = std::mem::take(&mut self.chunk_scratch);
        while self.pending.len() >= chunk_size {
            chunk.clear();
            chunk.extend(self.pending.drain(..chunk_size));
            self.process_chunk(&chunk, None, &mut produced);
        }
        self.chunk_scratch = chunk;
        produced
    }

    /// Nudge the resample ratio by a relative factor (`1.0` = no change);
    /// see [`DriftController::correction`](crate::drift::DriftController::correction).
    /// Ramps the change in smoothly over the next chunk rather than
    /// applying it as a discontinuity. Silently ignores an out-of-bounds
    /// request — the drift controller already clamps its corrections, so
    /// this is a defensive backstop, and a live session must keep running
    /// even if a correction were ever rejected.
    pub fn set_ratio_relative(&mut self, relative: f64) {
        let _ = self.inner.set_resample_ratio_relative(relative, true);
    }

    /// Process any samples queued but short of a full chunk (e.g. at
    /// session end, or before a segment rebuild swaps in a new resampler)
    /// and return the trailing output.
    pub fn flush(&mut self) -> Vec<f32> {
        let mut produced = Vec::new();
        if !self.pending.is_empty() {
            let partial_len = self.pending.len();
            let chunk = std::mem::take(&mut self.pending);
            self.process_chunk(&chunk, Some(partial_len), &mut produced);
        }
        produced
    }

    fn process_chunk(
        &mut self,
        chunk: &[f32],
        partial_len: Option<usize>,
        produced: &mut Vec<f32>,
    ) {
        let out_capacity = self.inner.output_frames_max();
        if self.scratch_out.len() < out_capacity {
            self.scratch_out.resize(out_capacity, 0.0);
        }
        let input = SequentialSlice::new(chunk, 1, chunk.len())
            .expect("chunk is sized to at least the frames it reports");
        let scratch_len = self.scratch_out.len();
        let mut output = SequentialSlice::new_mut(&mut self.scratch_out, 1, scratch_len)
            .expect("scratch_out is sized to at least output_frames_max");
        let indexing = partial_len.map(|n| Indexing::new().partial_len(n));
        let (_read, written) = self
            .inner
            .process_into_buffer(&input, &mut output, indexing.as_ref())
            .expect("input/output buffers are sized to what rubato requires");

        let mut frames = &self.scratch_out[..written];
        if self.delay_remaining > 0 {
            let trim = self.delay_remaining.min(frames.len());
            frames = &frames[trim..];
            self.delay_remaining -= trim;
        }
        produced.extend_from_slice(frames);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn sine(freq: f32, sample_rate: u32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| (2.0 * PI * freq * i as f32 / sample_rate as f32).sin())
            .collect()
    }

    fn resample_all(resampler: &mut MonoResampler, input: &[f32]) -> Vec<f32> {
        let mut out = resampler.push(input);
        out.extend(resampler.flush());
        out
    }

    fn zero_crossings(samples: &[f32]) -> usize {
        samples
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count()
    }

    #[test]
    fn identity_rate_roughly_preserves_sample_count() {
        let mut resampler = MonoResampler::new(48_000, 48_000).unwrap();
        let input = sine(220.0, 48_000, 5 * CHUNK_SIZE);
        let out = resample_all(&mut resampler, &input);
        let diff = (out.len() as i64 - input.len() as i64).abs();
        assert!(
            diff < 300,
            "expected output length close to input length, got {} vs {}",
            out.len(),
            input.len()
        );
    }

    #[test]
    fn upsampling_roughly_doubles_sample_count() {
        let mut resampler = MonoResampler::new(24_000, 48_000).unwrap();
        let input = sine(220.0, 24_000, 5 * CHUNK_SIZE);
        let out = resample_all(&mut resampler, &input);
        let expected = input.len() * 2;
        let diff = (out.len() as i64 - expected as i64).abs();
        assert!(
            diff < 300,
            "expected output length close to {}, got {}",
            expected,
            out.len()
        );
    }

    #[test]
    fn downsampling_roughly_halves_sample_count() {
        let mut resampler = MonoResampler::new(48_000, 24_000).unwrap();
        let input = sine(220.0, 48_000, 5 * CHUNK_SIZE);
        let out = resample_all(&mut resampler, &input);
        let expected = input.len() / 2;
        let diff = (out.len() as i64 - expected as i64).abs();
        assert!(
            diff < 300,
            "expected output length close to {}, got {}",
            expected,
            out.len()
        );
    }

    #[test]
    fn sine_period_is_preserved_across_resample() {
        let src_rate = 44_100;
        let target_rate = 48_000;
        let freq = 440.0;
        let mut resampler = MonoResampler::new(src_rate, target_rate).unwrap();
        let input = sine(freq, src_rate, 5 * CHUNK_SIZE);
        let out = resample_all(&mut resampler, &input);

        let duration_secs = input.len() as f32 / src_rate as f32;
        let expected_crossings = (2.0 * freq * duration_secs).round() as i64;
        let actual_crossings = zero_crossings(&out) as i64;
        assert!(
            (actual_crossings - expected_crossings).abs() <= 10,
            "expected ~{} zero crossings, got {}",
            expected_crossings,
            actual_crossings
        );
    }

    #[test]
    fn flush_of_short_input_does_not_panic() {
        let mut resampler = MonoResampler::new(48_000, 48_000).unwrap();
        // Well under one chunk.
        let input = vec![0.1_f32; 10];
        let out = resample_all(&mut resampler, &input);
        // The whole thing may be swallowed by startup-delay trimming; the
        // only real assertion is that it doesn't panic and stays bounded.
        assert!(out.len() <= CHUNK_SIZE);
    }

    #[test]
    fn set_ratio_relative_does_not_panic_and_keeps_producing_output() {
        let mut resampler = MonoResampler::new(48_000, 48_000).unwrap();
        resampler.set_ratio_relative(1.01);
        let input = sine(220.0, 48_000, 5 * CHUNK_SIZE);
        let out = resample_all(&mut resampler, &input);
        assert!(!out.is_empty());
    }
}
