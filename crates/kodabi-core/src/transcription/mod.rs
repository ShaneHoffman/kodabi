//! Engine-agnostic transcription surface.
//!
//! Kodabi transcribes audio through the [`TranscriptionEngine`] trait so that
//! concrete engines (Parakeet via sherpa-onnx, whisper.cpp, ...) are swappable
//! behind one interface. No engine-specific types cross this boundary.
//!
//! # Model loading
//!
//! Loading/initialising a model is each engine's own constructor's job
//! (e.g. `ParakeetEngine::new(cfg) -> Result<Self>`), because model config is
//! inherently engine-specific (a set of ONNX files vs. a single ggml path) and
//! cannot be expressed generically without leaking types. A value implementing
//! [`TranscriptionEngine`] is therefore an *already-loaded, ready* engine.
//! Construction failures surface as [`TranscriptionError::ModelLoad`].
//!
//! # Threading
//!
//! Engines wrap blocking CPU/GPU FFI, so the trait is deliberately synchronous;
//! callers offload to a blocking worker (e.g. `spawn_blocking`). The [`Send`]
//! supertrait lets a `Box<dyn TranscriptionEngine>` move onto that worker.
//!
//! # Segment shape
//!
//! [`Segment`] carries only `start_ms`/`end_ms`/`text`. The wire
//! `TranscriptSegment` fields `index`, `channel` (you/them) and `speaker` are
//! added by outer layers (capture layer / post-v1 diarization), not the engine.
//!
//! # Glossary biasing
//!
//! [`apply_glossary_bias`] bridges a project's [`crate::glossary::Glossary`]
//! into [`TranscriptionEngine::set_bias`].
//!
//! # Glossary cleanup post-pass
//!
//! [`clean_transcript`] is the engine-agnostic second half of that split: a
//! headless Claude Code call, run on the already-assembled transcript at
//! meeting end, that fixes obvious glossary misrecognitions bias couldn't
//! (or, on an engine with no bias hook, the only glossary defense at all).

mod cleanup;
mod glossary_bias;
mod mock;

pub use cleanup::{
    apply_corrections, build_request, clean_transcript, clean_transcript_for_project,
};
pub use glossary_bias::{apply_glossary_bias, glossary_bias_terms, GlossaryBiasError};
pub use mock::MockEngine;

/// Errors produced while loading a model or transcribing audio.
#[derive(Debug, thiserror::Error)]
pub enum TranscriptionError {
    /// A model failed to load or initialise (engine constructor failure).
    #[error("failed to load transcription model: {0}")]
    ModelLoad(String),

    /// The audio is not in a form the engine accepts (engines expect mono
    /// `f32` PCM at 16 kHz).
    #[error("unsupported audio: {0}")]
    UnsupportedAudio(String),

    /// An engine-internal failure (FFI/decode), kept as a message so no
    /// engine-specific error type leaks across the trait boundary.
    #[error("transcription engine error: {0}")]
    Engine(String),
}

/// `Result` specialised to [`TranscriptionError`].
pub type Result<T> = std::result::Result<T, TranscriptionError>;

/// A borrowed batch of mono PCM samples.
///
/// `samples` is single-channel `f32` PCM; `sample_rate` is in hertz. Kodabi's
/// engines expect 16 kHz. The slice is borrowed to avoid copying large buffers.
#[derive(Debug, Clone, Copy)]
pub struct AudioChunk<'a> {
    /// Mono `f32` PCM samples.
    pub samples: &'a [f32],
    /// Sample rate in hertz (Kodabi feeds 16 kHz).
    pub sample_rate: u32,
}

/// Per-channel attribution for a transcript segment: local mic = `You`,
/// system-audio loopback = `Them` (FOUNDING_DOC §3.3's two-channel bonus;
/// mirrors the `Channel` enum in `docs/MCP_TOOL_SURFACE.md`). `Unknown`
/// covers audio that never went through the two-channel split (e.g. a
/// single-channel import). Diarization within a channel — telling apart
/// multiple people on the same side — is v1 anti-scope (§5); this is the
/// whole of v1's speaker attribution.
///
/// The capture layer (`kodabi_audio::SessionChannel`) is positional — `Mic`
/// / `System` — and stays free of a dependency on this crate; outer layers
/// map `Mic -> You` and `System -> Them` when they assign this label to a
/// transcribed [`Segment`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    You,
    Them,
    Unknown,
}

/// A finalised, timestamped span of recognised text.
///
/// Timestamps are integer millisecond offsets from the first sample fed to the
/// engine. Outer layers add `index`, `channel` and `speaker`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// Start offset in milliseconds from the first sample.
    pub start_ms: u64,
    /// End offset in milliseconds from the first sample.
    pub end_ms: u64,
    /// Recognised text for this span.
    pub text: String,
}

/// An engine-agnostic streaming/batch transcription engine.
///
/// Implementors are already-loaded models (see the module docs). Feed audio with
/// [`accept`](TranscriptionEngine::accept), then call
/// [`finish`](TranscriptionEngine::finish) once after the final chunk to flush
/// any buffered tail. Batch callers can use
/// [`transcribe`](TranscriptionEngine::transcribe).
pub trait TranscriptionEngine: Send {
    /// Feed one chunk of audio and return the segments *finalised since the
    /// previous call*. Streaming engines may emit incrementally; batch engines
    /// may buffer and return everything from
    /// [`finish`](TranscriptionEngine::finish). Both are valid.
    fn accept(&mut self, chunk: AudioChunk<'_>) -> Result<Vec<Segment>>;

    /// Signal end of audio and return any remaining buffered segments.
    fn finish(&mut self) -> Result<Vec<Segment>>;

    /// Provide glossary/bias hint terms (e.g. Whisper's initial prompt).
    ///
    /// Defaults to a no-op: engines without a bias mechanism (Parakeet, whose
    /// glossary correctness comes from a later engine-agnostic post-pass)
    /// inherit it for free. Only biasing engines override this.
    fn set_bias(&mut self, _terms: &[String]) -> Result<()> {
        Ok(())
    }

    /// Transcribe a whole recording in one call: `accept` then `finish`.
    fn transcribe(&mut self, chunk: AudioChunk<'_>) -> Result<Vec<Segment>> {
        let mut segments = self.accept(chunk)?;
        segments.extend(self.finish()?);
        Ok(segments)
    }
}

/// Transcribe a whole buffer through a trait object, exactly as the distill
/// pipeline will: hold a `&mut dyn TranscriptionEngine` and never name a
/// concrete engine.
pub fn transcribe_all(
    engine: &mut dyn TranscriptionEngine,
    samples: &[f32],
    sample_rate: u32,
) -> Result<Vec<Segment>> {
    engine.transcribe(AudioChunk {
        samples,
        sample_rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_serializes_to_snake_case() {
        assert_eq!(serde_json::to_string(&Channel::You).unwrap(), r#""you""#);
        assert_eq!(serde_json::to_string(&Channel::Them).unwrap(), r#""them""#);
        assert_eq!(
            serde_json::to_string(&Channel::Unknown).unwrap(),
            r#""unknown""#
        );
    }

    #[test]
    fn channel_round_trips_through_json() {
        for channel in [Channel::You, Channel::Them, Channel::Unknown] {
            let json = serde_json::to_string(&channel).unwrap();
            let parsed: Channel = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, channel);
        }
    }
}
