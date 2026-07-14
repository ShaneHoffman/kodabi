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

use kodama_core::transcription::{
    AudioChunk, Result, Segment, TranscriptionEngine, TranscriptionError,
};

use crate::validate::{
    offset_into_span, path_to_string, require_file, segment_ms, validate_chunk, SAMPLE_RATE_HZ,
};

/// Silero VAD processing window, in samples. Handed to sherpa-onnx as its
/// `window_size`; the VAD buffers input internally and slices it into windows
/// itself, so callers may feed arbitrary-length chunks.
///
/// Keep in sync with the identical constant in `engine.rs` (`ParakeetEngine`
/// builds its own Silero VAD the same way; this ticket didn't extract a
/// shared `SileroVad` to avoid touching that working, CI-untested engine).
const WINDOW_SIZE: usize = 512;

/// Head-room added to `max_speech_duration` when sizing the VAD's internal
/// sample buffer, so a maximal speech segment plus the trailing silence needed
/// to finalise it always fits before the circular buffer starts dropping the
/// oldest samples.
///
/// Keep in sync with the identical constant in `engine.rs`.
const VAD_BUFFER_MARGIN_SECONDS: f32 = 10.0;

/// Floor for the VAD's internal buffer, covering a tiny or disabled
/// (`max_speech_duration <= 0`) force-finalise setting.
///
/// Keep in sync with the identical constant in `engine.rs`.
const VAD_BUFFER_MIN_SECONDS: f32 = 30.0;

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

/// Fronts a wrapped [`TranscriptionEngine`] with sherpa-onnx's bundled Silero
/// VAD, so the wrapped engine only ever decodes VAD-finalised speech spans.
/// See the module docs for why this makes the wrapped engine silence-safe.
pub struct VadGate<E: TranscriptionEngine> {
    vad: sherpa_onnx::VoiceActivityDetector,
    inner: E,
}

impl<E: TranscriptionEngine> VadGate<E> {
    /// Load the VAD and wrap `inner`. A missing model file or a sherpa-onnx
    /// init failure surfaces as [`kodama_core::transcription::TranscriptionError::ModelLoad`].
    pub fn new(cfg: VadConfig, inner: E) -> Result<Self> {
        require_file(&cfg.vad_model)?;

        // Clamp to at least one thread: sherpa-onnx treats `num_threads` as a
        // thread-pool size, and a zero or negative value from a future
        // settings layer is nonsensical rather than a valid "use defaults".
        let num_threads = cfg.num_threads.max(1);

        let vad_config = sherpa_onnx::VadModelConfig {
            silero_vad: sherpa_onnx::SileroVadModelConfig {
                model: Some(path_to_string(&cfg.vad_model)?),
                threshold: cfg.vad_threshold,
                min_silence_duration: cfg.min_silence_duration,
                min_speech_duration: cfg.min_speech_duration,
                window_size: WINDOW_SIZE as i32,
                max_speech_duration: cfg.max_speech_duration,
            },
            sample_rate: SAMPLE_RATE_HZ as i32,
            num_threads,
            provider: cfg.provider,
            debug: cfg.debug,
            ..Default::default()
        };
        // Scale the VAD's internal buffer to the configured max segment length
        // rather than a fixed constant: a caller raising `max_speech_duration`
        // past a hardcoded size would otherwise silently lose the audio that
        // overflows the circular buffer before force-finalisation kicks in.
        let vad_buffer_seconds = (cfg.max_speech_duration.max(0.0) + VAD_BUFFER_MARGIN_SECONDS)
            .max(VAD_BUFFER_MIN_SECONDS);
        let vad = sherpa_onnx::VoiceActivityDetector::create(&vad_config, vad_buffer_seconds)
            .ok_or_else(|| TranscriptionError::ModelLoad("failed to initialise VAD".to_owned()))?;

        Ok(Self { vad, inner })
    }

    /// Decode every VAD segment finalised so far, one at a time, re-mapping
    /// each result onto the absolute session clock, and drain the VAD queue.
    ///
    /// On an inner-engine error this returns before popping the failing
    /// segment, so it stays queued: the caller can retry `accept`/`finish`
    /// without losing that span's audio. The segments collected earlier in
    /// this call are discarded along with the error, matching the trait's
    /// all-or-nothing per-call contract.
    fn drain(&mut self, out: &mut Vec<Segment>) -> Result<()> {
        while let Some(seg) = self.vad.front() {
            let (span_start_ms, span_end_ms) = segment_ms(seg.start(), seg.n());

            let inner_segments = self.inner.transcribe(AudioChunk {
                samples: seg.samples(),
                sample_rate: SAMPLE_RATE_HZ,
            })?;

            for inner_segment in inner_segments {
                if let Some(mapped) = offset_into_span(inner_segment, span_start_ms, span_end_ms) {
                    out.push(mapped);
                }
            }

            self.vad.pop();
        }
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
