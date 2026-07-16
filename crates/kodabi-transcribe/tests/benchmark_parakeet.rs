//! Runs Parakeet TDT over the real-meeting benchmark fixture and scores it
//! against the reference transcript — the "Benchmark both engines" step of
//! FOUNDING_DOC §3.4/§7 ("Default STT engine"). See
//! `tests/data/benchmark/README.md` for how to produce the fixture.
//!
//! `#[ignore]`d and gated on `KODABI_BENCHMARK_MEETING_DIR` (mirrors
//! `PARAKEET_*` in `parakeet_real.rs`): needs the local fixture *and* real
//! Parakeet + Silero VAD model files, neither committed nor available in CI.
//!
//! ```text
//! KODABI_BENCHMARK_MEETING_DIR=... \
//! PARAKEET_ENCODER=... PARAKEET_DECODER=... PARAKEET_JOINER=... \
//! PARAKEET_TOKENS=... PARAKEET_VAD_MODEL=... \
//! cargo test -p kodabi-transcribe --features parakeet -- --ignored --nocapture benchmark
//! ```

#![cfg(feature = "parakeet")]

use std::path::{Path, PathBuf};
use std::time::Instant;

use kodabi_core::benchmark::{
    read_glossary_file, score_quality, write_engine_score, write_hypothesis, BenchmarkInputs,
    EngineScore, TimingScore,
};
use kodabi_core::raw_session::{assemble, read_raw_session};
use kodabi_core::transcription::{
    glossary_bias_terms, AudioChunk, Channel, Segment, TranscriptionEngine,
};
use kodabi_transcribe::{ParakeetConfig, ParakeetEngine};

const SAMPLE_RATE: u32 = 16_000;

/// Feed audio in ~1-second chunks. The VAD's internal circular buffer holds
/// only ~30 s (see `silero::build_silero_vad`), so handing it the whole
/// 3-minute recording in one `accept` overflows the buffer and drops nearly
/// every finalised segment before it can be drained — chunking (and draining
/// after each `accept`, as the trait does) mirrors how real capture streams
/// audio and keeps each feed well under that buffer.
const CHUNK_SAMPLES: usize = SAMPLE_RATE as usize;

fn env_path(var: &str) -> PathBuf {
    std::env::var(var)
        .unwrap_or_else(|_| panic!("set {var} to a real model file path to run this benchmark"))
        .into()
}

fn fixture_dir() -> Option<PathBuf> {
    std::env::var("KODABI_BENCHMARK_MEETING_DIR")
        .ok()
        .map(PathBuf::from)
}

fn parakeet_config() -> ParakeetConfig {
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

fn read_channel_wav(path: &Path) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path)
        .unwrap_or_else(|e| panic!("fixture wav {path:?} should be readable: {e}"));
    assert_eq!(
        reader.spec().sample_rate,
        SAMPLE_RATE,
        "{path:?} must be 16kHz"
    );
    assert_eq!(reader.spec().channels, 1, "{path:?} must be mono");
    reader
        .samples::<i16>()
        .map(|s| f32::from(s.expect("valid sample")) / f32::from(i16::MAX))
        .collect()
}

/// Transcribes one channel's samples through a fresh engine (avoids VAD
/// state bleeding between channels), feeding it in [`CHUNK_SAMPLES`] chunks
/// and draining after each. Times only the transcription work — model load is
/// excluded from the reported real-time factor.
fn transcribe_channel(samples: &[f32], glossary_terms: &[String]) -> (Vec<Segment>, u128) {
    let mut engine = ParakeetEngine::new(parakeet_config()).expect("Parakeet engine should load");
    engine
        .set_bias(glossary_terms)
        .expect("set_bias succeeds (no-op for Parakeet)");

    let start = Instant::now();
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
    (segments, start.elapsed().as_millis())
}

#[test]
#[ignore = "requires the local benchmark fixture (KODABI_BENCHMARK_MEETING_DIR) and real Parakeet + Silero VAD model files (PARAKEET_* env vars)"]
fn benchmark_parakeet_against_the_reference_transcript() {
    let dir = fixture_dir().expect("set KODABI_BENCHMARK_MEETING_DIR to run this benchmark");

    let you_samples = read_channel_wav(&dir.join("meeting_you_16k_mono.wav"));
    let them_samples = read_channel_wav(&dir.join("meeting_them_16k_mono.wav"));
    let you_ms = you_samples.len() as u64 * 1000 / u64::from(SAMPLE_RATE);
    let them_ms = them_samples.len() as u64 * 1000 / u64::from(SAMPLE_RATE);

    let glossary =
        read_glossary_file(&dir.join("glossary.yml")).expect("glossary.yml should parse");
    let bias_terms = glossary_bias_terms(&glossary);

    let (you_segments, you_elapsed_ms) = transcribe_channel(&you_samples, &bias_terms);
    let (them_segments, them_elapsed_ms) = transcribe_channel(&them_samples, &bias_terms);

    let hypothesis = assemble([(Channel::You, you_segments), (Channel::Them, them_segments)]);
    let reference =
        read_raw_session(&dir.join("reference.jsonl")).expect("reference.jsonl should parse");

    assert!(
        !hypothesis.is_empty(),
        "a real meeting should yield transcribed segments"
    );

    let channel_durations = [(Channel::You, you_ms), (Channel::Them, them_ms)];
    let quality = score_quality(BenchmarkInputs {
        reference: &reference,
        hypothesis: &hypothesis,
        glossary: &glossary,
        channel_durations: &channel_durations,
    });

    let timing = TimingScore::new(you_ms + them_ms, (you_elapsed_ms + them_elapsed_ms) as u64);
    assert!(timing.real_time_factor.is_finite() && timing.real_time_factor > 0.0);
    if let Some(recall) = quality.proper_nouns.micro_recall {
        assert!((0.0..=1.0).contains(&recall));
    }

    let score = EngineScore {
        engine: "parakeet".to_owned(),
        quality,
        timing,
        reference_digest: None,
    };

    println!("{score:#?}");
    let path = write_engine_score(&dir, &score).expect("results file should write");
    println!("wrote {}", path.display());
    let hyp_path =
        write_hypothesis(&dir, "parakeet", &hypothesis).expect("hypothesis file should write");
    println!("wrote {}", hyp_path.display());
}
