//! Error type for `kodabi-audio`.

use crate::source::CaptureSource;

/// Errors that can occur while negotiating or running an audio capture stream.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no default {} device available", .0.device_kind())]
    NoDefaultDevice(CaptureSource),

    #[error("failed to query default {kind} device config: {msg}", kind = .0.device_kind(), msg = .1)]
    DefaultConfig(CaptureSource, String),

    /// Playback (not capture) found no default output device — kept distinct
    /// from `NoDefaultDevice(CaptureSource::Loopback)` since that variant
    /// means "no render endpoint to capture as loopback", a different intent
    /// than "nothing to play a test tone through".
    #[error("no default output device available")]
    NoDefaultOutputDevice,

    #[error("failed to query default output device config: {0}")]
    DefaultOutputConfig(String),

    #[error("unsupported device sample format: {0:?}")]
    UnsupportedFormat(cpal::SampleFormat),

    #[error("failed to build capture stream: {0}")]
    BuildStream(String),

    #[error("failed to start stream: {0}")]
    Play(String),

    #[error("capture thread failed to start")]
    ThreadStart,

    #[error("failed to build resampler: {0}")]
    Resample(String),

    #[error("capture state poisoned")]
    StatePoisoned,

    #[error("capture start thread panicked")]
    StartThreadPanicked,
}

pub type Result<T> = std::result::Result<T, AudioError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_are_human_readable() {
        assert_eq!(
            AudioError::NoDefaultDevice(CaptureSource::Loopback).to_string(),
            "no default output device available"
        );
        assert_eq!(
            AudioError::NoDefaultDevice(CaptureSource::Microphone).to_string(),
            "no default microphone device available"
        );
        assert_eq!(
            AudioError::DefaultConfig(CaptureSource::Microphone, "boom".to_string()).to_string(),
            "failed to query default microphone device config: boom"
        );
        assert_eq!(
            AudioError::BuildStream("boom".to_string()).to_string(),
            "failed to build capture stream: boom"
        );
    }
}
