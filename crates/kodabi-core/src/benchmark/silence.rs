//! Silence / phantom-text scoring — Whisper's documented failure mode
//! (FOUNDING_DOC §8: "phantom text during pauses") versus Parakeet's
//! structural VAD-gated advantage. Scored per channel, since each engine
//! transcribes one mono channel WAV at a time (see the benchmark README).

use serde::{Deserialize, Serialize};

use crate::raw_session::TranscriptSegment;
use crate::transcription::Channel;

use super::text::normalize_tokens;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SilenceChannelScore {
    pub channel: Channel,
    pub silence_ms: u64,
    pub phantom_segments: usize,
    pub phantom_words: usize,
    /// Phantom segments whose silent window has no reference speech on
    /// *either* channel — a pure hallucination, not audio bleed from the
    /// other side.
    pub phantom_segments_mutual_silence: usize,
    /// Phantom segments whose silent window overlaps the *other* channel's
    /// reference speech — plausibly loudspeaker/mic bleed rather than a pure
    /// hallucination, so don't count these against silence-safety at face
    /// value.
    pub phantom_segments_crosstalk: usize,
    /// Words in the mutual-silence (pure-hallucination) phantom segments only —
    /// the crosstalk-bleed words are excluded, since those aren't the
    /// silence-safety failure the benchmark is measuring. This, not
    /// [`Self::phantom_words`], is what feeds the engine-comparison ranking.
    pub phantom_words_mutual_silence: usize,
    /// *All* `phantom_words` (mutual-silence and crosstalk alike) normalized by
    /// minutes of true silence — a per-channel diagnostic of total phantom
    /// pressure. `0.0` when there is no silence to normalize by. The
    /// crosstalk-excluding silence-safety metric lives on [`SilenceScore`] so
    /// it can be aggregated across channels by shared denominator.
    pub phantom_words_per_silent_minute: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SilenceScore {
    pub channels: Vec<SilenceChannelScore>,
    pub total_phantom_segments: usize,
    pub total_phantom_words: usize,
    /// Mutual-silence (pure-hallucination) phantom segments across all
    /// channels — the "true silence hallucinations" headline, crosstalk bleed
    /// excluded.
    pub total_phantom_segments_mutual_silence: usize,
    /// Mutual-silence phantom words across all channels.
    pub total_phantom_words_mutual_silence: usize,
    /// The silence-safety metric that decides the engine comparison:
    /// mutual-silence phantom words per minute of true silence, aggregated by
    /// the shared denominator `Σ words / Σ silent-minutes` (not an average of
    /// the per-channel rates, which would mis-weight channels with unequal
    /// silence). `0.0` when there is no silence to normalize by.
    pub hallucinated_words_per_silent_minute: f64,
}

/// Scores how much of `hypothesis` lands in silence per `reference`, for
/// each `(channel, true_audio_duration_ms)` pair in `channel_durations`.
///
/// `channel_durations` must carry the *true* recorded length, not the last
/// reference timestamp — trailing silence after the final reference
/// utterance is exactly where Whisper is known to hallucinate, so treating
/// "no more reference segments" as "no more silence to score" would hide its
/// worst failure mode.
pub fn silence_behavior(
    reference: &[TranscriptSegment],
    hypothesis: &[TranscriptSegment],
    channel_durations: &[(Channel, u64)],
) -> SilenceScore {
    let channels: Vec<SilenceChannelScore> = channel_durations
        .iter()
        .map(|&(channel, duration_ms)| score_channel(reference, hypothesis, channel, duration_ms))
        .collect();

    let total_phantom_segments = channels.iter().map(|c| c.phantom_segments).sum();
    let total_phantom_words = channels.iter().map(|c| c.phantom_words).sum();
    let total_phantom_segments_mutual_silence = channels
        .iter()
        .map(|c| c.phantom_segments_mutual_silence)
        .sum();
    let total_phantom_words_mutual_silence: usize = channels
        .iter()
        .map(|c| c.phantom_words_mutual_silence)
        .sum();

    // Aggregate by the shared denominator (total mutual-silence words over
    // total silent minutes) rather than averaging the per-channel rates, so a
    // channel with little silence can't dominate the metric.
    let total_silence_ms: u64 = channels.iter().map(|c| c.silence_ms).sum();
    let total_silent_minutes = total_silence_ms as f64 / 60_000.0;
    let hallucinated_words_per_silent_minute = if total_silent_minutes > 0.0 {
        total_phantom_words_mutual_silence as f64 / total_silent_minutes
    } else {
        0.0
    };

    SilenceScore {
        channels,
        total_phantom_segments,
        total_phantom_words,
        total_phantom_segments_mutual_silence,
        total_phantom_words_mutual_silence,
        hallucinated_words_per_silent_minute,
    }
}

fn score_channel(
    reference: &[TranscriptSegment],
    hypothesis: &[TranscriptSegment],
    channel: Channel,
    duration_ms: u64,
) -> SilenceChannelScore {
    let own_speech = merge(spans(reference, channel));
    let other_speech = merge(spans_excluding(reference, channel));
    let silence = complement(&own_speech, duration_ms);
    let silence_ms: u64 = silence.iter().map(|&(s, e)| e - s).sum();

    let mut phantom_segments = 0;
    let mut phantom_words = 0;
    let mut mutual_silence = 0;
    let mut mutual_silence_words = 0;
    let mut crosstalk = 0;

    for seg in hypothesis.iter().filter(|s| s.channel == channel) {
        let span = seg.end_ms.saturating_sub(seg.start_ms);
        if span == 0 {
            continue;
        }
        let silent_overlap = overlap_ms(&silence, seg.start_ms, seg.end_ms);
        // A phantom segment is one where at least half its duration falls in
        // silence, rather than requiring the whole span — a segment that
        // clips the tail end of a real utterance shouldn't count.
        if silent_overlap * 2 < span {
            continue;
        }
        let words = normalize_tokens(&seg.text).len();
        phantom_segments += 1;
        phantom_words += words;
        if spans_overlap(&other_speech, seg.start_ms, seg.end_ms) {
            crosstalk += 1;
        } else {
            mutual_silence += 1;
            mutual_silence_words += words;
        }
    }

    let silent_minutes = silence_ms as f64 / 60_000.0;
    let phantom_words_per_silent_minute = if silent_minutes > 0.0 {
        phantom_words as f64 / silent_minutes
    } else {
        0.0
    };

    SilenceChannelScore {
        channel,
        silence_ms,
        phantom_segments,
        phantom_words,
        phantom_segments_mutual_silence: mutual_silence,
        phantom_segments_crosstalk: crosstalk,
        phantom_words_mutual_silence: mutual_silence_words,
        phantom_words_per_silent_minute,
    }
}

fn spans(
    segments: &[TranscriptSegment],
    channel: Channel,
) -> impl Iterator<Item = (u64, u64)> + '_ {
    segments
        .iter()
        .filter(move |s| s.channel == channel)
        .map(|s| (s.start_ms, s.end_ms))
}

fn spans_excluding(
    segments: &[TranscriptSegment],
    channel: Channel,
) -> impl Iterator<Item = (u64, u64)> + '_ {
    segments
        .iter()
        .filter(move |s| s.channel != channel)
        .map(|s| (s.start_ms, s.end_ms))
}

/// Sorts and merges overlapping/adjacent `(start, end)` spans.
fn merge(spans: impl Iterator<Item = (u64, u64)>) -> Vec<(u64, u64)> {
    let mut spans: Vec<(u64, u64)> = spans.filter(|&(s, e)| e > s).collect();
    spans.sort_by_key(|&(s, _)| s);

    let mut merged: Vec<(u64, u64)> = Vec::with_capacity(spans.len());
    for (start, end) in spans {
        match merged.last_mut() {
            Some((_, last_end)) if start <= *last_end => *last_end = (*last_end).max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// The complement of `speech` within `[0, duration_ms)`.
fn complement(speech: &[(u64, u64)], duration_ms: u64) -> Vec<(u64, u64)> {
    let mut gaps = Vec::new();
    let mut cursor = 0u64;
    for &(start, end) in speech {
        let start = start.min(duration_ms);
        let end = end.min(duration_ms);
        if start > cursor {
            gaps.push((cursor, start));
        }
        cursor = cursor.max(end);
    }
    if cursor < duration_ms {
        gaps.push((cursor, duration_ms));
    }
    gaps
}

fn overlap_ms(spans: &[(u64, u64)], start: u64, end: u64) -> u64 {
    spans
        .iter()
        .map(|&(s, e)| {
            let lo = s.max(start);
            let hi = e.min(end);
            hi.saturating_sub(lo)
        })
        .sum()
}

fn spans_overlap(spans: &[(u64, u64)], start: u64, end: u64) -> bool {
    spans.iter().any(|&(s, e)| s.max(start) < e.min(end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(channel: Channel, start_ms: u64, end_ms: u64, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            index: 0,
            channel,
            speaker: None,
            start_ms,
            end_ms,
            text: text.to_owned(),
        }
    }

    #[test]
    fn phantom_segment_in_a_silent_gap_is_counted() {
        let reference = vec![segment(Channel::You, 0, 1000, "hello there")];
        let hypothesis = vec![segment(Channel::You, 2000, 2500, "hello there")];
        let durations = [(Channel::You, 5000), (Channel::Them, 5000)];

        let score = silence_behavior(&reference, &hypothesis, &durations);
        let you = &score.channels[0];

        assert_eq!(you.phantom_segments, 1);
        assert_eq!(you.phantom_words, 2);
        assert_eq!(you.phantom_segments_mutual_silence, 1);
        assert_eq!(you.phantom_segments_crosstalk, 0);
    }

    #[test]
    fn segment_inside_real_speech_is_not_phantom() {
        let reference = vec![segment(Channel::You, 0, 2000, "hello there friend")];
        let hypothesis = vec![segment(Channel::You, 500, 1000, "there")];
        let durations = [(Channel::You, 5000)];

        let score = silence_behavior(&reference, &hypothesis, &durations);

        assert_eq!(score.channels[0].phantom_segments, 0);
    }

    #[test]
    fn tail_hallucination_after_last_reference_utterance_is_counted() {
        // Reference speech ends at 1000ms, but the channel actually runs to
        // 5000ms — a hypothesis segment after 1000ms is still in-bounds
        // silence, not off the end of the scored window.
        let reference = vec![segment(Channel::You, 0, 1000, "hello")];
        let hypothesis = vec![segment(Channel::You, 4000, 4200, "goodbye")];
        let durations = [(Channel::You, 5000)];

        let score = silence_behavior(&reference, &hypothesis, &durations);

        assert_eq!(score.channels[0].phantom_segments, 1);
    }

    #[test]
    fn crosstalk_phantom_overlaps_the_other_channels_speech() {
        let reference = vec![segment(Channel::Them, 2000, 2500, "the other side talks")];
        let hypothesis = vec![segment(Channel::You, 2100, 2400, "bleed")];
        let durations = [(Channel::You, 5000), (Channel::Them, 5000)];

        let score = silence_behavior(&reference, &hypothesis, &durations);
        let you = &score.channels[0];

        assert_eq!(you.phantom_segments, 1);
        assert_eq!(you.phantom_segments_crosstalk, 1);
        assert_eq!(you.phantom_segments_mutual_silence, 0);
    }

    #[test]
    fn exactly_half_in_silence_counts_as_phantom() {
        let reference = vec![segment(Channel::You, 0, 1000, "hello")];
        // 1000..2000 spans equally silence (1000..1500 real speech ends at
        // 1000, so 1000..2000 is fully silence) — construct a segment that is
        // exactly half in speech, half in silence instead.
        let hypothesis = vec![segment(Channel::You, 500, 1500, "half and half")];
        let durations = [(Channel::You, 5000)];

        let score = silence_behavior(&reference, &hypothesis, &durations);

        assert_eq!(score.channels[0].phantom_segments, 1);
    }

    #[test]
    fn crosstalk_words_are_excluded_from_the_hallucination_rate() {
        // "you" channel: one pure hallucination in mutual silence (2 words) and
        // one crosstalk bleed over "them"'s speech (3 words). Only the mutual-
        // silence words count toward silence-safety.
        let reference = vec![segment(
            Channel::Them,
            2000,
            2600,
            "the other side is talking",
        )];
        let hypothesis = vec![
            segment(Channel::You, 500, 900, "pure hallucination"),
            segment(Channel::You, 2100, 2500, "bleed bleed bleed"),
        ];
        let durations = [(Channel::You, 60_000), (Channel::Them, 60_000)];

        let score = silence_behavior(&reference, &hypothesis, &durations);
        let you = &score.channels[0];

        assert_eq!(you.phantom_words, 5);
        assert_eq!(you.phantom_words_mutual_silence, 2);
        assert_eq!(score.total_phantom_words_mutual_silence, 2);
        assert_eq!(score.total_phantom_segments_mutual_silence, 1);
        // "you" silence is ~1 min, "them" silence ~1 min → ~2 silent minutes;
        // 2 hallucinated words / 2 min = ~1.0, with crosstalk's 3 words excluded.
        assert!((score.hallucinated_words_per_silent_minute - 1.0).abs() < 0.05);
    }

    #[test]
    fn hypothesis_matching_reference_exactly_has_zero_phantoms() {
        let reference = vec![
            segment(Channel::You, 0, 1000, "hello"),
            segment(Channel::You, 3000, 4000, "world"),
        ];
        let hypothesis = reference.clone();
        let durations = [(Channel::You, 5000)];

        let score = silence_behavior(&reference, &hypothesis, &durations);

        assert_eq!(score.total_phantom_segments, 0);
        assert_eq!(score.total_phantom_words, 0);
    }
}
