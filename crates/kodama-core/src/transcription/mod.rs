//! Engine-agnostic transcription surface.
//!
//! Kodama transcribes audio through the [`TranscriptionEngine`] trait so that
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

mod mock;

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
/// `samples` is single-channel `f32` PCM; `sample_rate` is in hertz. Kodama's
/// engines expect 16 kHz. The slice is borrowed to avoid copying large buffers.
#[derive(Debug, Clone, Copy)]
pub struct AudioChunk<'a> {
    /// Mono `f32` PCM samples.
    pub samples: &'a [f32],
    /// Sample rate in hertz (Kodama feeds 16 kHz).
    pub sample_rate: u32,
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
