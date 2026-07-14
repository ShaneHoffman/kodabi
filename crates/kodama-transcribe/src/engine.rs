//! [`ParakeetEngine`] — NVIDIA Parakeet TDT via sherpa-onnx, VAD-gated so
//! silence is never fed to the recognizer.
//!
//! Silence-safety comes from the architecture, not from post-filtering: audio
//! is fed to sherpa-onnx's bundled Silero VAD, and the offline recognizer
//! only ever runs on a VAD-finalised speech segment. Over a silent stretch
//! the VAD finalises nothing, so the recognizer is never invoked and no
//! phantom text is produced.

use std::path::PathBuf;

use kodama_core::transcription::{
    AudioChunk, Result, Segment, TranscriptionEngine, TranscriptionError,
};

use crate::validate::{path_to_string, require_file, segment_ms, validate_chunk, SAMPLE_RATE_HZ};

/// Silero VAD processing window, in samples. Handed to sherpa-onnx as its
/// `window_size`; the VAD buffers input internally and slices it into windows
/// itself, so callers may feed arbitrary-length chunks.
const WINDOW_SIZE: usize = 512;

/// Head-room added to `max_speech_duration` when sizing the VAD's internal
/// sample buffer, so a maximal speech segment plus the trailing silence needed
/// to finalise it always fits before the circular buffer starts dropping the
/// oldest samples.
const VAD_BUFFER_MARGIN_SECONDS: f32 = 10.0;

/// Floor for the VAD's internal buffer, covering a tiny or disabled
/// (`max_speech_duration <= 0`) force-finalise setting.
const VAD_BUFFER_MIN_SECONDS: f32 = 30.0;

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
            provider: cfg.provider.clone(),
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
