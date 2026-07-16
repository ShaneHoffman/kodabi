//! Benchmark scoring for the STT-engine comparison (FOUNDING_DOC §3.4, §7 —
//! "Default STT engine"). Pure and engine-agnostic: takes an already-
//! assembled hypothesis transcript (produced by running a
//! [`crate::transcription::TranscriptionEngine`] over the recorded fixture)
//! and a reference transcript, and scores it against the criteria named in
//! the benchmark's manifest
//! (`crates/kodabi-transcribe/tests/data/benchmark/README.md`): proper-noun
//! accuracy (headline), silence/phantom-text behavior, speed, and overall
//! word error rate (supporting).
//!
//! No result type here ever carries raw transcript text: only counts, rates,
//! and the glossary's own canonical terms, so a serialized [`EngineScore`]
//! (written to the git-ignored fixture directory by the per-engine runner in
//! `kodabi-transcribe`) never leaks real-meeting content.
//!
//! The actual native engines live in `kodabi-transcribe` (this crate stays
//! FFI-free); construction happens there and this module only ever scores
//! already-produced [`crate::raw_session::TranscriptSegment`] output.

mod compare;
mod proper_nouns;
mod silence;
mod text;
mod wer;

pub use compare::{compare_engines, render_markdown, EngineComparison};
pub use proper_nouns::{proper_noun_accuracy, ProperNounScore, ProperNounTermScore};
pub use silence::{silence_behavior, SilenceChannelScore, SilenceScore};
pub use wer::{word_error_rate, WerChannelScore, WerScore};

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::glossary::Glossary;
use crate::raw_session::TranscriptSegment;
use crate::transcription::Channel;

/// Everything [`score_quality`] needs: the two transcripts to compare, the
/// glossary defining the scored proper nouns, and each channel's true
/// recorded duration (see [`silence_behavior`] on why the true length
/// matters, not just the last reference timestamp).
pub struct BenchmarkInputs<'a> {
    pub reference: &'a [TranscriptSegment],
    pub hypothesis: &'a [TranscriptSegment],
    pub glossary: &'a Glossary,
    pub channel_durations: &'a [(Channel, u64)],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualityScore {
    pub proper_nouns: ProperNounScore,
    pub silence: SilenceScore,
    pub wer: WerScore,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimingScore {
    pub audio_duration_ms: u64,
    pub processing_ms: u64,
    /// `audio_duration_ms / processing_ms` — greater than 1.0 means faster
    /// than real time. `0.0` when `processing_ms` is `0` (unmeasured).
    pub real_time_factor: f64,
}

impl TimingScore {
    pub fn new(audio_duration_ms: u64, processing_ms: u64) -> Self {
        let real_time_factor = if processing_ms > 0 {
            audio_duration_ms as f64 / processing_ms as f64
        } else {
            0.0
        };
        Self {
            audio_duration_ms,
            processing_ms,
            real_time_factor,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineScore {
    pub engine: String,
    pub quality: QualityScore,
    pub timing: TimingScore,
    /// Short fingerprint of the reference transcript used (e.g. a sha256
    /// prefix), so two result files can be confirmed to have scored against
    /// the same fixture without embedding the transcript itself.
    pub reference_digest: Option<String>,
}

/// Runs every quality metric over `inputs`. Speed is scored separately via
/// [`TimingScore::new`], since only the (native) runner can measure wall
/// clock — this function stays pure and fixture-free.
pub fn score_quality(inputs: BenchmarkInputs<'_>) -> QualityScore {
    QualityScore {
        proper_nouns: proper_noun_accuracy(inputs.reference, inputs.hypothesis, inputs.glossary),
        silence: silence_behavior(
            inputs.reference,
            inputs.hypothesis,
            inputs.channel_durations,
        ),
        wer: word_error_rate(inputs.reference, inputs.hypothesis),
    }
}

/// Failure reading/writing benchmark fixture or result files.
#[derive(Debug, thiserror::Error)]
pub enum BenchmarkIoError {
    #[error("benchmark I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("benchmark YAML error at {path}: {source}")]
    Yaml {
        path: PathBuf,
        #[source]
        source: serde_yaml_ng::Error,
    },
    #[error("benchmark JSON error at {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// Reads the benchmark fixture's `glossary.yml` (**not**
/// [`crate::glossary::GLOSSARY_FILE`] / `_glossary.yml` — the fixture uses a
/// plain, undecorated filename per the manifest, so `Glossary::load` can't be
/// reused here).
pub fn read_glossary_file(path: &Path) -> Result<Glossary, BenchmarkIoError> {
    let raw = fs::read_to_string(path).map_err(|source| BenchmarkIoError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_yaml_ng::from_str(&raw).map_err(|source| BenchmarkIoError::Yaml {
        path: path.to_path_buf(),
        source,
    })
}

/// Writes `score` as `results_<engine>.json` in `dir`, returning the path
/// written. `dir` is expected to be the (git-ignored) benchmark fixture
/// directory — see the module docs on why this is safe to call freely.
pub fn write_engine_score(dir: &Path, score: &EngineScore) -> Result<PathBuf, BenchmarkIoError> {
    let path = dir.join(format!("results_{}.json", score.engine));
    let json = serde_json::to_string_pretty(score).map_err(|source| BenchmarkIoError::Json {
        path: path.clone(),
        source,
    })?;
    fs::write(&path, json).map_err(|source| BenchmarkIoError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

/// Writes the assembled hypothesis transcript to `hypothesis_<engine>.jsonl`
/// in `dir`, in the same JSONL shape as `reference.jsonl`, for local
/// word-level error analysis (which words an engine missed vs. filler it
/// cleaned up). Contains real transcript text, so — like the audio and
/// `reference.jsonl` — it stays in the git-ignored fixture directory and is
/// never committed; the scored [`EngineScore`] remains the only text-free,
/// shareable artifact.
pub fn write_hypothesis(
    dir: &Path,
    engine: &str,
    hypothesis: &[TranscriptSegment],
) -> Result<PathBuf, BenchmarkIoError> {
    let path = dir.join(format!("hypothesis_{engine}.jsonl"));
    let mut body = String::new();
    for segment in hypothesis {
        let line = serde_json::to_string(segment).map_err(|source| BenchmarkIoError::Json {
            path: path.clone(),
            source,
        })?;
        body.push_str(&line);
        body.push('\n');
    }
    fs::write(&path, body).map_err(|source| BenchmarkIoError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

/// Reads back an [`EngineScore`] written by [`write_engine_score`].
pub fn read_engine_score(path: &Path) -> Result<EngineScore, BenchmarkIoError> {
    let raw = fs::read_to_string(path).map_err(|source| BenchmarkIoError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&raw).map_err(|source| BenchmarkIoError::Json {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample_score() -> EngineScore {
        EngineScore {
            engine: "parakeet".to_owned(),
            quality: QualityScore {
                proper_nouns: ProperNounScore {
                    terms: Vec::new(),
                    micro_recall: Some(0.9),
                    macro_recall: Some(0.9),
                },
                silence: SilenceScore {
                    channels: Vec::new(),
                    total_phantom_segments: 0,
                    total_phantom_words: 0,
                    total_phantom_segments_mutual_silence: 0,
                    total_phantom_words_mutual_silence: 0,
                    hallucinated_words_per_silent_minute: 0.0,
                },
                wer: WerScore {
                    reference_tokens: 100,
                    substitutions: 5,
                    deletions: 1,
                    insertions: 0,
                    wer: 0.06,
                    per_channel: Vec::new(),
                },
            },
            timing: TimingScore::new(180_000, 60_000),
            reference_digest: Some("abc123".to_owned()),
        }
    }

    #[test]
    fn engine_score_round_trips_through_json() {
        let dir = tempdir().unwrap();
        let score = sample_score();

        let path = write_engine_score(dir.path(), &score).unwrap();
        assert_eq!(path.file_name().unwrap(), "results_parakeet.json");

        let read_back = read_engine_score(&path).unwrap();
        assert_eq!(read_back, score);
    }

    #[test]
    fn timing_score_computes_real_time_factor() {
        let timing = TimingScore::new(180_000, 60_000);
        assert_eq!(timing.real_time_factor, 3.0);
    }

    #[test]
    fn timing_score_is_zero_when_unmeasured() {
        let timing = TimingScore::new(180_000, 0);
        assert_eq!(timing.real_time_factor, 0.0);
    }

    #[test]
    fn read_glossary_file_parses_the_manifests_documented_shape() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("glossary.yml");
        fs::write(
            &path,
            r#"
terms:
  - term: OKIES
    definition: ""
    aliases: []
"#,
        )
        .unwrap();

        let glossary = read_glossary_file(&path).unwrap();

        assert_eq!(glossary.terms().len(), 1);
        assert_eq!(glossary.terms()[0].term, "OKIES");
    }
}
