//! A trivial no-op [`TranscriptionEngine`] for wiring and testing the distill
//! pipeline before either real engine (Parakeet, whisper.cpp) lands.

use std::time::Duration;

use super::{AudioChunk, Result, Segment, TranscriptionEngine, TranscriptionError};

/// A deterministic, dependency-free engine that exercises the transcription
/// surface. Each non-empty [`accept`](TranscriptionEngine::accept) emits exactly
/// one segment whose timestamps advance by the chunk's real duration, so callers
/// can assert that timestamps flow correctly through the trait.
#[derive(Debug, Default)]
pub struct MockEngine {
    cursor_ms: u64,
    accept_delay: Duration,
}

impl MockEngine {
    /// Create a fresh mock engine positioned at time zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sleep `delay` on every accepted chunk.
    ///
    /// The mock is otherwise instantaneous, which makes a debug build's whole
    /// transcription finish before anything watching it can render a frame —
    /// so a progress bar is unexercisable without a real model. Pacing the
    /// mock gives a dev run something to watch (`src-tauri/src/transcribe.rs`
    /// wires this to `KODABI_MOCK_STT_DELAY_MS`). The default is zero, which
    /// is what every test keeps.
    pub fn with_accept_delay(mut self, delay: Duration) -> Self {
        self.accept_delay = delay;
        self
    }
}

impl TranscriptionEngine for MockEngine {
    fn accept(&mut self, chunk: AudioChunk<'_>) -> Result<Vec<Segment>> {
        if chunk.sample_rate == 0 {
            return Err(TranscriptionError::UnsupportedAudio(
                "sample rate must be greater than zero".to_owned(),
            ));
        }
        if chunk.samples.is_empty() {
            return Ok(Vec::new());
        }
        if !self.accept_delay.is_zero() {
            std::thread::sleep(self.accept_delay);
        }

        let duration_ms = chunk.samples.len() as u64 * 1_000 / u64::from(chunk.sample_rate);
        let start_ms = self.cursor_ms;
        let end_ms = start_ms + duration_ms;
        self.cursor_ms = end_ms;

        Ok(vec![Segment {
            start_ms,
            end_ms,
            text: "mock".to_owned(),
        }])
    }

    fn finish(&mut self) -> Result<Vec<Segment>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcription::transcribe_all;

    fn silence(len: usize) -> Vec<f32> {
        vec![0.0; len]
    }

    #[test]
    fn accept_emits_one_segment_with_duration_timestamps() {
        let mut engine = MockEngine::new();
        let audio = silence(16_000); // 16 000 samples @ 16 kHz == 1000 ms
        let segments = engine
            .accept(AudioChunk {
                samples: &audio,
                sample_rate: 16_000,
            })
            .expect("accept succeeds");
        assert_eq!(
            segments,
            vec![Segment {
                start_ms: 0,
                end_ms: 1_000,
                text: "mock".to_owned(),
            }]
        );
    }

    #[test]
    fn timestamps_are_cumulative_across_chunks() {
        let mut engine = MockEngine::new();
        let audio = silence(8_000); // 500 ms each
        let first = engine
            .accept(AudioChunk {
                samples: &audio,
                sample_rate: 16_000,
            })
            .unwrap();
        let second = engine
            .accept(AudioChunk {
                samples: &audio,
                sample_rate: 16_000,
            })
            .unwrap();
        assert_eq!(first[0].end_ms, 500);
        assert_eq!(second[0].start_ms, 500);
        assert_eq!(second[0].end_ms, 1_000);
    }

    #[test]
    fn accept_delay_paces_without_changing_the_segment() {
        // 1 ms: enough to prove the sleep is wired, cheap enough that the
        // suite doesn't notice.
        let mut engine = MockEngine::new().with_accept_delay(Duration::from_millis(1));
        let audio = silence(16_000);
        let segments = engine
            .accept(AudioChunk {
                samples: &audio,
                sample_rate: 16_000,
            })
            .expect("accept succeeds");
        assert_eq!(
            segments,
            vec![Segment {
                start_ms: 0,
                end_ms: 1_000,
                text: "mock".to_owned(),
            }]
        );
    }

    #[test]
    fn empty_chunk_yields_no_segments() {
        let mut engine = MockEngine::new();
        let segments = engine
            .accept(AudioChunk {
                samples: &[],
                sample_rate: 16_000,
            })
            .unwrap();
        assert!(segments.is_empty());
    }

    #[test]
    fn zero_sample_rate_is_unsupported() {
        let mut engine = MockEngine::new();
        let audio = silence(16);
        let err = engine
            .accept(AudioChunk {
                samples: &audio,
                sample_rate: 0,
            })
            .unwrap_err();
        assert!(matches!(err, TranscriptionError::UnsupportedAudio(_)));
    }

    #[test]
    fn set_bias_default_is_noop() {
        let mut engine = MockEngine::new();
        engine
            .set_bias(&["Kodabi".to_owned(), "sherpa-onnx".to_owned()])
            .expect("default bias is a no-op");
    }

    #[test]
    fn distill_pipeline_calls_through_trait_object() {
        // Drive the engine as `&mut dyn`, exactly like the distill pipeline.
        let mut engine = MockEngine::new();
        let audio = silence(16_000);
        let segments = transcribe_all(&mut engine, &audio, 16_000).unwrap();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].start_ms, 0);
        assert_eq!(segments[0].end_ms, 1_000);
    }
}
