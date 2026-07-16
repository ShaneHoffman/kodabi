//! Proper-noun accuracy — the benchmark's headline metric
//! (`crates/kodabi-transcribe/tests/data/benchmark/README.md`): a mangled
//! project/client/teammate name poisons downstream categorization and
//! search, so this outranks raw word accuracy in the engine decision.

use serde::{Deserialize, Serialize};

use crate::glossary::Glossary;
use crate::raw_session::TranscriptSegment;

use super::text::{count_variant_occurrences, normalize_tokens};

/// Per-term recall: how many of a glossary term's spoken occurrences (by its
/// canonical spelling or any alias) survived transcription.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProperNounTermScore {
    pub term: String,
    pub reference_occurrences: usize,
    pub hypothesis_hits: usize,
    /// `None` when the term was never spoken in the reference — nothing to
    /// score, so it's excluded from the aggregate below rather than counted
    /// as a win or a loss.
    pub recall: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProperNounScore {
    pub terms: Vec<ProperNounTermScore>,
    /// Occurrence-weighted recall across all ref-present terms — the
    /// headline number: `Σ min(hits, ref) / Σ ref`.
    pub micro_recall: Option<f64>,
    /// Unweighted mean of each ref-present term's own recall — surfaces a
    /// rare term mangled every time even if it's a small share of total
    /// occurrences.
    pub macro_recall: Option<f64>,
}

/// Scores proper-noun recall of `hypothesis` against `reference`, for every
/// term in `glossary`.
///
/// A term's *canonical group* is its own spelling plus every alias —
/// `reference` and `hypothesis` occurrences of any spelling in the group all
/// count toward the same term, because aliases exist precisely to declare
/// "these spellings are the same proper noun" (mirrors
/// `GlossaryTerm::matches`). A spelling variant that isn't enumerated as an
/// alias (e.g. a hyphenation the glossary author didn't anticipate) won't
/// fold in — that's a fixture-authoring gap, not a scorer bug.
///
/// Channel-agnostic: a name is a name on either side of the conversation.
pub fn proper_noun_accuracy(
    reference: &[TranscriptSegment],
    hypothesis: &[TranscriptSegment],
    glossary: &Glossary,
) -> ProperNounScore {
    let reference_tokens = concat_tokens(reference);
    let hypothesis_tokens = concat_tokens(hypothesis);

    let terms: Vec<ProperNounTermScore> = glossary
        .terms()
        .iter()
        .map(|entry| {
            let variants: Vec<Vec<String>> = std::iter::once(entry.term.as_str())
                .chain(entry.aliases.iter().map(String::as_str))
                .map(normalize_tokens)
                .filter(|v| !v.is_empty())
                .collect();

            // A single non-overlapping scan over all variants at once, rather
            // than an independent per-variant count summed together: the latter
            // double-counts a spoken occurrence wherever one variant is a
            // subsequence of another (canonical `"Acme"` + alias `"Acme Corp"`).
            let reference_occurrences = count_variant_occurrences(&reference_tokens, &variants);
            let hypothesis_hits = count_variant_occurrences(&hypothesis_tokens, &variants);
            let recall = (reference_occurrences > 0).then(|| {
                hypothesis_hits.min(reference_occurrences) as f64 / reference_occurrences as f64
            });

            ProperNounTermScore {
                term: entry.term.clone(),
                reference_occurrences,
                hypothesis_hits,
                recall,
            }
        })
        .collect();

    let (matched_sum, ref_sum) = terms.iter().fold((0usize, 0usize), |(matched, refs), t| {
        (
            matched + t.hypothesis_hits.min(t.reference_occurrences),
            refs + t.reference_occurrences,
        )
    });
    let micro_recall = (ref_sum > 0).then(|| matched_sum as f64 / ref_sum as f64);

    let present: Vec<f64> = terms.iter().filter_map(|t| t.recall).collect();
    let macro_recall =
        (!present.is_empty()).then(|| present.iter().sum::<f64>() / present.len() as f64);

    ProperNounScore {
        terms,
        micro_recall,
        macro_recall,
    }
}

fn concat_tokens(segments: &[TranscriptSegment]) -> Vec<String> {
    segments
        .iter()
        .flat_map(|s| normalize_tokens(&s.text))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glossary::{GlossaryTerm, OnConflict};
    use crate::transcription::Channel;

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

    fn glossary_with(term: &str, aliases: &[&str]) -> Glossary {
        let mut glossary = Glossary::default();
        glossary
            .upsert(
                GlossaryTerm {
                    term: term.to_owned(),
                    definition: String::new(),
                    aliases: aliases.iter().map(|s| s.to_string()).collect(),
                },
                OnConflict::Error,
            )
            .unwrap();
        glossary
    }

    #[test]
    fn partial_recall_is_hits_over_reference_occurrences() {
        let glossary = glossary_with("OKIES", &[]);
        let reference = vec![
            segment(0, Channel::You, "talk about OKIES today"),
            segment(1, Channel::You, "OKIES again"),
        ];
        let hypothesis = vec![segment(0, Channel::You, "talk about OKIES today")];

        let score = proper_noun_accuracy(&reference, &hypothesis, &glossary);

        assert_eq!(score.terms[0].reference_occurrences, 2);
        assert_eq!(score.terms[0].hypothesis_hits, 1);
        assert_eq!(score.terms[0].recall, Some(0.5));
        assert_eq!(score.micro_recall, Some(0.5));
    }

    #[test]
    fn alias_hit_folds_into_the_canonical_term() {
        let glossary = glossary_with("ForeUp", &["4up"]);
        let reference = vec![segment(0, Channel::You, "we use ForeUp daily")];
        // Mistranscribed as the alias spelling.
        let hypothesis = vec![segment(0, Channel::You, "we use 4up daily")];

        let score = proper_noun_accuracy(&reference, &hypothesis, &glossary);

        assert_eq!(score.terms[0].hypothesis_hits, 1);
        assert_eq!(score.terms[0].recall, Some(1.0));
    }

    #[test]
    fn multi_word_phrase_matches_only_as_an_adjacent_pair() {
        let glossary = glossary_with("Acme Corp", &[]);
        let reference = vec![segment(0, Channel::You, "we work with Acme Corp")];
        let scattered = vec![segment(0, Channel::You, "Acme is not a Corp")];

        let hit_score = proper_noun_accuracy(&reference, &reference, &glossary);
        let miss_score = proper_noun_accuracy(&reference, &scattered, &glossary);

        assert_eq!(hit_score.terms[0].hypothesis_hits, 1);
        assert_eq!(miss_score.terms[0].hypothesis_hits, 0);
    }

    #[test]
    fn hyphenation_variant_only_matches_when_enumerated_as_an_alias() {
        let reference = vec![segment(0, Channel::You, "ForeUp is the vendor")];
        let hypothesis = vec![segment(0, Channel::You, "Fore-Up is the vendor")];

        let without_alias = glossary_with("ForeUp", &[]);
        let with_alias = glossary_with("ForeUp", &["Fore-Up"]);

        let missed = proper_noun_accuracy(&reference, &hypothesis, &without_alias);
        let matched = proper_noun_accuracy(&reference, &hypothesis, &with_alias);

        assert_eq!(missed.terms[0].hypothesis_hits, 0);
        assert_eq!(matched.terms[0].hypothesis_hits, 1);
    }

    #[test]
    fn ref_absent_term_has_no_recall_and_is_excluded_from_aggregates() {
        let glossary = glossary_with("Ghostwriter", &[]);
        let reference = vec![segment(0, Channel::You, "nothing relevant here")];
        let hypothesis = reference.clone();

        let score = proper_noun_accuracy(&reference, &hypothesis, &glossary);

        assert_eq!(score.terms[0].recall, None);
        assert_eq!(score.micro_recall, None);
        assert_eq!(score.macro_recall, None);
    }

    #[test]
    fn empty_glossary_yields_no_scores() {
        let glossary = Glossary::default();
        let reference = vec![segment(0, Channel::You, "hello world")];

        let score = proper_noun_accuracy(&reference, &reference, &glossary);

        assert!(score.terms.is_empty());
        assert_eq!(score.micro_recall, None);
    }

    #[test]
    fn overlapping_variants_count_a_spoken_occurrence_once() {
        // The alias contains the canonical term, so summing an independent
        // per-variant count would report two occurrences for one utterance.
        let glossary = glossary_with("Acme", &["Acme Corp"]);
        let reference = vec![segment(0, Channel::You, "we signed Acme Corp today")];
        let hypothesis = reference.clone();

        let score = proper_noun_accuracy(&reference, &hypothesis, &glossary);

        assert_eq!(score.terms[0].reference_occurrences, 1);
        assert_eq!(score.terms[0].hypothesis_hits, 1);
        assert_eq!(score.terms[0].recall, Some(1.0));
    }

    #[test]
    fn over_emission_is_capped_at_full_recall() {
        let glossary = glossary_with("OKIES", &[]);
        let reference = vec![segment(0, Channel::You, "OKIES")];
        let hypothesis = vec![segment(0, Channel::You, "OKIES OKIES OKIES")];

        let score = proper_noun_accuracy(&reference, &hypothesis, &glossary);

        assert_eq!(score.terms[0].recall, Some(1.0));
        assert_eq!(score.micro_recall, Some(1.0));
    }
}
