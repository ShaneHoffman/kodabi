//! [`VadGate`] — a Silero VAD (via sherpa-onnx) fronting any
//! [`TranscriptionEngine`], so silence never reaches the wrapped engine.
//!
//! Whisper.cpp has no bundled VAD and is known to hallucinate phantom text
//! over silence (FOUNDING_DOC §8). `VadGate<WhisperEngine>` — assembled by
//! `whisper::whisper_with_vad` — is how the Whisper path gets the same
//! silence-safety Parakeet has natively: audio is fed to sherpa-onnx's
//! bundled Silero VAD, and the wrapped engine only ever sees a VAD-finalised
//! speech segment, decoded on its own. Over a silent stretch the VAD
//! finalises nothing, so the wrapped engine is never invoked.
//!
//! Each finalised speech segment is decoded independently (mirrors
//! `ParakeetEngine`'s `drain_speech`), and the wrapped engine's
//! segment-relative timestamps are re-mapped onto the absolute session clock
//! via [`segment_ms`] + [`offset_into_span`].

use std::path::PathBuf;

use kodabi_core::transcription::{AudioChunk, Result, Segment, TranscriptionEngine};

use crate::silero::{build_silero_vad, SileroParams};
use crate::validate::{
    apply_nonnegative_f32_override, apply_positive_i32_override, offset_into_span, segment_ms,
    validate_chunk, SAMPLE_RATE_HZ,
};

/// Silero VAD tuning knobs for [`VadGate`].
///
/// `vad_model` must point at a file that already exists on disk (e.g.
/// `silero_vad.onnx`). Download-on-first-run, settings persistence and Tauri
/// wiring are separate, later tickets (mirrors `ParakeetConfig`).
#[derive(Debug, Clone)]
pub struct VadConfig {
    /// `silero_vad.onnx`, sherpa-onnx's bundled VAD model.
    pub vad_model: PathBuf,
    /// Threads handed to the VAD.
    pub num_threads: i32,
    /// Inference provider, e.g. `Some("cpu")`. `None` lets sherpa-onnx choose.
    pub provider: Option<String>,
    /// Silero VAD speech-probability threshold (0.0-1.0).
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

impl VadConfig {
    /// Applies `KODABI_VAD_THREADS` and the VAD-gating overrides
    /// `KODABI_VAD_THRESHOLD`/`KODABI_VAD_MIN_SILENCE`/`KODABI_VAD_MIN_SPEECH`/
    /// `KODABI_VAD_MAX_SPEECH`, if set and valid — the same gating env vars
    /// [`crate::ParakeetConfig::apply_env_overrides`] reads (this config's
    /// `num_threads` is the standalone VAD's own, distinct from Parakeet's
    /// shared recognizer+VAD thread count). A blank/unparsable/out-of-range
    /// value falls back to whatever `self` already carries.
    pub fn apply_env_overrides(mut self) -> Self {
        self.num_threads =
            apply_positive_i32_override(self.num_threads, std::env::var("KODABI_VAD_THREADS").ok());
        self.vad_threshold = apply_nonnegative_f32_override(
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

/// Fronts a wrapped [`TranscriptionEngine`] with sherpa-onnx's bundled Silero
/// VAD, so the wrapped engine only ever decodes VAD-finalised speech spans.
/// See the module docs for why this makes the wrapped engine silence-safe.
pub struct VadGate<E: TranscriptionEngine> {
    vad: sherpa_onnx::VoiceActivityDetector,
    inner: E,
    /// Segments already transcribed (and popped from the VAD queue) but not
    /// yet handed back to the caller, because a *later* span in the same
    /// `drain` errored before the call could return. Re-emitted at the front
    /// of the next successful drain, so a mid-drain inner-engine error never
    /// loses already-decoded speech. Empty on the happy path.
    pending: Vec<Segment>,
}

impl<E: TranscriptionEngine> VadGate<E> {
    /// Load the VAD and wrap `inner`. A missing model file or a sherpa-onnx
    /// init failure surfaces as [`kodabi_core::transcription::TranscriptionError::ModelLoad`].
    pub fn new(cfg: VadConfig, inner: E) -> Result<Self> {
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

        Ok(Self {
            vad,
            inner,
            pending: Vec::new(),
        })
    }

    /// Decode every VAD segment finalised so far, one at a time, re-mapping
    /// each result onto the absolute session clock, and drain the VAD queue.
    ///
    /// Decoded segments accumulate in `self.pending` — and a previous call's
    /// `pending` is re-emitted first — so the drain is genuinely lossless
    /// across an inner-engine error. The `sherpa` VAD queue only exposes
    /// `front`/`pop`, so a span must be popped before the next can be decoded;
    /// staging results in `pending` (rather than the caller's `out`) means a
    /// later span's error leaves every already-decoded span buffered for the
    /// next successful drain to return, while the *failing* span stays queued
    /// (not popped) for retry. On success the whole batch moves to `out`.
    fn drain(&mut self, out: &mut Vec<Segment>) -> Result<()> {
        while let Some(seg) = self.vad.front() {
            let (span_start_ms, span_end_ms) = segment_ms(seg.start(), seg.n());

            let inner_segments = self.inner.transcribe(AudioChunk {
                samples: seg.samples(),
                sample_rate: SAMPLE_RATE_HZ,
            })?;

            for inner_segment in inner_segments {
                if let Some(mapped) = offset_into_span(inner_segment, span_start_ms, span_end_ms) {
                    self.pending.push(mapped);
                }
            }

            self.vad.pop();
        }
        out.append(&mut self.pending);
        Ok(())
    }
}

impl<E: TranscriptionEngine> TranscriptionEngine for VadGate<E> {
    fn accept(&mut self, chunk: AudioChunk<'_>) -> Result<Vec<Segment>> {
        if !validate_chunk(chunk.sample_rate, chunk.samples.is_empty())? {
            return Ok(Vec::new());
        }

        // The VAD buffers input internally and windows it itself, so the whole
        // chunk goes in at once; any complete speech segments it finalises are
        // then drained.
        self.vad.accept_waveform(chunk.samples);

        let mut segments = Vec::new();
        self.drain(&mut segments)?;
        Ok(segments)
    }

    fn finish(&mut self) -> Result<Vec<Segment>> {
        self.vad.flush();

        let mut segments = Vec::new();
        self.drain(&mut segments)?;
        Ok(segments)
    }

    /// Forward to the wrapped engine's bias mechanism (e.g. Whisper's initial
    /// prompt). The VAD gate itself has no bias concept.
    fn set_bias(&mut self, terms: &[String]) -> Result<()> {
        self.inner.set_bias(terms)
    }
}
