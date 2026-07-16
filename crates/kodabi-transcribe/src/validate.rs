//! Pure, dependency-free helpers shared by every transcription engine in this
//! crate. Kept free of `sherpa-onnx` types so they compile and unit-test in
//! the default (no native deps) build, without the `parakeet` feature.

#[cfg(any(feature = "parakeet", feature = "vad"))]
use std::path::Path;

use kodabi_core::transcription::{Result, Segment, TranscriptionError};

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

/// Clamp an engine's configured thread count to at least one. sherpa-onnx and
/// whisper.cpp both treat it as a thread-pool size, so a zero or negative
/// value from a future settings layer is nonsensical rather than a valid "use
/// defaults". Shared by every engine's constructor.
///
/// Gated on the native-engine features (see [`require_file`]); `whisper`
/// co-enables `vad`, so it's covered without being named here.
#[cfg(any(feature = "parakeet", feature = "vad"))]
pub(crate) fn clamp_threads(num_threads: i32) -> i32 {
    num_threads.max(1)
}

/// Parses a positive `i32` override (e.g. `KODABI_PARAKEET_THREADS`),
/// falling back to `current` on a blank, unparsable, or non-positive value —
/// an override that would zero out a thread count is nonsensical, not a
/// valid "use zero". Mirrors `kodabi-llm`'s `apply_*_override(config,
/// Option<String>)` pattern so the parsing itself is unit-tested without
/// touching real environment variables.
///
/// Gated on the native-engine features (see [`require_file`]).
#[cfg(any(feature = "parakeet", feature = "vad"))]
pub(crate) fn apply_positive_i32_override(current: i32, raw: Option<String>) -> i32 {
    match raw.and_then(|v| v.trim().parse::<i32>().ok()) {
        Some(v) if v > 0 => v,
        _ => current,
    }
}

/// Parses a non-negative `f32` override (e.g. `KODABI_VAD_MIN_SILENCE`,
/// `KODABI_VAD_MIN_SPEECH`, `KODABI_VAD_MAX_SPEECH` — durations, which have no
/// natural upper bound), falling back to `current` on a blank, unparsable,
/// non-finite, or negative value. See [`apply_positive_i32_override`] for the
/// fallback rationale. A bounded probability (the VAD threshold) uses
/// [`apply_probability_f32_override`] instead.
///
/// Gated on the native-engine features (see [`require_file`]).
#[cfg(any(feature = "parakeet", feature = "vad"))]
pub(crate) fn apply_nonnegative_f32_override(current: f32, raw: Option<String>) -> f32 {
    match raw.and_then(|v| v.trim().parse::<f32>().ok()) {
        Some(v) if v.is_finite() && v >= 0.0 => v,
        _ => current,
    }
}

/// Parses a speech-probability override in `[0.0, 1.0]` (e.g.
/// `KODABI_VAD_THRESHOLD`), falling back to `current` on a blank, unparsable,
/// non-finite, or out-of-range value. Unlike a duration, a probability has a
/// hard upper bound of 1.0: a threshold above it can never be crossed, so it
/// would silently disable VAD gating entirely (every span read as silence,
/// an empty transcript) rather than tuning it — so it is rejected, not
/// applied. See [`apply_positive_i32_override`] for the fallback rationale.
///
/// Gated on the native-engine features (see [`require_file`]).
#[cfg(any(feature = "parakeet", feature = "vad"))]
pub(crate) fn apply_probability_f32_override(current: f32, raw: Option<String>) -> f32 {
    match raw.and_then(|v| v.trim().parse::<f32>().ok()) {
        Some(v) if v.is_finite() && (0.0..=1.0).contains(&v) => v,
        _ => current,
    }
}

/// Parses a `bool` override (e.g. `KODABI_WHISPER_GPU`) from `"true"`/`"1"`
/// or `"false"`/`"0"` (case-insensitive), falling back to `current` on a
/// blank or unrecognized value. See [`apply_positive_i32_override`] for the
/// fallback rationale.
///
/// Gated on `whisper` — the only config field this applies to today
/// (`WhisperConfig::use_gpu`).
#[cfg(feature = "whisper")]
pub(crate) fn apply_bool_override(current: bool, raw: Option<String>) -> bool {
    match raw.as_deref().map(str::trim) {
        Some(s) if s.eq_ignore_ascii_case("true") || s == "1" => true,
        Some(s) if s.eq_ignore_ascii_case("false") || s == "0" => false,
        _ => current,
    }
}

/// Validate an incoming [`AudioChunk`](kodabi_core::transcription::AudioChunk)
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

#[cfg(all(test, any(feature = "parakeet", feature = "vad")))]
mod override_tests {
    use super::*;

    #[test]
    fn i32_override_applies_a_valid_value() {
        assert_eq!(apply_positive_i32_override(1, Some("4".to_owned())), 4);
    }

    #[test]
    fn i32_override_falls_back_when_unset() {
        assert_eq!(apply_positive_i32_override(1, None), 1);
    }

    #[test]
    fn i32_override_falls_back_on_blank() {
        assert_eq!(apply_positive_i32_override(1, Some("  ".to_owned())), 1);
    }

    #[test]
    fn i32_override_falls_back_on_garbage() {
        assert_eq!(apply_positive_i32_override(1, Some("nope".to_owned())), 1);
    }

    #[test]
    fn i32_override_falls_back_on_zero_or_negative() {
        assert_eq!(apply_positive_i32_override(1, Some("0".to_owned())), 1);
        assert_eq!(apply_positive_i32_override(1, Some("-2".to_owned())), 1);
    }

    #[test]
    fn f32_override_applies_a_valid_value() {
        assert_eq!(
            apply_nonnegative_f32_override(0.5, Some("0.7".to_owned())),
            0.7
        );
    }

    #[test]
    fn f32_override_falls_back_when_unset() {
        assert_eq!(apply_nonnegative_f32_override(0.5, None), 0.5);
    }

    #[test]
    fn f32_override_falls_back_on_blank() {
        assert_eq!(
            apply_nonnegative_f32_override(0.5, Some(String::new())),
            0.5
        );
    }

    #[test]
    fn f32_override_falls_back_on_garbage() {
        assert_eq!(
            apply_nonnegative_f32_override(0.5, Some("nope".to_owned())),
            0.5
        );
    }

    #[test]
    fn f32_override_falls_back_on_negative() {
        assert_eq!(
            apply_nonnegative_f32_override(0.5, Some("-1.0".to_owned())),
            0.5
        );
    }

    #[test]
    fn f32_override_accepts_zero() {
        assert_eq!(
            apply_nonnegative_f32_override(0.5, Some("0".to_owned())),
            0.0
        );
    }

    #[test]
    fn probability_override_applies_an_in_range_value() {
        assert_eq!(
            apply_probability_f32_override(0.5, Some("0.7".to_owned())),
            0.7
        );
    }

    #[test]
    fn probability_override_accepts_the_bounds() {
        assert_eq!(
            apply_probability_f32_override(0.5, Some("0".to_owned())),
            0.0
        );
        assert_eq!(
            apply_probability_f32_override(0.5, Some("1".to_owned())),
            1.0
        );
    }

    #[test]
    fn probability_override_falls_back_above_one() {
        // A threshold above 1.0 can never be crossed — silently disabling VAD
        // gating — so it must be rejected, not applied.
        assert_eq!(
            apply_probability_f32_override(0.5, Some("1.5".to_owned())),
            0.5
        );
    }

    #[test]
    fn probability_override_falls_back_on_negative_or_garbage() {
        assert_eq!(
            apply_probability_f32_override(0.5, Some("-0.1".to_owned())),
            0.5
        );
        assert_eq!(
            apply_probability_f32_override(0.5, Some("nope".to_owned())),
            0.5
        );
        assert_eq!(apply_probability_f32_override(0.5, None), 0.5);
    }
}

#[cfg(all(test, feature = "whisper"))]
mod bool_override_tests {
    use super::*;

    #[test]
    fn bool_override_accepts_true_variants() {
        assert!(apply_bool_override(false, Some("true".to_owned())));
        assert!(apply_bool_override(false, Some("TRUE".to_owned())));
        assert!(apply_bool_override(false, Some("1".to_owned())));
    }

    #[test]
    fn bool_override_accepts_false_variants() {
        assert!(!apply_bool_override(true, Some("false".to_owned())));
        assert!(!apply_bool_override(true, Some("FALSE".to_owned())));
        assert!(!apply_bool_override(true, Some("0".to_owned())));
    }

    #[test]
    fn bool_override_falls_back_when_unset_or_garbage() {
        assert!(apply_bool_override(true, None));
        assert!(apply_bool_override(true, Some("nope".to_owned())));
    }
}
