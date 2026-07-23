//! Shared construction and feeding of sherpa-onnx's bundled Silero VAD. Both
//! [`ParakeetEngine`](crate::engine) (which gates its recognizer on a VAD) and
//! [`VadGate`](crate::vad) (which fronts an arbitrary engine with one) build
//! the *same* VAD the same way and feed it through [`feed_windowed`]; this is
//! the single place that knows how, so the window size, feeding discipline,
//! buffer sizing and thread clamp can't drift between them.
//!
//! Compiled whenever either native-engine feature that needs a Silero VAD is
//! on (`parakeet` builds its own; `whisper` co-enables `vad`).

use std::path::Path;

use kodabi_core::transcription::{Result, TranscriptionError};

use crate::validate::{clamp_threads, path_to_string, require_file, SAMPLE_RATE_HZ};

/// Silero VAD processing window, in samples. Handed to sherpa-onnx as its
/// `window_size`, and the granularity [`feed_windowed`] slices audio down to.
///
/// sherpa-onnx evaluates the Silero model per window internally, but its
/// segment state machine — speech-start estimation and finalisation — advances
/// only once per `accept_waveform` call, with "was speech" OR-ed across all of
/// the call's windows. Fed multi-second chunks it therefore estimates every
/// segment start near the chunk *tail* and finalises nothing until a fully
/// silent call, losing all speech but the tail (each capture "began" ~9.6 s
/// in). One window per call keeps the state machine at its designed
/// per-window granularity.
const WINDOW_SIZE: usize = 512;

/// Head-room added to `max_speech_duration` when sizing the VAD's internal
/// sample buffer, so a maximal speech segment plus the trailing silence needed
/// to finalise it always fits before the circular buffer starts dropping the
/// oldest samples.
const VAD_BUFFER_MARGIN_SECONDS: f32 = 10.0;

/// Floor for the VAD's internal buffer, covering a tiny or disabled
/// (`max_speech_duration <= 0`) force-finalise setting.
const VAD_BUFFER_MIN_SECONDS: f32 = 30.0;

/// The Silero VAD tuning knobs [`build_silero_vad`] needs, borrowed from
/// whichever engine config carries them (`ParakeetConfig` / `VadConfig`).
pub(crate) struct SileroParams<'a> {
    /// `silero_vad.onnx` on disk.
    pub model: &'a Path,
    /// Threads handed to the VAD (clamped to at least one internally).
    pub num_threads: i32,
    /// Inference provider, e.g. `Some("cpu")`; `None` lets sherpa-onnx choose.
    pub provider: Option<&'a str>,
    /// Speech-probability threshold (0.0–1.0).
    pub threshold: f32,
    /// Minimum silence (seconds) before a speech segment is finalised.
    pub min_silence_duration: f32,
    /// Minimum speech (seconds) before a segment is considered real speech.
    pub min_speech_duration: f32,
    /// Maximum speech (seconds) before a segment is force-finalised.
    pub max_speech_duration: f32,
    /// Forwarded to sherpa-onnx's own debug logging.
    pub debug: bool,
}

/// Build sherpa-onnx's bundled Silero VAD from `params`. A missing model file
/// or a sherpa-onnx init failure surfaces as [`TranscriptionError::ModelLoad`].
pub(crate) fn build_silero_vad(
    params: SileroParams<'_>,
) -> Result<sherpa_onnx::VoiceActivityDetector> {
    require_file(params.model)?;

    let vad_config = sherpa_onnx::VadModelConfig {
        silero_vad: sherpa_onnx::SileroVadModelConfig {
            model: Some(path_to_string(params.model)?),
            threshold: params.threshold,
            min_silence_duration: params.min_silence_duration,
            min_speech_duration: params.min_speech_duration,
            window_size: WINDOW_SIZE as i32,
            max_speech_duration: params.max_speech_duration,
        },
        sample_rate: SAMPLE_RATE_HZ as i32,
        num_threads: clamp_threads(params.num_threads),
        provider: params.provider.map(str::to_owned),
        debug: params.debug,
        ..Default::default()
    };
    // Scale the VAD's internal buffer to the configured max segment length
    // rather than a fixed constant: a caller raising `max_speech_duration`
    // past a hardcoded size would otherwise silently lose the audio that
    // overflows the circular buffer before force-finalisation kicks in.
    let vad_buffer_seconds = (params.max_speech_duration.max(0.0) + VAD_BUFFER_MARGIN_SECONDS)
        .max(VAD_BUFFER_MIN_SECONDS);
    sherpa_onnx::VoiceActivityDetector::create(&vad_config, vad_buffer_seconds)
        .ok_or_else(|| TranscriptionError::ModelLoad("failed to initialise VAD".to_owned()))
}

/// Feed `samples` to `vad` one [`WINDOW_SIZE`]-sample slice per
/// `accept_waveform` call — the granularity the VAD's per-call segment state
/// machine requires (see [`WINDOW_SIZE`]). Every accept site must go through
/// here; feeding a whole pipeline chunk directly reintroduces the lost-speech
/// bug.
///
/// The trailing sub-window slice goes in too: `chunks` (never `chunks_exact`)
/// is load-bearing, because sherpa-onnx buffers a partial window internally
/// across calls, so dropping the remainder would lose samples and drift the
/// VAD's sample clock away from the session clock.
pub(crate) fn feed_windowed(vad: &sherpa_onnx::VoiceActivityDetector, samples: &[f32]) {
    for window in samples.chunks(WINDOW_SIZE) {
        vad.accept_waveform(window);
    }
}
