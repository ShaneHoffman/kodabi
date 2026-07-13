//! Integration tests that load real Parakeet + Silero VAD models and prove
//! the Done-when for this ticket: a real recording transcribes to
//! timestamped text through the [`TranscriptionEngine`] trait, and silence
//! produces no phantom text.
//!
//! Every test is `#[ignore]` (mirrors `crates/kodama-audio/tests/loopback.rs`)
//! because it needs ~630 MB of locally-downloaded ONNX model files that are
//! not committed to the repo and not available in CI. Run locally after
//! downloading `sherpa-onnx-nemo-parakeet-tdt-0.6b-v2` (int8) and
//! `silero_vad.onnx`, pointing the env vars below at them:
//!
//! ```text
//! PARAKEET_ENCODER=... PARAKEET_DECODER=... PARAKEET_JOINER=... \
//! PARAKEET_TOKENS=... PARAKEET_VAD_MODEL=... \
//! cargo test -p kodama-transcribe --features parakeet -- --ignored
//! ```

#![cfg(feature = "parakeet")]

use std::path::PathBuf;

use kodama_core::transcription::{
    transcribe_all, AudioChunk, TranscriptionEngine, TranscriptionError,
};
use kodama_transcribe::{ParakeetConfig, ParakeetEngine};

const SAMPLE_RATE: u32 = 16_000;

fn env_path(var: &str) -> PathBuf {
    std::env::var(var)
        .unwrap_or_else(|_| panic!("set {var} to a real model file path to run this test"))
        .into()
}

fn real_config() -> ParakeetConfig {
    ParakeetConfig {
        encoder: env_path("PARAKEET_ENCODER"),
        decoder: env_path("PARAKEET_DECODER"),
        joiner: env_path("PARAKEET_JOINER"),
        tokens: env_path("PARAKEET_TOKENS"),
        vad_model: env_path("PARAKEET_VAD_MODEL"),
        num_threads: 1,
        provider: Some("cpu".to_owned()),
        vad_threshold: 0.5,
        min_silence_duration: 0.25,
        min_speech_duration: 0.25,
        max_speech_duration: 20.0,
        debug: false,
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
#[ignore = "requires real Parakeet + Silero VAD model files (set PARAKEET_* env vars)"]
fn bad_model_paths_yield_model_load_error() {
    let cfg = ParakeetConfig {
        encoder: PathBuf::from("does-not-exist/encoder.onnx"),
        decoder: PathBuf::from("does-not-exist/decoder.onnx"),
        joiner: PathBuf::from("does-not-exist/joiner.onnx"),
        tokens: PathBuf::from("does-not-exist/tokens.txt"),
        vad_model: PathBuf::from("does-not-exist/silero_vad.onnx"),
        num_threads: 1,
        provider: Some("cpu".to_owned()),
        vad_threshold: 0.5,
        min_silence_duration: 0.25,
        min_speech_duration: 0.25,
        max_speech_duration: 20.0,
        debug: false,
    };

    match ParakeetEngine::new(cfg) {
        Err(err) => assert!(matches!(err, TranscriptionError::ModelLoad(_))),
        Ok(_) => panic!("expected ModelLoad for nonexistent model files"),
    }
}

#[test]
#[ignore = "requires real Parakeet + Silero VAD model files (set PARAKEET_* env vars)"]
fn transcribes_a_real_recording_through_the_trait() {
    let mut engine = ParakeetEngine::new(real_config()).expect("engine should load");
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
            segment.start_ms < segment.end_ms,
            "segment timestamps should be ordered"
        );
        assert!(segment.start_ms >= prev_end, "segments should not overlap");
        prev_end = segment.end_ms;
    }
}

#[test]
#[ignore = "requires real Parakeet + Silero VAD model files (set PARAKEET_* env vars)"]
fn silence_yields_no_segments() {
    let mut engine = ParakeetEngine::new(real_config()).expect("engine should load");

    // Two seconds of digital silence: the VAD should never finalise a speech
    // segment, so the recognizer is never invoked and nothing is emitted.
    let silence = vec![0.0f32; SAMPLE_RATE as usize * 2];

    let mut segments = engine
        .accept(AudioChunk {
            samples: &silence,
            sample_rate: SAMPLE_RATE,
        })
        .expect("accept succeeds");
    segments.extend(engine.finish().expect("finish succeeds"));

    assert!(segments.is_empty(), "silence must not produce phantom text");
}
