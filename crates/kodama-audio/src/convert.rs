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

/// Peak absolute amplitude in a buffer of f32 samples (0.0 for empty).
pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0_f32, |acc, &s| acc.max(s.abs()))
}

/// Root-mean-square amplitude in a buffer of f32 samples (0.0 for empty).
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|&s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
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
    fn peak_of_silence_is_zero() {
        assert_eq!(peak(&[0.0, 0.0, 0.0]), 0.0);
    }

    #[test]
    fn peak_finds_largest_absolute_value() {
        assert_eq!(peak(&[-1.0, 0.5, 0.3]), 1.0);
    }

    #[test]
    fn peak_of_empty_is_zero() {
        assert_eq!(peak(&[]), 0.0);
    }

    #[test]
    fn rms_of_silence_is_zero() {
        assert_eq!(rms(&[0.0, 0.0, 0.0]), 0.0);
    }

    #[test]
    fn rms_of_full_scale_square_wave_is_one() {
        assert!(approx_eq(rms(&[1.0, -1.0, 1.0, -1.0]), 1.0, 0.0001));
    }

    #[test]
    fn rms_of_empty_is_zero() {
        assert_eq!(rms(&[]), 0.0);
    }
}
