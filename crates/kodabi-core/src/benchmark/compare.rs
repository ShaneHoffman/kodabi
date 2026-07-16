//! Reconciles two independently-produced [`EngineScore`]s (each engine runs
//! in its own feature-gated binary — see `crates/kodabi-transcribe/Cargo.toml`
//! on why Parakeet and Whisper can't share one binary) into a single
//! comparison and an advisory pick.

use serde::{Deserialize, Serialize};

use super::EngineScore;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineComparison {
    pub engines: Vec<EngineScore>,
    /// Advisory only — a human confirms the locked default in
    /// `docs/FOUNDING_DOC.md` §7; this doesn't write anything.
    pub suggested_winner: Option<String>,
    pub rationale: String,
}

/// Picks a suggested winner from `engines` in the priority order the
/// benchmark's manifest establishes: proper-noun recall (headline — a
/// mangled name poisons categorization and search) decides first, then
/// silence-safety (phantom words per silent minute), then speed (RTF).
/// Fewer than two scores yields no suggestion.
pub fn compare_engines(engines: Vec<EngineScore>) -> EngineComparison {
    let suggested_winner = if engines.len() >= 2 {
        engines
            .iter()
            .max_by(|a, b| rank(a).cmp(&rank(b)))
            .map(|e| e.engine.clone())
    } else {
        None
    };
    let rationale = describe(suggested_winner.as_deref());

    EngineComparison {
        engines,
        suggested_winner,
        rationale,
    }
}

/// A comparable key ordering engines by the priority above. Recall and the
/// hallucination rate are compared at a coarse resolution (nearest 0.1%)
/// before RTF, so float jitter can't flip the decision on an
/// effectively-tied recall or silence score.
fn rank(score: &EngineScore) -> (i64, i64, i64) {
    let recall = score.quality.proper_nouns.micro_recall.unwrap_or(0.0);
    let recall_key = (recall * 1000.0).round() as i64;

    // The crosstalk-excluding, correctly-aggregated silence-safety metric —
    // pure hallucinations only, mic-bleed bleed doesn't count against an
    // engine (see `SilenceScore::hallucinated_words_per_silent_minute`).
    let hallucination_rate = score.quality.silence.hallucinated_words_per_silent_minute;
    // Fewer hallucinated words is better, so negate for a "higher is better" key.
    let phantom_key = -((hallucination_rate * 100.0).round() as i64);

    let rtf_key = (score.timing.real_time_factor * 100.0).round() as i64;

    (recall_key, phantom_key, rtf_key)
}

fn describe(winner: Option<&str>) -> String {
    match winner {
        Some(name) => format!(
            "{name} suggested by priority order: proper-noun recall, then silence-safety, then speed."
        ),
        None => "not enough engine scores to compare (need at least two)".to_owned(),
    }
}

/// Renders `comparison` as an aggregate-only markdown table — no raw
/// transcript text, safe to paste into a committed results doc.
pub fn render_markdown(comparison: &EngineComparison) -> String {
    let mut out = String::new();
    out.push_str("| Engine | Proper-noun recall | Hallucinated words/silent-min | RTF | WER |\n");
    out.push_str("|---|---|---|---|---|\n");
    for engine in &comparison.engines {
        let recall = engine
            .quality
            .proper_nouns
            .micro_recall
            .map(|r| format!("{:.1}%", r * 100.0))
            .unwrap_or_else(|| "n/a".to_owned());
        out.push_str(&format!(
            "| {} | {} | {:.2} | {:.2}x | {:.1}% |\n",
            engine.engine,
            recall,
            engine.quality.silence.hallucinated_words_per_silent_minute,
            engine.timing.real_time_factor,
            engine.quality.wer.wer * 100.0,
        ));
    }
    match &comparison.suggested_winner {
        Some(winner) => out.push_str(&format!(
            "\nSuggested default: **{winner}** — {}\n",
            comparison.rationale
        )),
        None => out.push_str(&format!("\n{}\n", comparison.rationale)),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::benchmark::{
        ProperNounScore, SilenceChannelScore, SilenceScore, TimingScore, WerScore,
    };
    use crate::transcription::Channel;

    fn score(engine: &str, recall: f64, hallucination_rate: f64, rtf: f64) -> EngineScore {
        EngineScore {
            engine: engine.to_owned(),
            quality: crate::benchmark::QualityScore {
                proper_nouns: ProperNounScore {
                    terms: Vec::new(),
                    micro_recall: Some(recall),
                    macro_recall: Some(recall),
                },
                silence: SilenceScore {
                    channels: vec![SilenceChannelScore {
                        channel: Channel::You,
                        silence_ms: 60_000,
                        phantom_segments: 0,
                        phantom_words: 0,
                        phantom_segments_mutual_silence: 0,
                        phantom_segments_crosstalk: 0,
                        phantom_words_mutual_silence: 0,
                        phantom_words_per_silent_minute: hallucination_rate,
                    }],
                    total_phantom_segments: 0,
                    total_phantom_words: 0,
                    total_phantom_segments_mutual_silence: 0,
                    total_phantom_words_mutual_silence: 0,
                    hallucinated_words_per_silent_minute: hallucination_rate,
                },
                wer: WerScore {
                    reference_tokens: 0,
                    substitutions: 0,
                    deletions: 0,
                    insertions: 0,
                    wer: 0.0,
                    per_channel: Vec::new(),
                },
            },
            timing: TimingScore::new(60_000, (60_000.0 / rtf) as u64),
            reference_digest: None,
        }
    }

    #[test]
    fn proper_noun_recall_dominates_a_better_rtf() {
        let fast_but_wrong = score("whisper", 0.70, 0.0, 3.0);
        let slower_but_accurate = score("parakeet", 0.95, 0.0, 1.0);

        let comparison = compare_engines(vec![fast_but_wrong, slower_but_accurate]);

        assert_eq!(comparison.suggested_winner, Some("parakeet".to_owned()));
    }

    #[test]
    fn silence_breaks_a_recall_tie() {
        let hallucinates = score("whisper", 0.90, 12.0, 1.0);
        let silence_safe = score("parakeet", 0.90, 0.0, 1.0);

        let comparison = compare_engines(vec![hallucinates, silence_safe]);

        assert_eq!(comparison.suggested_winner, Some("parakeet".to_owned()));
    }

    #[test]
    fn fewer_than_two_scores_has_no_suggestion() {
        let comparison = compare_engines(vec![score("parakeet", 0.9, 0.0, 1.0)]);

        assert_eq!(comparison.suggested_winner, None);
    }
}
