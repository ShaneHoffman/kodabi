//! Pure sample-format conversion and level-measurement helpers used by the
//! capture RT callback. Kept free of cpal streams/devices so they're
//! testable without a real audio device.

use cpal::{FromSample, Sample};

/// Convert a slice of device-native samples to interleaved f32, using cpal's
/// own `FromSample` conversion so behavior matches what cpal's backends
/// assume of the device's native format.
pub fn to_f32<T>(samples: &[T]) -> Vec<f32>
where
    T: Sample,
    f32: FromSample<T>,
{
    samples.iter().map(|&s| f32::from_sample(s)).collect()
}

/// Peak absolute amplitude and RMS of a buffer of f32 samples, computed in a
/// single pass (both `0.0` for an empty buffer). One pass matters here: this
/// runs in the capture RT callback on every buffer.
pub fn levels(samples: &[f32]) -> (f32, f32) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    let mut peak = 0.0_f32;
    let mut sum_sq = 0.0_f32;
    for &s in samples {
        peak = peak.max(s.abs());
        sum_sq += s * s;
    }
    (peak, (sum_sq / samples.len() as f32).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32, epsilon: f32) -> bool {
        (a - b).abs() <= epsilon
    }

    #[test]
    fn i16_extremes_convert_to_f32_range() {
        let out = to_f32(&[i16::MAX, 0, i16::MIN]);
        assert!(approx_eq(out[0], 1.0, 0.001), "got {}", out[0]);
        assert!(approx_eq(out[1], 0.0, 0.001), "got {}", out[1]);
        assert!(approx_eq(out[2], -1.0, 0.001), "got {}", out[2]);
    }

    #[test]
    fn u16_midpoint_converts_to_zero() {
        let out = to_f32(&[32_768_u16]);
        assert!(approx_eq(out[0], 0.0, 0.001), "got {}", out[0]);
    }

    #[test]
    fn f32_passes_through_unchanged() {
        let out = to_f32(&[0.5_f32, -0.25, 0.0]);
        assert_eq!(out, vec![0.5, -0.25, 0.0]);
    }

    #[test]
    fn levels_of_silence_are_zero() {
        assert_eq!(levels(&[0.0, 0.0, 0.0]), (0.0, 0.0));
    }

    #[test]
    fn levels_peak_finds_largest_absolute_value() {
        let (peak, _rms) = levels(&[-1.0, 0.5, 0.3]);
        assert_eq!(peak, 1.0);
    }

    #[test]
    fn levels_rms_of_full_scale_square_wave_is_one() {
        let (peak, rms) = levels(&[1.0, -1.0, 1.0, -1.0]);
        assert_eq!(peak, 1.0);
        assert!(approx_eq(rms, 1.0, 0.0001), "got {rms}");
    }

    #[test]
    fn levels_of_empty_are_zero() {
        assert_eq!(levels(&[]), (0.0, 0.0));
    }
}
