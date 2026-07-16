//! Overall word error rate — a supporting accuracy metric alongside the
//! headline proper-noun recall (the founding doc's stated criterion; raw
//! WER is never named there, so this exists for a fuller picture rather than
//! as a recorded decision criterion).

use serde::{Deserialize, Serialize};

use crate::raw_session::TranscriptSegment;
use crate::transcription::Channel;

use super::text::normalize_tokens;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WerChannelScore {
    pub channel: Channel,
    pub reference_tokens: usize,
    pub substitutions: usize,
    pub deletions: usize,
    pub insertions: usize,
    pub wer: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WerScore {
    pub reference_tokens: usize,
    pub substitutions: usize,
    pub deletions: usize,
    pub insertions: usize,
    pub wer: f64,
    pub per_channel: Vec<WerChannelScore>,
}

/// Token-level word error rate of `hypothesis` against `reference`.
///
/// Scored **per channel** and then micro-aggregated, rather than over
/// [`crate::raw_session::assemble`]'s single start_ms-interleaved stream —
/// interleaving two channels' text by timestamp can split one channel's
/// sentence around the other's, which would inflate edit distance with
/// artifacts of the merge rather than real transcription errors.
pub fn word_error_rate(
    reference: &[TranscriptSegment],
    hypothesis: &[TranscriptSegment],
) -> WerScore {
    // Scored channels come from the *reference* only: WER is defined against a
    // reference, so a channel the reference never had has no denominator to
    // score against. Including a hypothesis-only channel would fold its words
    // into the aggregate as insertions while its own per-channel `wer` reads
    // `0.0` (empty reference) — an inconsistency between the row and the total.
    // A reference channel the hypothesis missed entirely is still scored (all
    // deletions), which is correct.
    let channels = reference_channels(reference);

    let per_channel: Vec<WerChannelScore> = channels
        .into_iter()
        .map(|channel| score_channel(reference, hypothesis, channel))
        .collect();

    let reference_tokens = per_channel.iter().map(|c| c.reference_tokens).sum();
    let substitutions = per_channel.iter().map(|c| c.substitutions).sum();
    let deletions = per_channel.iter().map(|c| c.deletions).sum();
    let insertions = per_channel.iter().map(|c| c.insertions).sum();
    let wer = if reference_tokens > 0 {
        (substitutions + deletions + insertions) as f64 / reference_tokens as f64
    } else {
        0.0
    };

    WerScore {
        reference_tokens,
        substitutions,
        deletions,
        insertions,
        wer,
        per_channel,
    }
}

fn reference_channels(reference: &[TranscriptSegment]) -> Vec<Channel> {
    let mut channels: Vec<Channel> = reference.iter().map(|s| s.channel).collect();
    channels.sort_by_key(channel_rank);
    channels.dedup();
    channels
}

fn channel_rank(channel: &Channel) -> u8 {
    match channel {
        Channel::You => 0,
        Channel::Them => 1,
        Channel::Unknown => 2,
    }
}

fn score_channel(
    reference: &[TranscriptSegment],
    hypothesis: &[TranscriptSegment],
    channel: Channel,
) -> WerChannelScore {
    let ref_tokens = concat_tokens(reference, channel);
    let hyp_tokens = concat_tokens(hypothesis, channel);
    let (substitutions, deletions, insertions) = levenshtein_ops(&ref_tokens, &hyp_tokens);
    let wer = if !ref_tokens.is_empty() {
        (substitutions + deletions + insertions) as f64 / ref_tokens.len() as f64
    } else {
        0.0
    };
    WerChannelScore {
        channel,
        reference_tokens: ref_tokens.len(),
        substitutions,
        deletions,
        insertions,
        wer,
    }
}

fn concat_tokens(segments: &[TranscriptSegment], channel: Channel) -> Vec<String> {
    let mut ordered: Vec<&TranscriptSegment> =
        segments.iter().filter(|s| s.channel == channel).collect();
    ordered.sort_by_key(|s| s.index);
    ordered
        .into_iter()
        .flat_map(|s| normalize_tokens(&s.text))
        .collect()
}

/// Token-level Levenshtein distance with unit substitution/insertion/deletion
/// cost, returning `(substitutions, deletions, insertions)` recovered via
/// backtrace through the DP matrix (`reference` indexes rows, `hypothesis`
/// indexes columns).
fn levenshtein_ops(reference: &[String], hypothesis: &[String]) -> (usize, usize, usize) {
    let (r, h) = (reference.len(), hypothesis.len());
    let mut dp = vec![vec![0usize; h + 1]; r + 1];
    for (i, row) in dp.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in dp[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=r {
        for j in 1..=h {
            dp[i][j] = if reference[i - 1] == hypothesis[j - 1] {
                dp[i - 1][j - 1]
            } else {
                1 + dp[i - 1][j - 1].min(dp[i - 1][j]).min(dp[i][j - 1])
            };
        }
    }

    let (mut i, mut j) = (r, h);
    let (mut subs, mut dels, mut inss) = (0, 0, 0);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && reference[i - 1] == hypothesis[j - 1] && dp[i][j] == dp[i - 1][j - 1] {
            i -= 1;
            j -= 1;
        } else if i > 0 && j > 0 && dp[i][j] == dp[i - 1][j - 1] + 1 {
            subs += 1;
            i -= 1;
            j -= 1;
        } else if i > 0 && dp[i][j] == dp[i - 1][j] + 1 {
            dels += 1;
            i -= 1;
        } else {
            inss += 1;
            j -= 1;
        }
    }

    (subs, dels, inss)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(index: u64, channel: Channel, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            index,
            channel,
            speaker: None,
            start_ms: index * 1000,
            end_ms: index * 1000 + 500,
            text: text.to_owned(),
        }
    }

    #[test]
    fn identical_transcripts_have_zero_wer() {
        let reference = vec![segment(0, Channel::You, "hello world")];
        let hypothesis = reference.clone();

        let score = word_error_rate(&reference, &hypothesis);

        assert_eq!(score.wer, 0.0);
        assert_eq!(score.substitutions + score.deletions + score.insertions, 0);
    }

    #[test]
    fn single_substitution_is_one_over_reference_length() {
        let reference = vec![segment(0, Channel::You, "a b c")];
        let hypothesis = vec![segment(0, Channel::You, "a x c")];

        let score = word_error_rate(&reference, &hypothesis);

        assert_eq!(score.substitutions, 1);
        assert_eq!(score.deletions, 0);
        assert_eq!(score.insertions, 0);
        assert!((score.wer - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn pure_insertion_is_counted() {
        let reference = vec![segment(0, Channel::You, "a b")];
        let hypothesis = vec![segment(0, Channel::You, "a b c")];

        let score = word_error_rate(&reference, &hypothesis);

        assert_eq!(score.insertions, 1);
        assert_eq!(score.substitutions, 0);
        assert_eq!(score.deletions, 0);
    }

    #[test]
    fn pure_deletion_is_counted() {
        let reference = vec![segment(0, Channel::You, "a b c")];
        let hypothesis = vec![segment(0, Channel::You, "a c")];

        let score = word_error_rate(&reference, &hypothesis);

        assert_eq!(score.deletions, 1);
        assert_eq!(score.substitutions, 0);
        assert_eq!(score.insertions, 0);
    }

    #[test]
    fn hypothesis_only_channel_does_not_inflate_the_aggregate() {
        // The reference is "you"-only; the hypothesis also carries a "them"
        // channel the reference never had. Those tokens have no reference to
        // score against, so they must not appear as insertions in the total.
        let reference = vec![segment(0, Channel::You, "a b c")];
        let hypothesis = vec![
            segment(0, Channel::You, "a b c"),
            segment(1, Channel::Them, "spurious words here"),
        ];

        let score = word_error_rate(&reference, &hypothesis);

        assert_eq!(score.insertions, 0);
        assert_eq!(score.wer, 0.0);
        assert_eq!(score.per_channel.len(), 1);
        assert_eq!(score.per_channel[0].channel, Channel::You);
    }

    #[test]
    fn reference_channel_missing_from_hypothesis_is_all_deletions() {
        // The mirror case: a reference channel the hypothesis dropped entirely
        // is still scored, as deletions — it must not be silently excluded.
        let reference = vec![
            segment(0, Channel::You, "a b"),
            segment(1, Channel::Them, "x y z"),
        ];
        let hypothesis = vec![segment(0, Channel::You, "a b")];

        let score = word_error_rate(&reference, &hypothesis);

        assert_eq!(score.deletions, 3);
        assert_eq!(score.per_channel.len(), 2);
    }

    #[test]
    fn punctuation_and_case_are_normalized_away() {
        let reference = vec![segment(0, Channel::You, "Hello, World!")];
        let hypothesis = vec![segment(0, Channel::You, "hello world")];

        let score = word_error_rate(&reference, &hypothesis);

        assert_eq!(score.wer, 0.0);
    }
}
