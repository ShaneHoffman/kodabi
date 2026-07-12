//! Error type for `kodama-audio`.

/// Errors that can occur while negotiating or running an audio capture stream.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no default output device available")]
    NoDefaultOutputDevice,

    #[error("failed to query default output config: {0}")]
    DefaultConfig(String),

    #[error("unsupported device sample format: {0:?}")]
    UnsupportedFormat(cpal::SampleFormat),

    #[error("failed to build loopback stream: {0}")]
    BuildStream(String),

    #[error("failed to start stream: {0}")]
    Play(String),

    #[error("capture thread failed to start")]
    ThreadStart,
}

pub type Result<T> = std::result::Result<T, AudioError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_are_human_readable() {
        assert_eq!(
            AudioError::NoDefaultOutputDevice.to_string(),
            "no default output device available"
        );
        assert_eq!(
            AudioError::BuildStream("boom".to_string()).to_string(),
            "failed to build loopback stream: boom"
        );
    }
}
