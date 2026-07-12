//! What a capture stream captures from. This is the only place device
//! selection differs between capture sources — everything else in `capture`
//! (thread lifecycle, channel, meter, sample-format dispatch) is shared.

use cpal::traits::{DeviceTrait, HostTrait};

use crate::error::{AudioError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureSource {
    /// System audio, captured as an input stream on the default *output*
    /// device — cpal's WASAPI backend detects the render endpoint and sets
    /// the loopback flag automatically; there is no separate "loopback
    /// device".
    Loopback,
    /// The default microphone (input device).
    Microphone,
}

impl CaptureSource {
    /// Resolve this source to a concrete device and its default config.
    pub fn resolve(self, host: &cpal::Host) -> Result<(cpal::Device, cpal::SupportedStreamConfig)> {
        let device = match self {
            CaptureSource::Loopback => host.default_output_device(),
            CaptureSource::Microphone => host.default_input_device(),
        }
        .ok_or(AudioError::NoDefaultDevice(self))?;

        let supported = match self {
            CaptureSource::Loopback => device.default_output_config(),
            CaptureSource::Microphone => device.default_input_config(),
        }
        .map_err(|e| AudioError::DefaultConfig(e.to_string()))?;

        Ok((device, supported))
    }

    /// Name for the dedicated capture thread this source runs on.
    pub fn thread_name(self) -> &'static str {
        match self {
            CaptureSource::Loopback => "kodama-audio-loopback",
            CaptureSource::Microphone => "kodama-audio-microphone",
        }
    }

    /// Human-readable device kind, for error messages.
    pub(crate) fn device_kind(self) -> &'static str {
        match self {
            CaptureSource::Loopback => "output",
            CaptureSource::Microphone => "microphone",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_names_are_source_specific() {
        assert_eq!(
            CaptureSource::Loopback.thread_name(),
            "kodama-audio-loopback"
        );
        assert_eq!(
            CaptureSource::Microphone.thread_name(),
            "kodama-audio-microphone"
        );
    }

    #[test]
    fn device_kinds_are_source_specific() {
        assert_eq!(CaptureSource::Loopback.device_kind(), "output");
        assert_eq!(CaptureSource::Microphone.device_kind(), "microphone");
    }
}
