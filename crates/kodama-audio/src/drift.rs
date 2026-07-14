//! Pure clock-drift correction math for the two-channel combiner. Each
//! capture source has its own hardware clock, and those clocks are separate
//! crystals that drift apart by tens of parts-per-million — enough to
//! desync a long meeting's channels by hundreds of milliseconds if left
//! uncorrected. This module has no notion of threads, resamplers, or audio
//! devices; it only turns "how many output samples have we written vs. how
//! many real time implies we should have" into a correction, so it's
//! testable without any of that (same split as `convert.rs`/`mix.rs`).

use std::time::Duration;

/// A one-off correction for a gap too large for the gentle ratio nudge in
/// [`DriftController::correction`] to close in reasonable time — e.g. the
/// silent window while a source's stream rebuilds after a device change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GapCorrection {
    /// The source has fallen behind real time by this many samples; insert
    /// that much silence to catch the timeline back up.
    InsertSilence(usize),
    /// The source has run ahead of real time by this many samples; drop
    /// that many samples to bring it back in line.
    TrimSamples(usize),
}

/// Compares a source's accumulated output against what its elapsed
/// wall-clock time implies it should have produced, and turns the
/// difference into corrections. Crude but useful for v1: this closes
/// steady-state clock drift and coarse gaps (e.g. a device-change rebuild)
/// to within about one cpal buffer, not sub-sample alignment.
#[derive(Clone, Copy, Debug)]
pub struct DriftController {
    target_rate: u32,
    /// Proportional gain: fraction of ratio nudge applied per second of
    /// accumulated error. Kept small so correction is inaudible.
    kp: f64,
    /// Maximum relative ratio nudge from [`correction`](Self::correction),
    /// as a fraction (e.g. `0.02` = ±2%). Must stay within the resampler's
    /// own adjustable range (see `resample.rs`).
    max_rel: f64,
    /// Error magnitude, in samples, past which [`gap_correction`](Self::gap_correction)
    /// stops relying on the gentle nudge and reports a one-off fill/trim.
    hard_gap_samples: usize,
}

impl DriftController {
    /// A controller for a source resampled to `target_rate`, with sensible
    /// v1 defaults: a 5%-of-error-per-second proportional gain, clamped to
    /// a ±2% ratio nudge, and a 50ms hard-gap threshold.
    pub fn new(target_rate: u32) -> Self {
        DriftController {
            target_rate,
            kp: 0.05,
            max_rel: 0.02,
            hard_gap_samples: (target_rate / 20).max(1) as usize,
        }
    }

    fn expected_samples(&self, elapsed: Duration) -> f64 {
        elapsed.as_secs_f64() * self.target_rate as f64
    }

    /// A relative ratio nudge (centered on `1.0`) to feed a resampler's
    /// `set_resample_ratio_relative`, computed from how far `output_written`
    /// has drifted from what `elapsed` real time implies. Positive error
    /// (ahead of real time) slows future output down (`< 1.0`); negative
    /// error (behind) speeds it up (`> 1.0`). Always within
    /// `1.0 ± max_rel`.
    pub fn correction(&self, elapsed: Duration, output_written: u64) -> f64 {
        let error = output_written as f64 - self.expected_samples(elapsed);
        let error_seconds = error / self.target_rate as f64;
        let raw = 1.0 - self.kp * error_seconds;
        raw.clamp(1.0 - self.max_rel, 1.0 + self.max_rel)
    }

    /// A one-off fill/trim for an error too large for [`correction`](Self::correction)
    /// to close gracefully (e.g. the silent window during a device-change
    /// rebuild), or `None` if the error is within the hard-gap threshold.
    pub fn gap_correction(&self, elapsed: Duration, output_written: u64) -> Option<GapCorrection> {
        let error = output_written as f64 - self.expected_samples(elapsed);
        let threshold = self.hard_gap_samples as f64;
        if error <= -threshold {
            Some(GapCorrection::InsertSilence((-error).round() as usize))
        } else if error >= threshold {
            Some(GapCorrection::TrimSamples(error.round() as usize))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, epsilon: f64) -> bool {
        (a - b).abs() <= epsilon
    }

    #[test]
    fn correction_is_neutral_when_exactly_on_time() {
        let ctrl = DriftController::new(48_000);
        let ratio = ctrl.correction(Duration::from_secs(1), 48_000);
        assert!(approx_eq(ratio, 1.0, 1e-9), "got {ratio}");
    }

    #[test]
    fn correction_speeds_up_when_behind_real_time() {
        let ctrl = DriftController::new(48_000);
        // Only 47_900 samples written after 1s of 48kHz elapsed: behind.
        let ratio = ctrl.correction(Duration::from_secs(1), 47_900);
        assert!(ratio > 1.0, "expected ratio > 1.0 to catch up, got {ratio}");
    }

    #[test]
    fn correction_slows_down_when_ahead_of_real_time() {
        let ctrl = DriftController::new(48_000);
        // 48_100 samples written after 1s of 48kHz elapsed: ahead.
        let ratio = ctrl.correction(Duration::from_secs(1), 48_100);
        assert!(
            ratio < 1.0,
            "expected ratio < 1.0 to slow down, got {ratio}"
        );
    }

    #[test]
    fn correction_is_clamped_to_max_rel() {
        let ctrl = DriftController::new(48_000);
        // A huge deficit should clamp at the +2% ceiling, not blow past it.
        let ratio = ctrl.correction(Duration::from_secs(3600), 0);
        assert!(approx_eq(ratio, 1.02, 1e-9), "got {ratio}");

        // A huge surplus should clamp at the -2% floor.
        let ratio = ctrl.correction(Duration::from_secs(0), 10_000_000);
        assert!(approx_eq(ratio, 0.98, 1e-9), "got {ratio}");
    }

    #[test]
    fn gap_correction_is_none_within_threshold() {
        let ctrl = DriftController::new(48_000);
        // A few samples off after 1s is well within the 50ms hard-gap band.
        assert_eq!(ctrl.gap_correction(Duration::from_secs(1), 47_990), None);
        assert_eq!(ctrl.gap_correction(Duration::from_secs(1), 48_010), None);
    }

    #[test]
    fn gap_correction_inserts_silence_when_far_behind() {
        let ctrl = DriftController::new(48_000);
        // 200ms behind after 1s elapsed — well past the 50ms hard-gap band.
        let correction = ctrl.gap_correction(Duration::from_secs(1), 38_400);
        assert_eq!(correction, Some(GapCorrection::InsertSilence(9_600)));
    }

    #[test]
    fn gap_correction_trims_when_far_ahead() {
        let ctrl = DriftController::new(48_000);
        // 200ms ahead after 1s elapsed.
        let correction = ctrl.gap_correction(Duration::from_secs(1), 57_600);
        assert_eq!(correction, Some(GapCorrection::TrimSamples(9_600)));
    }

    #[test]
    fn gap_correction_threshold_scales_with_target_rate() {
        let ctrl = DriftController::new(16_000);
        // 50ms at 16kHz is 800 samples; 799 behind must still be "no gap".
        assert_eq!(ctrl.gap_correction(Duration::from_secs(1), 15_201), None);
        // 801 behind must trip it.
        assert_eq!(
            ctrl.gap_correction(Duration::from_secs(1), 15_199),
            Some(GapCorrection::InsertSilence(801))
        );
    }
}
