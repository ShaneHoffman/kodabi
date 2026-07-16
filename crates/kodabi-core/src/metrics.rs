//! Pure timing types for the transcribe → clean → persist pipeline
//! (FOUNDING_DOC §3.7's resource budget, `docs/RESOURCE_BUDGET.md`).
//!
//! [`crate::pipeline::transcribe_and_persist`] always computes these — an
//! `Instant` read is ~free — and returns them to the caller; whether they're
//! ever looked at is the caller's call (see `src-tauri/src/transcribe.rs`'s
//! `KODABI_METRICS` gate). This module stays free of any emission concern
//! (stderr/file/event) so it compiles and unit-tests without one.

/// Per-stage wall-clock timing for one `transcribe_and_persist` run.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PipelineTimings {
    /// Total audio processed, summed across every channel (seconds).
    pub audio_secs: f64,
    /// Summed across channels — a fresh engine is built per channel (see
    /// `transcribe_and_persist`'s docs on why), so this is the total model
    /// load cost, not one channel's.
    pub engine_build_ms: u64,
    /// One entry per channel, in the order `transcribe_and_persist` processed them.
    pub transcribe_ms: Vec<u64>,
    pub assemble_ms: u64,
    /// The glossary cleanup post-pass — a separate `claude` subprocess, so
    /// its own CPU never shows up in this process's self-CPU even though its
    /// wall time counts toward `total_ms`.
    pub cleanup_ms: u64,
    pub persist_ms: u64,
    pub total_ms: u64,
}

impl PipelineTimings {
    /// `audio_secs / wall_secs` — greater than 1.0 means the pipeline ran
    /// faster than realtime. This is the resource-budget ticket's "audio ÷
    /// wall" convention, the *inverse* of the usual wall÷audio real-time
    /// factor: read the ratio, don't assume which direction is "faster"
    /// without checking which convention a number uses.
    ///
    /// Wall time is floored at the 1 ms resolution of `total_ms` so an instant
    /// run (`total_ms` rounding to 0, e.g. a `MockEngine`) yields a finite
    /// ratio. Left as `f64::INFINITY`, it would serialize to `null` in the
    /// `KODABI_METRICS` JSONL (serde_json maps non-finite floats to `null`),
    /// silently dropping the very number the line exists to record.
    pub fn speed_x(&self) -> f64 {
        real_time_factor(self.audio_secs, self.total_ms.max(1) as f64 / 1_000.0)
    }
}

/// `audio_secs / wall_secs`, guarded against a zero/negative wall time (an
/// instant `MockEngine` run rounding to 0 ms) rather than dividing by zero.
pub fn real_time_factor(audio_secs: f64, wall_secs: f64) -> f64 {
    if wall_secs <= 0.0 {
        return f64::INFINITY;
    }
    audio_secs / wall_secs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_time_factor_divides_audio_by_wall() {
        assert_eq!(real_time_factor(10.0, 5.0), 2.0);
        assert_eq!(real_time_factor(5.0, 10.0), 0.5);
    }

    #[test]
    fn real_time_factor_guards_zero_wall_time() {
        assert_eq!(real_time_factor(10.0, 0.0), f64::INFINITY);
    }

    #[test]
    fn real_time_factor_guards_negative_wall_time() {
        assert_eq!(real_time_factor(10.0, -1.0), f64::INFINITY);
    }

    #[test]
    fn speed_x_matches_real_time_factor_of_its_own_fields() {
        let timings = PipelineTimings {
            audio_secs: 20.0,
            engine_build_ms: 100,
            transcribe_ms: vec![2_000, 3_000],
            assemble_ms: 10,
            cleanup_ms: 500,
            persist_ms: 5,
            total_ms: 5_000,
        };
        assert_eq!(timings.speed_x(), 4.0);
    }

    #[test]
    fn speed_x_is_finite_when_total_ms_rounds_to_zero() {
        // An instant run (e.g. `MockEngine`) times at 0 ms. `speed_x` must
        // stay finite so it survives JSON serialization instead of becoming
        // `null` — serde_json maps `f64::INFINITY` to `null`.
        let timings = PipelineTimings {
            audio_secs: 4.0,
            engine_build_ms: 0,
            transcribe_ms: vec![0],
            assemble_ms: 0,
            cleanup_ms: 0,
            persist_ms: 0,
            total_ms: 0,
        };
        assert!(timings.speed_x().is_finite());
        // 4.0s audio / 1 ms floored wall = 4000x.
        assert_eq!(timings.speed_x(), 4_000.0);
    }
}
