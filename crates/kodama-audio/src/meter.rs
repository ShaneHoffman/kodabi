//! Lock-free level meter: lets a status/verification caller read capture
//! progress (frame count, drops, segment count, peak/RMS) without draining
//! the frame channel.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::convert;

#[derive(Debug, Default)]
pub struct LevelMeter {
    frames: AtomicU64,
    dropped: AtomicU64,
    segments: AtomicU32,
    peak_bits: AtomicU32,
    rms_bits: AtomicU32,
}

/// A point-in-time read of a capture session's progress.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct MeterSnapshot {
    pub running: bool,
    pub frames_captured: u64,
    pub frames_dropped: u64,
    pub segments: u32,
    pub peak: f32,
    pub rms: f32,
}

impl LevelMeter {
    /// Record one RT callback's worth of already-converted f32 samples.
    /// `peak` is a running max over the whole session (not a decaying
    /// window) — enough to answer "was there any real audio captured?".
    pub fn observe(&self, samples: &[f32]) {
        let (peak, rms) = convert::levels(samples);
        self.frames.fetch_add(1, Ordering::Relaxed);
        // Safe to compare f32 bit patterns with fetch_max: peak/rms are
        // always >= 0, and non-negative IEEE-754 floats order the same as
        // their bit patterns interpreted as unsigned integers.
        self.peak_bits.fetch_max(peak.to_bits(), Ordering::Relaxed);
        self.rms_bits.store(rms.to_bits(), Ordering::Relaxed);
    }

    /// Record that a frame was dropped because the channel was full.
    pub fn inc_dropped(&self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }

    /// Record that a new segment started (e.g. a device-change rebuild).
    pub fn inc_segments(&self) {
        self.segments.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self, running: bool) -> MeterSnapshot {
        MeterSnapshot {
            running,
            frames_captured: self.frames.load(Ordering::Relaxed),
            frames_dropped: self.dropped.load(Ordering::Relaxed),
            segments: self.segments.load(Ordering::Relaxed),
            peak: f32::from_bits(self.peak_bits.load(Ordering::Relaxed)),
            rms: f32::from_bits(self.rms_bits.load(Ordering::Relaxed)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_meter_snapshots_to_zero() {
        let meter = LevelMeter::default();
        let snap = meter.snapshot(false);
        assert_eq!(snap.frames_captured, 0);
        assert_eq!(snap.frames_dropped, 0);
        assert_eq!(snap.segments, 0);
        assert_eq!(snap.peak, 0.0);
        assert_eq!(snap.rms, 0.0);
        assert!(!snap.running);
    }

    #[test]
    fn observe_updates_frame_count_and_levels() {
        let meter = LevelMeter::default();
        meter.observe(&[0.5, -0.5]);
        let snap = meter.snapshot(true);
        assert_eq!(snap.frames_captured, 1);
        assert_eq!(snap.peak, 0.5);
        assert!(snap.rms > 0.0);
        assert!(snap.running);
    }

    #[test]
    fn peak_is_monotonic_max_across_observations() {
        let meter = LevelMeter::default();
        meter.observe(&[0.2]);
        meter.observe(&[0.9]);
        meter.observe(&[0.1]); // quieter callback must not lower the running peak
        assert_eq!(meter.snapshot(true).peak, 0.9);
    }

    #[test]
    fn inc_dropped_and_inc_segments_count() {
        let meter = LevelMeter::default();
        meter.inc_dropped();
        meter.inc_dropped();
        meter.inc_segments();
        let snap = meter.snapshot(false);
        assert_eq!(snap.frames_dropped, 2);
        assert_eq!(snap.segments, 1);
    }
}
