//! `#[ignore]`d RTF (real-time factor) harness for the resource-budget
//! ticket (FOUNDING_DOC §3.7, `docs/RESOURCE_BUDGET.md`) — loads a real
//! engine against the committed `speech_16k_mono.wav` fixture and prints
//! wall time + `speed_x` (audio ÷ wall; >1.0 runs faster than realtime).
//!
//! A real meeting's numbers still require running on the target machine
//! (see `docs/RESOURCE_BUDGET.md`), but this gives a quick, repeatable proxy
//! that doesn't need a live recording. Model-gated and `#[ignore]`d like
//! `parakeet_real.rs`/`whisper_real.rs`; run locally:
//!
//! ```text
//! # Parakeet:
//! PARAKEET_ENCODER=... PARAKEET_DECODER=... PARAKEET_JOINER=... \
//! PARAKEET_TOKENS=... PARAKEET_VAD_MODEL=... \
//! cargo test -p kodabi-transcribe --features parakeet -- --ignored --nocapture rtf
//!
//! # Whisper:
//! WHISPER_MODEL=... VAD_MODEL=... \
//! cargo test -p kodabi-transcribe --features whisper -- --ignored --nocapture rtf
//! ```
//!
//! Every env-overridable knob from `docs/RESOURCE_BUDGET.md` (e.g.
//! `KODABI_PARAKEET_THREADS`, `KODABI_WHISPER_THREADS`) applies here too via
//! each config's `apply_env_overrides`, so a tuning pass can iterate without
//! recompiling.

#![cfg(any(feature = "parakeet", feature = "whisper"))]

use std::path::PathBuf;
use std::time::Instant;

use kodabi_core::metrics::real_time_factor;
use kodabi_core::transcription::{transcribe_all, TranscriptionEngine};

const SAMPLE_RATE: u32 = 16_000;

fn env_path(var: &str) -> PathBuf {
    std::env::var(var)
        .unwrap_or_else(|_| panic!("set {var} to a real model file path to run this test"))
        .into()
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

/// Runs `engine` over `samples` once, timing the whole call, and prints the
/// resulting `speed_x` — the number to record in `docs/RESOURCE_BUDGET.md`.
fn run_and_report(mut engine: impl TranscriptionEngine, samples: &[f32]) {
    let audio_secs = samples.len() as f64 / SAMPLE_RATE as f64;
    let start = Instant::now();
    let segments =
        transcribe_all(&mut engine, samples, SAMPLE_RATE).expect("transcription succeeds");
    let wall_secs = start.elapsed().as_secs_f64();

    let speed_x = real_time_factor(audio_secs, wall_secs);
    println!(
        "audio={audio_secs:.2}s wall={wall_secs:.2}s speed_x={speed_x:.2} segments={}",
        segments.len()
    );
    assert!(
        !segments.is_empty(),
        "a real recording should yield at least one segment"
    );
}

#[cfg(feature = "parakeet")]
#[test]
#[ignore = "requires real Parakeet + Silero VAD model files (set PARAKEET_* env vars)"]
fn parakeet_rtf_on_the_committed_fixture() {
    use kodabi_transcribe::{ParakeetConfig, ParakeetEngine};

    let config = ParakeetConfig {
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
    .apply_env_overrides();

    let engine = ParakeetEngine::new(config).expect("engine should load");
    run_and_report(engine, &read_speech_wav());
}

#[cfg(feature = "whisper")]
#[test]
#[ignore = "requires real whisper.cpp + Silero VAD model files (set WHISPER_MODEL/VAD_MODEL env vars)"]
fn whisper_rtf_on_the_committed_fixture() {
    use kodabi_transcribe::{whisper_with_vad, VadConfig, WhisperConfig};

    let whisper_config = WhisperConfig {
        model: env_path("WHISPER_MODEL"),
        use_gpu: true,
        num_threads: 4,
        language: Some("en".to_owned()),
    }
    .apply_env_overrides();
    let vad_config = VadConfig {
        vad_model: env_path("VAD_MODEL"),
        num_threads: 1,
        provider: Some("cpu".to_owned()),
        vad_threshold: 0.5,
        min_silence_duration: 0.25,
        min_speech_duration: 0.25,
        max_speech_duration: 20.0,
        debug: false,
    }
    .apply_env_overrides();

    let engine = whisper_with_vad(whisper_config, vad_config).expect("engine should load");
    run_and_report(engine, &read_speech_wav());
}
