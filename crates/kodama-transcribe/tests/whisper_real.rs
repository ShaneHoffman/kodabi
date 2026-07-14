//! Integration tests that load a real whisper.cpp model and prove the
//! Done-when for this ticket: a real recording transcribes to timestamped
//! text through the [`TranscriptionEngine`] trait, and the engine accepts
//! an initial-prompt bias string.
//!
//! Every test is `#[ignore]` (mirrors `tests/parakeet_real.rs`) because it
//! needs a locally-downloaded ggml model (~1.6 GB for large-v3-turbo) that
//! is not committed to the repo and not available in CI. Run locally after
//! downloading `ggml-large-v3-turbo.bin` from
//! <https://huggingface.co/ggerganov/whisper.cpp>, pointing the env var
//! below at it:
//!
//! ```text
//! WHISPER_MODEL=... cargo test -p kodama-transcribe --features whisper-cuda -- --ignored
//! ```
//!
//! (Use `--features whisper` instead for a CPU-only run.)
//!
//! There is deliberately no silence-safety test here: whisper.cpp has no
//! bundled VAD and is known to hallucinate over silence. Pairing this
//! engine with Silero VAD — and proving silence-safety — is
//! `feat/silero-vad-whisper`, not this ticket.

#![cfg(feature = "whisper")]

use std::path::PathBuf;

use kodama_core::glossary::{Glossary, GlossaryTerm, OnConflict};
use kodama_core::transcription::{
    apply_glossary_bias, transcribe_all, TranscriptionEngine, TranscriptionError,
};
use kodama_transcribe::{WhisperConfig, WhisperEngine};

const SAMPLE_RATE: u32 = 16_000;

fn env_path(var: &str) -> PathBuf {
    std::env::var(var)
        .unwrap_or_else(|_| panic!("set {var} to a real model file path to run this test"))
        .into()
}

fn real_config() -> WhisperConfig {
    WhisperConfig {
        model: env_path("WHISPER_MODEL"),
        use_gpu: true,
        num_threads: 4,
        language: Some("en".to_owned()),
    }
}

fn read_speech_wav() -> Vec<f32> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/speech_16k_mono.wav"
    );
    let mut reader = hound::WavReader::open(path).expect("test wav should be readable");
    assert_eq!(reader.spec().sample_rate, SAMPLE_RATE);
    assert_eq!(reader.spec().channels, 1);
    reader
        .samples::<i16>()
        .map(|s| f32::from(s.expect("valid sample")) / f32::from(i16::MAX))
        .collect()
}

#[test]
#[ignore = "requires a real whisper.cpp model file (set WHISPER_MODEL env var)"]
fn bad_model_path_yields_model_load_error() {
    let cfg = WhisperConfig {
        model: PathBuf::from("does-not-exist/ggml-large-v3-turbo.bin"),
        use_gpu: true,
        num_threads: 4,
        language: Some("en".to_owned()),
    };

    match WhisperEngine::new(cfg) {
        Err(err) => assert!(matches!(err, TranscriptionError::ModelLoad(_))),
        Ok(_) => panic!("expected ModelLoad for a nonexistent model file"),
    }
}

#[test]
#[ignore = "requires a real whisper.cpp model file (set WHISPER_MODEL env var)"]
fn transcribes_a_real_recording_through_the_trait() {
    let mut engine = WhisperEngine::new(real_config()).expect("engine should load");
    let samples = read_speech_wav();

    // Drive it as `&mut dyn TranscriptionEngine`, exactly like the future
    // distill pipeline will.
    let segments =
        transcribe_all(&mut engine, &samples, SAMPLE_RATE).expect("transcription succeeds");

    assert!(
        !segments.is_empty(),
        "a real recording should yield at least one segment"
    );
    let mut prev_end = 0u64;
    for segment in &segments {
        assert!(
            !segment.text.trim().is_empty(),
            "segment text should not be empty"
        );
        assert!(
            segment.start_ms <= segment.end_ms,
            "segment timestamps should be ordered"
        );
        assert!(segment.start_ms >= prev_end, "segments should not overlap");
        prev_end = segment.end_ms;
    }
}

#[test]
#[ignore = "requires a real whisper.cpp model file (set WHISPER_MODEL env var)"]
fn accepts_an_initial_prompt() {
    let mut engine = WhisperEngine::new(real_config()).expect("engine should load");
    engine
        .set_bias(&["MERIDIAN".to_owned(), "TeeTrack".to_owned()])
        .expect("set_bias succeeds");

    let samples = read_speech_wav();
    let segments =
        transcribe_all(&mut engine, &samples, SAMPLE_RATE).expect("transcription succeeds");

    assert!(
        !segments.is_empty(),
        "transcription with an initial-prompt bias should still yield segments"
    );
}

/// Proves the full project-glossary → engine-bias → transcription path: a
/// glossary persisted on disk is loaded by [`apply_glossary_bias`], applied as
/// the engine's initial-prompt bias, and the biased engine still transcribes
/// the recording to well-formed segments.
///
/// This deliberately does *not* assert the model recognizes a specific proper
/// noun. The shared `speech_16k_mono.wav` fixture doesn't utter the glossary
/// terms, and Whisper's exact spelling/casing of any term isn't contractual,
/// so a content assertion here would be either always-false or flaky. The
/// runnable coverage of how terms are turned into a prompt (join order,
/// blank-term dropping, NUL stripping) lives in `whisper::tests::build_bias_*`,
/// which run without a model.
#[test]
#[ignore = "requires a real whisper.cpp model file (set WHISPER_MODEL env var)"]
fn applies_a_glossary_from_a_project_dir_as_bias() {
    let project_dir = tempfile::tempdir().expect("temp project dir");
    let mut glossary = Glossary::default();
    glossary
        .upsert(
            GlossaryTerm {
                term: "MERIDIAN".to_owned(),
                definition: "A regional systems-migration project.".to_owned(),
                aliases: Vec::new(),
            },
            OnConflict::Error,
        )
        .expect("upsert succeeds");
    glossary
        .upsert(
            GlossaryTerm {
                term: "TeeTrack".to_owned(),
                definition: "Tee-sheet / POS vendor.".to_owned(),
                aliases: Vec::new(),
            },
            OnConflict::Error,
        )
        .expect("upsert succeeds");
    glossary.save(project_dir.path()).expect("glossary saves");

    let samples = read_speech_wav();

    let mut engine = WhisperEngine::new(real_config()).expect("engine should load");
    apply_glossary_bias(&mut engine, project_dir.path()).expect("glossary bias applies");
    let segments =
        transcribe_all(&mut engine, &samples, SAMPLE_RATE).expect("biased transcription succeeds");

    assert!(
        !segments.is_empty(),
        "a glossary-biased transcription should still yield segments"
    );
    for segment in &segments {
        assert!(
            !segment.text.trim().is_empty(),
            "segment text should not be empty"
        );
    }
}
