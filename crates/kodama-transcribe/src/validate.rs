//! Pure, dependency-free helpers shared by every transcription engine in this
//! crate. Kept free of `sherpa-onnx` types so they compile and unit-test in
//! the default (no native deps) build, without the `parakeet` feature.

#[cfg(any(feature = "parakeet", feature = "vad"))]
use std::path::Path;

use kodama_core::transcription::{Result, Segment, TranscriptionError};

/// Sample rate every engine in this crate expects.
pub const SAMPLE_RATE_HZ: u32 = 16_000;

/// Assert a required model file exists on disk, surfacing a missing one as
/// [`TranscriptionError::ModelLoad`]. Shared by every engine's constructor.
///
/// Gated on the native-engine features so it doesn't read as dead code in the
/// default (no-engine) build that `clippy --workspace -D warnings` lints.
/// `whisper` always enables `vad`, so it's covered without being named here.
#[cfg(any(feature = "parakeet", feature = "vad"))]
pub(crate) fn require_file(path: &Path) -> Result<()> {
    if !path.is_file() {
        return Err(TranscriptionError::ModelLoad(format!(
            "missing model file: {}",
            path.display()
        )));
    }
    Ok(())
}

/// Render a model path as the UTF-8 `String` the native engines' FFI configs
/// require, surfacing a non-UTF-8 path as [`TranscriptionError::ModelLoad`].
///
/// Gated on the native-engine features (see [`require_file`]).
#[cfg(any(feature = "parakeet", feature = "vad"))]
pub(crate) fn path_to_string(path: &Path) -> Result<String> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        TranscriptionError::ModelLoad(format!("model path is not valid UTF-8: {}", path.display()))
    })
}

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

/// Re-map a segment produced by decoding one VAD speech span in isolation
/// (timestamps relative to that span's own buffer) onto the absolute session
/// clock, given the span's own `(start_ms, end_ms)` on that clock.
///
/// Returns `None` for a segment that should not be emitted at all:
///
/// - a relative `start_ms` at or past the span's own length. whisper.cpp
///   zero-pads every buffer it decodes up to a 30 s mel window and can emit a
///   hallucinated trailing segment sitting *in that padding*; decoding each
///   VAD span in isolation removes the silence *between* speech spans, but
///   not this *intra*-window pad, so such a segment must be dropped rather
///   than clamped into a bogus zero-width span at the boundary.
/// - empty (post-trim) text.
/// - a zero/negative-width result after clamping (defensive: shouldn't arise
///   from the checks above, but kept so callers can rely on `start_ms <
///   end_ms` unconditionally).
pub fn offset_into_span(seg: Segment, span_start_ms: u64, span_end_ms: u64) -> Option<Segment> {
    let span_len_ms = span_end_ms.saturating_sub(span_start_ms);
    if seg.start_ms >= span_len_ms || seg.text.trim().is_empty() {
        return None;
    }

    let start_ms = span_start_ms + seg.start_ms;
    let end_ms = span_start_ms + seg.end_ms.min(span_len_ms);
    if start_ms >= end_ms {
        return None;
    }

    Some(Segment {
        start_ms,
        end_ms,
        text: seg.text,
    })
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

    fn seg(start_ms: u64, end_ms: u64, text: &str) -> Segment {
        Segment {
            start_ms,
            end_ms,
            text: text.to_owned(),
        }
    }

    #[test]
    fn offset_into_span_shifts_onto_the_session_clock() {
        // A 2s VAD span starting at 10s; whisper reported this segment at
        // 0.5s-1.5s relative to that span's own buffer.
        let mapped = offset_into_span(seg(500, 1_500, "hello"), 10_000, 12_000).unwrap();
        assert_eq!(mapped.start_ms, 10_500);
        assert_eq!(mapped.end_ms, 11_500);
        assert_eq!(mapped.text, "hello");
    }

    #[test]
    fn offset_into_span_clamps_end_to_the_span_length() {
        // whisper's end timestamp overruns the span's own length slightly.
        let mapped = offset_into_span(seg(0, 2_500, "hi"), 5_000, 7_000).unwrap();
        assert_eq!(mapped.start_ms, 5_000);
        assert_eq!(mapped.end_ms, 7_000);
    }

    #[test]
    fn offset_into_span_drops_a_segment_starting_in_the_padding() {
        // whisper zero-pads to a 30s window; a hallucination anchored past the
        // span's own length is in that padding and must be dropped, not clamped.
        assert!(offset_into_span(seg(2_000, 2_100, "phantom"), 0, 2_000).is_none());
    }

    #[test]
    fn offset_into_span_drops_empty_text() {
        assert!(offset_into_span(seg(0, 500, "   "), 1_000, 2_000).is_none());
    }

    #[test]
    fn offset_into_span_drops_everything_for_a_zero_length_span() {
        assert!(offset_into_span(seg(0, 100, "hi"), 3_000, 3_000).is_none());
    }
}
