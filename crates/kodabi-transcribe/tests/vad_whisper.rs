//! Integration tests that load real whisper.cpp + Silero VAD models and prove
//! the Done-when for `feat/silero-vad-whisper`: silence never reaches
//! whisper.cpp (no phantom text), and a pause-heavy recording still
//! transcribes to correctly-ordered, session-clock timestamps.
//!
//! Every test is `#[ignore]` (mirrors `tests/whisper_real.rs` and
//! `tests/parakeet_real.rs`) because it needs locally-downloaded model files
//! not committed to the repo and not available in CI. Run locally after
//! downloading `ggml-large-v3-turbo.bin`
//! (<https://huggingface.co/ggerganov/whisper.cpp>) and `silero_vad.onnx`
//! (<https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx>),
//! pointing the env vars below at them:
//!
//! ```text
//! WHISPER_MODEL=... VAD_MODEL=... cargo test -p kodabi-transcribe --features whisper -- --ignored
//! ```
//!
//! The `whisper` feature enables `vad` (see Cargo.toml), which builds
//! sherpa-onnx in its `shared` link mode instead of the `static` mode
//! `parakeet` uses — whisper-rs's CMake build of whisper.cpp links the MSVC
//! C++ runtime dynamically, and combining that with sherpa-onnx's normally
//! statically-linked runtime trips a `link.exe` CRT mismatch. `shared` mode's
//! build script copies its runtime DLLs (`onnxruntime.dll`,
//! `sherpa-onnx-c-api.dll`, ...) next to the build output in `target/debug`,
//! which is where a real `cargo run`/Tauri binary lives — but `cargo test`
//! places *this* file's compiled test binary one level deeper, in
//! `target/debug/deps`, so it won't find them there and instead falls back to
//! an incompatible system-wide `onnxruntime.dll` if one is present (a version
//! mismatch that surfaces as an access violation, not a linker or compiler
//! error). This crate's `build.rs` now mirrors those DLLs from `target/<profile>`
//! into `target/<profile>/deps` on Windows whenever the `whisper` feature is
//! enabled, so the test binary finds the correct `onnxruntime.dll` in its own
//! directory (DLL search position #1, ahead of the System32 copy) with no manual
//! step. That mirror is best-effort; if it was skipped (e.g. the source DLLs were
//! not present when the build script ran), fall back to copying them down a level
//! once:
//!
//! ```text
//! cp target/debug/{onnxruntime,onnxruntime_providers_shared,sherpa-onnx-c-api,sherpa-onnx-cxx-api}*.dll target/debug/deps/
//! ```

#![cfg(feature = "whisper")]

use std::path::PathBuf;

use kodabi_core::transcription::{transcribe_all, AudioChunk, TranscriptionEngine};
use kodabi_transcribe::{whisper_with_vad, VadConfig, WhisperConfig};

const SAMPLE_RATE: u32 = 16_000;

fn env_path(var: &str) -> PathBuf {
    std::env::var(var)
        .unwrap_or_else(|_| panic!("set {var} to a real model file path to run this test"))
        .into()
}

fn real_whisper_config() -> WhisperConfig {
    WhisperConfig {
        model: env_path("WHISPER_MODEL"),
        use_gpu: true,
        num_threads: 4,
        language: Some("en".to_owned()),
    }
}

fn real_vad_config() -> VadConfig {
    VadConfig {
        vad_model: env_path("VAD_MODEL"),
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
#[ignore = "requires real whisper.cpp + Silero VAD model files (set WHISPER_MODEL, VAD_MODEL env vars)"]
fn silence_yields_no_segments() {
    let mut engine =
        whisper_with_vad(real_whisper_config(), real_vad_config()).expect("engine should load");

    // Two seconds of digital silence: the VAD should never finalise a speech
    // segment, so whisper.cpp is never invoked and nothing is emitted.
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

#[test]
#[ignore = "requires real whisper.cpp + Silero VAD model files (set WHISPER_MODEL, VAD_MODEL env vars)"]
fn chunked_feed_recovers_speech_after_leading_silence() {
    let mut engine =
        whisper_with_vad(real_whisper_config(), real_vad_config()).expect("engine should load");
    let speech = read_speech_wav();

    // 3s silence + speech + 1s silence + speech (~16.2s): an utterance that
    // starts well before the end of the first 10s pipeline chunk, plus a
    // second one in the next chunk. Regression coverage for the window-feed
    // bug where whole-chunk `accept_waveform` calls collapsed the VAD's
    // per-window segment state machine into per-call decisions: every capture
    // "started" near the tail of the first chunk (~9.7s) and nothing
    // finalised before flush (see `silero::feed_windowed`). Mirrors the same
    // test in `tests/parakeet_real.rs`, through `VadGate<WhisperEngine>`.
    let silence_3s = vec![0.0f32; SAMPLE_RATE as usize * 3];
    let silence_1s = vec![0.0f32; SAMPLE_RATE as usize];
    let mut samples = silence_3s;
    samples.extend_from_slice(&speech);
    samples.extend_from_slice(&silence_1s);
    samples.extend_from_slice(&speech);

    // Feed 10s chunks, exactly like the capture pipeline
    // (`IN_MEMORY_CHUNK_SAMPLES` in `src-tauri/src/transcribe.rs`). 160,000
    // is not a multiple of the VAD's 512-sample window, so this also
    // exercises the partial-window carry across `accept` calls.
    const CHUNK_SAMPLES: usize = 10 * SAMPLE_RATE as usize;
    let mut segments = Vec::new();
    for chunk in samples.chunks(CHUNK_SAMPLES) {
        segments.extend(
            engine
                .accept(AudioChunk {
                    samples: chunk,
                    sample_rate: SAMPLE_RATE,
                })
                .expect("accept succeeds"),
        );
    }
    segments.extend(engine.finish().expect("finish succeeds"));

    assert!(
        segments.len() >= 2,
        "both utterances should finalise as separate segments, got {segments:?}"
    );
    let first_start = segments[0].start_ms;
    assert!(
        (2000..=5000).contains(&first_start),
        "the first segment should start near the 3s speech onset, got {first_start}ms"
    );
    assert!(
        segments.iter().any(|s| s.start_ms >= 9800),
        "the second utterance (from ~10.1s) should be found, got {segments:?}"
    );
    // Looser than the parakeet mirror's 15,000: the last end comes from
    // whisper's own inner timestamps mapped via `offset_into_span`, which are
    // sloppier than the VAD span edge parakeet reports.
    let last_end = segments.last().expect("segments is non-empty").end_ms;
    assert!(
        last_end > 14_500,
        "the last segment should reach the end of the second utterance, got {last_end}ms"
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
        assert!(
            segment.start_ms >= prev_end,
            "segments should not overlap on the session clock"
        );
        prev_end = segment.end_ms;
    }
}

#[test]
#[ignore = "requires real whisper.cpp + Silero VAD model files (set WHISPER_MODEL, VAD_MODEL env vars)"]
fn transcribes_a_real_recording_with_correct_timestamps() {
    let mut engine =
        whisper_with_vad(real_whisper_config(), real_vad_config()).expect("engine should load");
    let samples = read_speech_wav();

    // Drive it as `&mut dyn TranscriptionEngine`, exactly like the future
    // distill pipeline will — the VAD gate is transparent to the caller.
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
        assert!(
            segment.start_ms >= prev_end,
            "segments should not overlap on the session clock"
        );
        prev_end = segment.end_ms;
    }
}
