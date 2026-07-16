//! Orchestrates the live-capture pipeline: transcribe each channel, merge
//! into one you/them transcript, run the glossary cleanup post-pass, and
//! persist it as a raw session.
//!
//! Pure and engine-agnostic — the concrete engine, the headless Claude
//! runner, and the target directory are all supplied by the caller (the
//! Tauri shell), so this stays unit-testable against [`crate::transcription::MockEngine`]
//! and a mock [`HeadlessClaude`].

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::device::DeviceId;
use crate::glossary::Glossary;
use crate::raw_session::{self, RawSessionError};
use crate::transcription::{
    self, clean_transcript, glossary_bias_terms, Channel, HeadlessClaude, TranscriptionEngine,
    TranscriptionError,
};

/// Failure transcribing or persisting a session.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error(transparent)]
    Transcription(#[from] TranscriptionError),
    #[error(transparent)]
    Persist(#[from] RawSessionError),
}

/// Transcribes each channel's audio through a freshly built engine, merges
/// the results into one you/them transcript, runs the glossary cleanup
/// post-pass, and persists it.
///
/// `make_engine` is called once per channel — a fresh engine, not a shared
/// one, because an engine's internal VAD sample clock is not reset by
/// `finish()`; reusing one engine across channels would offset every channel
/// but the first onto the wrong session clock. `channels` must already be
/// resampled to `sample_rate` (engines expect 16 kHz mono `f32`); this module
/// has no audio-processing dependency of its own.
///
/// Returns the path [`raw_session::write_raw_session`] wrote.
#[allow(clippy::too_many_arguments)]
pub fn transcribe_and_persist(
    make_engine: &mut dyn FnMut() -> transcription::Result<Box<dyn TranscriptionEngine>>,
    cleaner: &dyn HeadlessClaude,
    glossary: &Glossary,
    channels: &[(Channel, Vec<f32>)],
    sample_rate: u32,
    dir: &Path,
    captured_at: DateTime<Utc>,
    device: &DeviceId,
    slug: Option<&str>,
) -> Result<PathBuf, PipelineError> {
    let bias_terms = glossary_bias_terms(glossary);

    let mut per_channel = Vec::with_capacity(channels.len());
    for (channel, samples) in channels {
        let mut engine = make_engine()?;
        engine.set_bias(&bias_terms)?;
        let segments = transcription::transcribe_all(engine.as_mut(), samples, sample_rate)?;
        per_channel.push((*channel, segments));
    }

    let assembled = raw_session::assemble(per_channel);
    let cleaned = clean_transcript(cleaner, assembled, glossary);

    let path = raw_session::write_raw_session(dir, captured_at, device, slug, &cleaned)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcription::{CleanupRequest, CleanupRunError, MockEngine};
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
        fn run(&self, _request: &CleanupRequest) -> Result<String, CleanupRunError> {
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
        let channels = vec![
            (Channel::You, silence(16_000)),
            (Channel::Them, silence(8_000)),
        ];
        let mut make_engine = mock_engine_factory();

        let path = transcribe_and_persist(
            &mut make_engine,
            &NoopRunner,
            &Glossary::default(),
            &channels,
            16_000,
            dir.path(),
            instant(),
            &device(),
            None,
        )
        .expect("pipeline should succeed");

        let segments = raw_session::read_raw_session(&path).unwrap();
        assert_eq!(segments.len(), 2);
        assert!(segments.iter().any(|s| s.channel == Channel::You));
        assert!(segments.iter().any(|s| s.channel == Channel::Them));
        assert!(segments.iter().all(|s| s.text == "mock"));
    }

    #[test]
    fn empty_channels_persist_an_empty_transcript() {
        let dir = tempdir().unwrap();
        let mut make_engine = mock_engine_factory();

        let path = transcribe_and_persist(
            &mut make_engine,
            &NoopRunner,
            &Glossary::default(),
            &[],
            16_000,
            dir.path(),
            instant(),
            &device(),
            None,
        )
        .expect("pipeline should succeed with no channels");

        let segments = raw_session::read_raw_session(&path).unwrap();
        assert!(segments.is_empty());
    }

    #[test]
    fn engine_build_failure_surfaces_as_a_pipeline_error() {
        let dir = tempdir().unwrap();
        let channels = vec![(Channel::You, silence(16_000))];
        let mut make_engine = || {
            Err(TranscriptionError::ModelLoad(
                "model file missing".to_owned(),
            ))
        };

        let err = transcribe_and_persist(
            &mut make_engine,
            &NoopRunner,
            &Glossary::default(),
            &channels,
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
            &[],
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
