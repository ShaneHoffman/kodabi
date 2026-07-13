//! Pure downmix and interleave helpers used to combine capture sources into
//! a two-channel session. Kept free of cpal/device types so they're testable
//! without a real audio device (same split as `convert.rs`).

/// Downmix interleaved multi-channel `f32` samples to mono by averaging each
/// frame's channels, appending the result to `out`. A `channels` of `0`
/// appends nothing (there is no frame to average); `1` appends the input
/// unchanged (already mono). This is the allocation-free form: a per-callback
/// caller (the combiner) reuses one `out` buffer across frames rather than
/// allocating a fresh `Vec` each time.
pub fn downmix_to_mono_into(interleaved: &[f32], channels: u16, out: &mut Vec<f32>) {
    let channels = channels as usize;
    if channels == 0 {
        return;
    }
    if channels == 1 {
        out.extend_from_slice(interleaved);
        return;
    }
    out.extend(
        interleaved
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32),
    );
}

/// Interleave two mono `f32` channels into one stereo buffer: `L, R, L, R,
/// ...`. The caller guarantees `left` and `right` are the same length (the
/// combiner pads both channels to equal length before this is ever called);
/// if they differ, the output is truncated to the shorter of the two.
pub fn interleave_stereo(left: &[f32], right: &[f32]) -> Vec<f32> {
    let frames = left.len().min(right.len());
    let mut out = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        out.push(left[i]);
        out.push(right[i]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collect [`downmix_to_mono_into`] into a fresh buffer, for terse
    /// assertions on the downmix semantics.
    fn downmix(interleaved: &[f32], channels: u16) -> Vec<f32> {
        let mut out = Vec::new();
        downmix_to_mono_into(interleaved, channels, &mut out);
        out
    }

    #[test]
    fn downmix_mono_passes_through_unchanged() {
        let samples = vec![0.5, -0.25, 0.0];
        assert_eq!(downmix(&samples, 1), samples);
    }

    #[test]
    fn downmix_zero_channels_is_empty() {
        assert_eq!(downmix(&[1.0, 2.0, 3.0], 0), Vec::<f32>::new());
    }

    #[test]
    fn downmix_stereo_averages_each_frame() {
        // L,R,L,R: (1.0,-1.0) -> 0.0; (0.5,0.5) -> 0.5
        let samples = vec![1.0, -1.0, 0.5, 0.5];
        assert_eq!(downmix(&samples, 2), vec![0.0, 0.5]);
    }

    #[test]
    fn downmix_multichannel_averages_all_channels_in_a_frame() {
        // One frame of 4 channels: (1.0, 1.0, 1.0, -1.0) -> 0.5
        let samples = vec![1.0, 1.0, 1.0, -1.0];
        assert_eq!(downmix(&samples, 4), vec![0.5]);
    }

    #[test]
    fn interleave_produces_alternating_l_r_order() {
        let left = vec![1.0, 2.0, 3.0];
        let right = vec![-1.0, -2.0, -3.0];
        assert_eq!(
            interleave_stereo(&left, &right),
            vec![1.0, -1.0, 2.0, -2.0, 3.0, -3.0]
        );
    }

    #[test]
    fn interleave_truncates_to_shorter_channel() {
        let left = vec![1.0, 2.0, 3.0];
        let right = vec![-1.0, -2.0];
        assert_eq!(interleave_stereo(&left, &right), vec![1.0, -1.0, 2.0, -2.0]);
    }

    #[test]
    fn interleave_of_empty_channels_is_empty() {
        assert_eq!(interleave_stereo(&[], &[]), Vec::<f32>::new());
    }
}
