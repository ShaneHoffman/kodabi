//! [`WhisperEngine`] — whisper.cpp large-v3-turbo via `whisper-rs`, the
//! multilingual fallback with the strongest glossary-biasing mechanism
//! (the initial prompt, see [`TranscriptionEngine::set_bias`]).
//!
//! Unlike Parakeet, whisper.cpp has no bundled VAD and is known to
//! hallucinate phantom text over silence. This engine does **not** attempt
//! silence-safety itself — it is a batch engine: `accept` only buffers
//! samples, and `finish` runs whisper.cpp once over the whole buffer, exactly
//! as the trait's docs allow for batch engines, with no gating of its own.
//!
//! Silence-safety is instead composed in front of it via [`whisper_with_vad`],
//! which fronts a [`WhisperEngine`] with sherpa-onnx's bundled Silero VAD
//! ([`crate::VadGate`]) so it only ever decodes VAD-finalised speech segments.
//! Use `whisper_with_vad` for production wiring; construct a bare
//! [`WhisperEngine`] directly only for engine-isolation tests.

use std::path::PathBuf;

use kodama_core::transcription::{
    AudioChunk, Result, Segment, TranscriptionEngine, TranscriptionError,
};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::validate::{clamp_threads, path_to_string, require_file, validate_chunk};
use crate::{VadConfig, VadGate};

/// Local model file and tuning knobs for [`WhisperEngine`].
///
/// `model` must point at a file that already exists on disk — this ticket
/// only supports pointing at a locally-downloaded ggml model (e.g.
/// `ggml-large-v3-turbo.bin`). Download-on-first-run, settings persistence
/// and Tauri wiring are separate, later tickets (mirrors `ParakeetConfig`).
#[derive(Debug, Clone)]
pub struct WhisperConfig {
    /// Path to a ggml/gguf whisper.cpp model file.
    pub model: PathBuf,
    /// Run inference on GPU when the engine was built with a GPU feature
    /// (e.g. `whisper-cuda`). Ignored on a CPU-only build.
    pub use_gpu: bool,
    /// Threads handed to whisper.cpp's decoder.
    pub num_threads: i32,
    /// Force a spoken language (e.g. `Some("en")`), or `None` to let
    /// whisper.cpp auto-detect — the multilingual case this engine exists
    /// for.
    pub language: Option<String>,
}

/// whisper.cpp large-v3-turbo transcription. A batch engine: `accept`
/// buffers samples, `finish` runs the model once over the full buffer.
pub struct WhisperEngine {
    ctx: WhisperContext,
    num_threads: i32,
    language: Option<String>,
    buffer: Vec<f32>,
    bias_prompt: Option<String>,
}

impl WhisperEngine {
    /// Load the whisper.cpp model. A missing file or a whisper.cpp init
    /// failure surfaces as [`TranscriptionError::ModelLoad`].
    pub fn new(cfg: WhisperConfig) -> Result<Self> {
        require_file(&cfg.model)?;
        let model_path = path_to_string(&cfg.model)?;

        let mut params = WhisperContextParameters::default();
        params.use_gpu(cfg.use_gpu);

        let ctx = WhisperContext::new_with_params(&model_path, params)
            .map_err(|err| TranscriptionError::ModelLoad(err.to_string()))?;

        Ok(Self {
            ctx,
            num_threads: clamp_threads(cfg.num_threads),
            language: cfg.language,
            buffer: Vec::new(),
            bias_prompt: None,
        })
    }
}

impl TranscriptionEngine for WhisperEngine {
    fn accept(&mut self, chunk: AudioChunk<'_>) -> Result<Vec<Segment>> {
        if !validate_chunk(chunk.sample_rate, chunk.samples.is_empty())? {
            return Ok(Vec::new());
        }

        // Whole-buffer engine: accumulate and defer inference to `finish`.
        self.buffer.extend_from_slice(chunk.samples);
        Ok(Vec::new())
    }

    fn finish(&mut self) -> Result<Vec<Segment>> {
        // Take the buffer up front so this batch is consumed by this call
        // whether inference succeeds or fails: a retry after an `Engine` error,
        // or a reused engine driven through a second recording, then starts
        // from an empty buffer instead of re-processing stale audio.
        let audio = std::mem::take(&mut self.buffer);
        if audio.is_empty() {
            return Ok(Vec::new());
        }

        let mut state = self
            .ctx
            .create_state()
            .map_err(|err| TranscriptionError::Engine(err.to_string()))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.num_threads);
        params.set_language(self.language.as_deref());
        if let Some(prompt) = self.bias_prompt.as_deref() {
            params.set_initial_prompt(prompt);
        }
        // This is a headless engine call; whisper.cpp's own stdout logging
        // would otherwise interleave with the host application's output.
        params.set_print_progress(false);
        params.set_print_special(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        state
            .full(params, &audio)
            .map_err(|err| TranscriptionError::Engine(err.to_string()))?;

        let mut segments = Vec::new();
        let segment_count = state.full_n_segments();
        for index in 0..segment_count {
            let Some(segment) = state.get_segment(index) else {
                continue;
            };
            let Ok(text) = segment.to_str() else {
                continue;
            };
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            segments.push(Segment {
                start_ms: centiseconds_to_ms(segment.start_timestamp()),
                end_ms: centiseconds_to_ms(segment.end_timestamp()),
                text: text.to_owned(),
            });
        }

        Ok(segments)
    }

    /// Join the glossary terms into whisper.cpp's initial prompt — the
    /// strongest bias mechanism this engine offers (unlike Parakeet, which
    /// has no such hook and relies entirely on the post-pass).
    ///
    /// Interior NUL bytes are stripped from each term: `whisper-rs`'s
    /// `set_initial_prompt` builds a `CString` and *panics* on an embedded NUL,
    /// so a stray one in a glossary term would otherwise take down the whole
    /// `finish` call rather than surfacing as an error.
    fn set_bias(&mut self, terms: &[String]) -> Result<()> {
        self.bias_prompt = if terms.is_empty() {
            None
        } else {
            Some(
                terms
                    .iter()
                    .map(|term| term.replace('\0', ""))
                    .collect::<Vec<_>>()
                    .join(", "),
            )
        };
        Ok(())
    }
}

/// whisper.cpp reports segment timestamps in centiseconds (10 ms units)
/// relative to the start of the buffer passed to `full`.
fn centiseconds_to_ms(cs: i64) -> u64 {
    (cs.max(0) as u64) * 10
}

/// Load a [`WhisperEngine`] fronted by a Silero VAD gate — the blessed
/// production entry point for the Whisper path (FOUNDING_DOC §8: "VAD
/// mandatory on any Whisper path"). Silence never reaches whisper.cpp: the
/// VAD gate only decodes VAD-finalised speech segments and re-maps their
/// timestamps onto the absolute session clock. See the module docs and
/// [`crate::VadGate`].
pub fn whisper_with_vad(whisper: WhisperConfig, vad: VadConfig) -> Result<VadGate<WhisperEngine>> {
    VadGate::new(vad, WhisperEngine::new(whisper)?)
}
