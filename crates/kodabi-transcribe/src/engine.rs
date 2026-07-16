//! [`ParakeetEngine`] — NVIDIA Parakeet TDT via sherpa-onnx, VAD-gated so
//! silence is never fed to the recognizer.
//!
//! Silence-safety comes from the architecture, not from post-filtering: audio
//! is fed to sherpa-onnx's bundled Silero VAD, and the offline recognizer
//! only ever runs on a VAD-finalised speech segment. Over a silent stretch
//! the VAD finalises nothing, so the recognizer is never invoked and no
//! phantom text is produced.

use std::path::PathBuf;

use kodabi_core::transcription::{
    AudioChunk, Result, Segment, TranscriptionEngine, TranscriptionError,
};

use crate::silero::{build_silero_vad, SileroParams};
use crate::validate::{
    apply_nonnegative_f32_override, apply_positive_i32_override, apply_probability_f32_override,
    clamp_threads, path_to_string, require_file, segment_ms, validate_chunk, SAMPLE_RATE_HZ,
};

/// Local model files and tuning knobs for [`ParakeetEngine`].
///
/// All five paths must point at files that already exist on disk — this
/// ticket only supports pointing at a locally-downloaded model directory.
/// Download-on-first-run, settings persistence and Tauri wiring are separate,
/// later tickets.
#[derive(Debug, Clone)]
pub struct ParakeetConfig {
    /// `encoder.onnx` (or `encoder.int8.onnx`) for the Parakeet TDT transducer.
    pub encoder: PathBuf,
    /// `decoder.onnx`.
    pub decoder: PathBuf,
    /// `joiner.onnx`.
    pub joiner: PathBuf,
    /// `tokens.txt`.
    pub tokens: PathBuf,
    /// `silero_vad.onnx`, sherpa-onnx's bundled VAD model.
    pub vad_model: PathBuf,
    /// Threads handed to both the VAD and the recognizer.
    pub num_threads: i32,
    /// Inference provider, e.g. `Some("cpu")`. `None` lets sherpa-onnx choose.
    pub provider: Option<String>,
    /// Silero VAD speech-probability threshold (0.0–1.0).
    pub vad_threshold: f32,
    /// Minimum silence (seconds) before a speech segment is finalised.
    pub min_silence_duration: f32,
    /// Minimum speech (seconds) before a segment is considered real speech.
    pub min_speech_duration: f32,
    /// Maximum speech (seconds) before a segment is force-finalised, so a
    /// long monologue still emits incrementally instead of buffering forever.
    pub max_speech_duration: f32,
    /// Forwarded to sherpa-onnx's own debug logging.
    pub debug: bool,
}

impl ParakeetConfig {
    /// Applies `KODABI_PARAKEET_THREADS` (both the VAD and recognizer thread
    /// count — see [`ParakeetConfig::num_threads`]) and the VAD-gating
    /// overrides `KODABI_VAD_THRESHOLD`/`KODABI_VAD_MIN_SILENCE`/
    /// `KODABI_VAD_MIN_SPEECH`/`KODABI_VAD_MAX_SPEECH`, if set and valid —
    /// the same env vars [`crate::VadConfig::apply_env_overrides`] reads, so
    /// one set of knobs tunes VAD gating for whichever engine is active. A
    /// blank/unparsable/out-of-range value falls back to whatever `self`
    /// already carries rather than silently breaking the pipeline. Lets a
    /// resource-budget pass iterate on real hardware without recompiling.
    pub fn apply_env_overrides(mut self) -> Self {
        self.num_threads = apply_positive_i32_override(
            self.num_threads,
            std::env::var("KODABI_PARAKEET_THREADS").ok(),
        );
        self.vad_threshold = apply_probability_f32_override(
            self.vad_threshold,
            std::env::var("KODABI_VAD_THRESHOLD").ok(),
        );
        self.min_silence_duration = apply_nonnegative_f32_override(
            self.min_silence_duration,
            std::env::var("KODABI_VAD_MIN_SILENCE").ok(),
        );
        self.min_speech_duration = apply_nonnegative_f32_override(
            self.min_speech_duration,
            std::env::var("KODABI_VAD_MIN_SPEECH").ok(),
        );
        self.max_speech_duration = apply_nonnegative_f32_override(
            self.max_speech_duration,
            std::env::var("KODABI_VAD_MAX_SPEECH").ok(),
        );
        self
    }
}

/// NVIDIA Parakeet TDT transcription, gated by sherpa-onnx's bundled Silero
/// VAD. See the module docs for why this can't hallucinate over silence.
pub struct ParakeetEngine {
    vad: sherpa_onnx::VoiceActivityDetector,
    recognizer: sherpa_onnx::OfflineRecognizer,
}

impl ParakeetEngine {
    /// Load the VAD and Parakeet models. Any failure — a missing file or a
    /// sherpa-onnx init failure — surfaces as [`TranscriptionError::ModelLoad`].
    pub fn new(cfg: ParakeetConfig) -> Result<Self> {
        for path in [
            &cfg.encoder,
            &cfg.decoder,
            &cfg.joiner,
            &cfg.tokens,
            &cfg.vad_model,
        ] {
            require_file(path)?;
        }

        let vad = build_silero_vad(SileroParams {
            model: &cfg.vad_model,
            num_threads: cfg.num_threads,
            provider: cfg.provider.as_deref(),
            threshold: cfg.vad_threshold,
            min_silence_duration: cfg.min_silence_duration,
            min_speech_duration: cfg.min_speech_duration,
            max_speech_duration: cfg.max_speech_duration,
            debug: cfg.debug,
        })?;

        let num_threads = clamp_threads(cfg.num_threads);
        let model_config = sherpa_onnx::OfflineModelConfig {
            transducer: sherpa_onnx::OfflineTransducerModelConfig {
                encoder: Some(path_to_string(&cfg.encoder)?),
                decoder: Some(path_to_string(&cfg.decoder)?),
                joiner: Some(path_to_string(&cfg.joiner)?),
            },
            tokens: Some(path_to_string(&cfg.tokens)?),
            num_threads,
            provider: cfg.provider,
            debug: cfg.debug,
            ..Default::default()
        };
        let recognizer_config = sherpa_onnx::OfflineRecognizerConfig {
            model_config,
            ..Default::default()
        };
        let recognizer =
            sherpa_onnx::OfflineRecognizer::create(&recognizer_config).ok_or_else(|| {
                TranscriptionError::ModelLoad("failed to initialise Parakeet recognizer".to_owned())
            })?;

        Ok(Self { vad, recognizer })
    }

    /// Recognise every VAD segment finalised so far and drain the VAD queue.
    fn drain_speech(&mut self, segments: &mut Vec<Segment>) {
        while let Some(seg) = self.vad.front() {
            let (start_ms, end_ms) = segment_ms(seg.start(), seg.n());

            let stream = self.recognizer.create_stream();
            stream.accept_waveform(SAMPLE_RATE_HZ as i32, seg.samples());
            self.recognizer.decode(&stream);

            if let Some(result) = stream.get_result() {
                let text = result.text.trim();
                if !text.is_empty() {
                    segments.push(Segment {
                        start_ms,
                        end_ms,
                        text: text.to_owned(),
                    });
                }
            }

            self.vad.pop();
        }
    }
}

impl TranscriptionEngine for ParakeetEngine {
    fn accept(&mut self, chunk: AudioChunk<'_>) -> Result<Vec<Segment>> {
        if !validate_chunk(chunk.sample_rate, chunk.samples.is_empty())? {
            return Ok(Vec::new());
        }

        // The VAD buffers input internally and windows it itself, so the whole
        // chunk goes in at once; any complete speech segments it finalises are
        // then drained.
        self.vad.accept_waveform(chunk.samples);

        let mut segments = Vec::new();
        self.drain_speech(&mut segments);
        Ok(segments)
    }

    fn finish(&mut self) -> Result<Vec<Segment>> {
        self.vad.flush();

        let mut segments = Vec::new();
        self.drain_speech(&mut segments);
        Ok(segments)
    }

    // `set_bias` keeps the trait's no-op default: Parakeet has no
    // initial-prompt bias mechanism, so glossary correctness comes entirely
    // from the later engine-agnostic post-pass.
}
