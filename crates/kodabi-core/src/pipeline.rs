//! Orchestrates the live-capture pipeline: transcribe each channel, merge
//! into one you/them transcript, run the glossary cleanup post-pass, and
//! persist it as a raw session.
//!
//! Pure and engine-agnostic — the concrete engine, the headless Claude
//! runner, and the target directory are all supplied by the caller (the
//! Tauri shell), so this stays unit-testable against [`crate::transcription::MockEngine`]
//! and a mock [`HeadlessClaude`].

use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::{DateTime, Utc};

use crate::device::DeviceId;
use crate::glossary::Glossary;
use crate::llm::HeadlessClaude;
use crate::metrics::PipelineTimings;
use crate::raw_session::{self, RawSessionError};
use crate::transcription::{
    self, clean_transcript, glossary_bias_terms, AudioChunk, AudioSource, Channel,
    TranscriptionEngine, TranscriptionError,
};

/// Failure transcribing or persisting a session.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error(transparent)]
    Transcription(#[from] TranscriptionError),
    #[error(transparent)]
    Persist(#[from] RawSessionError),
    /// Reading a channel's audio (e.g. a spill file) failed mid-pipeline.
    #[error("audio read failed: {0}")]
    AudioRead(#[from] std::io::Error),
}

/// [`transcribe_and_persist`]'s successful result: the path
/// [`raw_session::write_raw_session`] wrote, plus this run's per-stage
/// timing (see [`PipelineTimings`]).
#[derive(Debug, Clone)]
pub struct PipelineOutcome {
    pub path: PathBuf,
    pub timings: PipelineTimings,
}

/// Transcribes each channel's audio through a freshly built engine, merges
/// the results into one you/them transcript, runs the glossary cleanup
/// post-pass, and persists it.
///
/// `make_engine` is called once per channel — a fresh engine, not a shared
/// one, because an engine's internal VAD sample clock is not reset by
/// `finish()`; reusing one engine across channels would offset every channel
/// but the first onto the wrong session clock. Each channel is a pull-based
/// [`AudioSource`] already yielding `sample_rate` mono `f32` (engines expect
/// 16 kHz), so a long meeting streams from its spill file a chunk at a time
/// rather than materialising in memory; this module has no audio-processing
/// dependency of its own.
///
/// Every stage is timed (an `Instant` read is ~free) and returned in
/// [`PipelineOutcome::timings`] regardless of whether the caller looks at
/// them — see `src-tauri/src/transcribe.rs`'s `KODABI_METRICS` gate for the
/// only place that does.
#[allow(clippy::too_many_arguments)]
pub fn transcribe_and_persist(
    make_engine: &mut dyn FnMut() -> transcription::Result<Box<dyn TranscriptionEngine>>,
    cleaner: &dyn HeadlessClaude,
    glossary: &Glossary,
    channels: &mut [(Channel, &mut dyn AudioSource)],
    sample_rate: u32,
    dir: &Path,
    captured_at: DateTime<Utc>,
    device: &DeviceId,
    slug: Option<&str>,
) -> Result<PipelineOutcome, PipelineError> {
    let total_start = Instant::now();
    let bias_terms = glossary_bias_terms(glossary);

    let mut per_channel = Vec::with_capacity(channels.len());
    let mut engine_build_ms: u64 = 0;
    let mut transcribe_ms = Vec::with_capacity(channels.len());
    let mut audio_secs = 0.0f64;
    for (channel, source) in channels.iter_mut() {
        let build_start = Instant::now();
        let mut engine = make_engine()?;
        engine.set_bias(&bias_terms)?;
        engine_build_ms += build_start.elapsed().as_millis() as u64;

        let transcribe_start = Instant::now();
        // Pull the channel chunk by chunk, feeding each straight to the engine
        // so nothing but the current chunk is ever resident.
        let mut segments = Vec::new();
        let mut channel_samples: u64 = 0;
        while let Some(chunk) = source.next_chunk()? {
            channel_samples += chunk.len() as u64;
            segments.extend(engine.accept(AudioChunk {
                samples: chunk,
                sample_rate,
            })?);
        }
        segments.extend(engine.finish()?);
        transcribe_ms.push(transcribe_start.elapsed().as_millis() as u64);

        audio_secs += channel_samples as f64 / sample_rate.max(1) as f64;
        per_channel.push((*channel, segments));
    }

    let assemble_start = Instant::now();
    let assembled = raw_session::assemble(per_channel);
    let assemble_ms = assemble_start.elapsed().as_millis() as u64;

    let cleanup_start = Instant::now();
    let cleaned = clean_transcript(cleaner, assembled, glossary);
    let cleanup_ms = cleanup_start.elapsed().as_millis() as u64;

    let persist_start = Instant::now();
    let path = raw_session::write_raw_session(dir, captured_at, device, slug, &cleaned)?;
    let persist_ms = persist_start.elapsed().as_millis() as u64;

    let timings = PipelineTimings {
        audio_secs,
        engine_build_ms,
        transcribe_ms,
        assemble_ms,
        cleanup_ms,
        persist_ms,
        total_ms: total_start.elapsed().as_millis() as u64,
    };

    Ok(PipelineOutcome { path, timings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{LlmRequest, LlmRunError};
    use crate::transcription::{MockEngine, SliceSource};
    use chrono::TimeZone;
    use tempfile::tempdir;

    fn device() -> DeviceId {
        DeviceId::parse("k4m2xp7q").unwrap()
    }

    fn instant() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 12, 14, 3, 35).unwrap()
    }

    fn silence(len: usize) -> Vec<f32> {
        vec![0.0; len]
    }

    struct NoopRunner;
    impl HeadlessClaude for NoopRunner {
        fn run(&self, _request: &LlmRequest) -> Result<String, LlmRunError> {
            Ok("[]".to_owned())
        }
    }

    fn mock_engine_factory() -> impl FnMut() -> transcription::Result<Box<dyn TranscriptionEngine>>
    {
        || Ok(Box::new(MockEngine::new()) as Box<dyn TranscriptionEngine>)
    }

    #[test]
    fn transcribes_and_persists_both_channels() {
        let dir = tempdir().unwrap();
        let you = silence(16_000);
        let them = silence(8_000);
        // One chunk per channel (chunk_len >= the channel) so MockEngine emits
        // exactly one segment each — the wiring check.
        let mut you_source = SliceSource::new(&you, usize::MAX);
        let mut them_source = SliceSource::new(&them, usize::MAX);
        let mut channels: [(Channel, &mut dyn AudioSource); 2] = [
            (Channel::You, &mut you_source),
            (Channel::Them, &mut them_source),
        ];
        let mut make_engine = mock_engine_factory();

        let outcome = transcribe_and_persist(
            &mut make_engine,
            &NoopRunner,
            &Glossary::default(),
            &mut channels,
            16_000,
            dir.path(),
            instant(),
            &device(),
            None,
        )
        .expect("pipeline should succeed");

        let segments = raw_session::read_raw_session(&outcome.path).unwrap();
        assert_eq!(segments.len(), 2);
        assert!(segments.iter().any(|s| s.channel == Channel::You));
        assert!(segments.iter().any(|s| s.channel == Channel::Them));
        assert!(segments.iter().all(|s| s.text == "mock"));

        // 16,000 + 8,000 samples at 16 kHz = 1.5s of audio across both channels.
        assert_eq!(outcome.timings.audio_secs, 1.5);
        assert_eq!(outcome.timings.transcribe_ms.len(), 2);
    }

    #[test]
    fn streams_a_channel_in_chunks_with_cumulative_timestamps() {
        // Feed one channel in several small chunks. MockEngine emits one
        // segment per chunk with advancing timestamps, proving the pipeline
        // streams rather than materialising the whole channel.
        let dir = tempdir().unwrap();
        let audio = silence(16_000); // 1000 ms at 16 kHz
        let mut source = SliceSource::new(&audio, 4_000); // 4 chunks of 250 ms
        let mut channels: [(Channel, &mut dyn AudioSource); 1] = [(Channel::You, &mut source)];
        let mut make_engine = mock_engine_factory();

        let outcome = transcribe_and_persist(
            &mut make_engine,
            &NoopRunner,
            &Glossary::default(),
            &mut channels,
            16_000,
            dir.path(),
            instant(),
            &device(),
            None,
        )
        .expect("pipeline should succeed");

        let segments = raw_session::read_raw_session(&outcome.path).unwrap();
        assert_eq!(segments.len(), 4, "one segment per streamed chunk");
        // Timestamps advance 0→250→500→750→1000 across the chunks.
        assert_eq!(segments[0].start_ms, 0);
        assert_eq!(segments[3].end_ms, 1_000);
        // Audio duration is summed across chunks, not lost to chunking.
        assert_eq!(outcome.timings.audio_secs, 1.0);
    }

    #[test]
    fn empty_channels_persist_an_empty_transcript() {
        let dir = tempdir().unwrap();
        let mut make_engine = mock_engine_factory();

        let outcome = transcribe_and_persist(
            &mut make_engine,
            &NoopRunner,
            &Glossary::default(),
            &mut [],
            16_000,
            dir.path(),
            instant(),
            &device(),
            None,
        )
        .expect("pipeline should succeed with no channels");

        let segments = raw_session::read_raw_session(&outcome.path).unwrap();
        assert!(segments.is_empty());
        assert_eq!(outcome.timings.audio_secs, 0.0);
        assert!(outcome.timings.transcribe_ms.is_empty());
    }

    #[test]
    fn engine_build_failure_surfaces_as_a_pipeline_error() {
        let dir = tempdir().unwrap();
        let you = silence(16_000);
        let mut source = SliceSource::new(&you, usize::MAX);
        let mut channels: [(Channel, &mut dyn AudioSource); 1] = [(Channel::You, &mut source)];
        let mut make_engine = || {
            Err(TranscriptionError::ModelLoad(
                "model file missing".to_owned(),
            ))
        };

        let err = transcribe_and_persist(
            &mut make_engine,
            &NoopRunner,
            &Glossary::default(),
            &mut channels,
            16_000,
            dir.path(),
            instant(),
            &device(),
            None,
        )
        .unwrap_err();

        assert!(matches!(err, PipelineError::Transcription(_)));
    }

    #[test]
    fn audio_read_failure_surfaces_as_a_pipeline_error() {
        // A source whose read fails mid-pipeline must surface as AudioRead, not
        // be silently swallowed.
        struct FailingSource;
        impl AudioSource for FailingSource {
            fn next_chunk(&mut self) -> std::io::Result<Option<&[f32]>> {
                Err(std::io::Error::other("disk gone"))
            }
        }

        let dir = tempdir().unwrap();
        let mut source = FailingSource;
        let mut channels: [(Channel, &mut dyn AudioSource); 1] = [(Channel::You, &mut source)];
        let mut make_engine = mock_engine_factory();

        let err = transcribe_and_persist(
            &mut make_engine,
            &NoopRunner,
            &Glossary::default(),
            &mut channels,
            16_000,
            dir.path(),
            instant(),
            &device(),
            None,
        )
        .unwrap_err();

        assert!(matches!(err, PipelineError::AudioRead(_)));
    }

    #[test]
    fn persist_failure_surfaces_as_a_pipeline_error() {
        let dir = tempdir().unwrap();
        // A regular file where a directory is expected: `create_dir_all`
        // fails, which is `write_raw_session`'s only I/O failure mode here.
        let blocked = dir.path().join("not_a_dir");
        std::fs::write(&blocked, "occupied").unwrap();
        let mut make_engine = mock_engine_factory();

        let err = transcribe_and_persist(
            &mut make_engine,
            &NoopRunner,
            &Glossary::default(),
            &mut [],
            16_000,
            &blocked,
            instant(),
            &device(),
            None,
        )
        .unwrap_err();

        assert!(matches!(err, PipelineError::Persist(_)));
    }
}
