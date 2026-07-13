//! Pure, dependency-free helpers shared by every transcription engine in this
//! crate. Kept free of `sherpa-onnx` types so they compile and unit-test in
//! the default (no native deps) build, without the `parakeet` feature.

use kodama_core::transcription::{Result, TranscriptionError};

/// Sample rate every engine in this crate expects.
pub const SAMPLE_RATE_HZ: u32 = 16_000;

/// Validate an incoming [`AudioChunk`](kodama_core::transcription::AudioChunk)
/// before it reaches an engine.
///
/// Returns `Err(UnsupportedAudio)` for any rate other than 16 kHz. Otherwise
/// returns `Ok(false)` for an empty chunk (nothing to do — the caller should
/// emit no segments) or `Ok(true)` when there is audio to process.
pub fn validate_chunk(sample_rate: u32, is_empty: bool) -> Result<bool> {
    if sample_rate != SAMPLE_RATE_HZ {
        return Err(TranscriptionError::UnsupportedAudio(format!(
            "expected {SAMPLE_RATE_HZ} Hz mono audio, got {sample_rate} Hz"
        )));
    }
    Ok(!is_empty)
}

/// Convert a VAD speech segment's sample offset/length (relative to the whole
/// stream fed so far, at [`SAMPLE_RATE_HZ`]) into millisecond timestamps.
///
/// `start` is clamped to zero (defensive: the VAD contract has it
/// non-negative, but a clamp is cheaper than a panic if that ever changes).
pub fn segment_ms(start: i32, len: i32) -> (u64, u64) {
    let start = i64::from(start.max(0));
    let len = i64::from(len.max(0));
    let start_ms = (start * 1_000 / i64::from(SAMPLE_RATE_HZ)) as u64;
    let end_ms = ((start + len) * 1_000 / i64::from(SAMPLE_RATE_HZ)) as u64;
    (start_ms, end_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrong_sample_rate_is_unsupported() {
        let err = validate_chunk(48_000, false).unwrap_err();
        assert!(matches!(err, TranscriptionError::UnsupportedAudio(_)));
    }

    #[test]
    fn zero_sample_rate_is_unsupported() {
        let err = validate_chunk(0, false).unwrap_err();
        assert!(matches!(err, TranscriptionError::UnsupportedAudio(_)));
    }

    #[test]
    fn empty_chunk_at_correct_rate_yields_no_work() {
        assert!(!validate_chunk(16_000, true).unwrap());
    }

    #[test]
    fn nonempty_chunk_at_correct_rate_yields_work() {
        assert!(validate_chunk(16_000, false).unwrap());
    }

    #[test]
    fn rate_is_checked_before_emptiness() {
        // Mirrors MockEngine's ordering: an unsupported rate is always an
        // error, even for an empty chunk.
        let err = validate_chunk(48_000, true).unwrap_err();
        assert!(matches!(err, TranscriptionError::UnsupportedAudio(_)));
    }

    #[test]
    fn segment_ms_converts_offsets_at_16khz() {
        assert_eq!(segment_ms(0, 16_000), (0, 1_000));
        assert_eq!(segment_ms(16_000, 8_000), (1_000, 1_500));
    }

    #[test]
    fn segment_ms_clamps_negative_start() {
        assert_eq!(segment_ms(-5, 16_000), (0, 1_000));
    }
}
