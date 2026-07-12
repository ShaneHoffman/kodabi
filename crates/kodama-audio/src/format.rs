//! Audio format description — what a capture stream negotiated with the device.

use cpal::SampleFormat;

/// The device's native sample representation. Capture always *emits* `f32`
/// regardless of this tag; it exists so downstream consumers can tell what
/// precision the source device actually had.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SampleTag {
    F32,
    I16,
    U16,
    I32,
    Other,
}

impl From<SampleFormat> for SampleTag {
    fn from(format: SampleFormat) -> Self {
        match format {
            SampleFormat::F32 => SampleTag::F32,
            SampleFormat::I16 => SampleTag::I16,
            SampleFormat::U16 => SampleTag::U16,
            SampleFormat::I32 => SampleTag::I32,
            _ => SampleTag::Other,
        }
    }
}

/// The negotiated shape of a PCM stream: how many samples per second, how
/// many interleaved channels, and what the device's native format was.
/// Resampling to a specific rate/channel count for a transcription engine is
/// a downstream concern — this struct just tells the truth about the source.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub source_format: SampleTag,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_tag_from_cpal_format() {
        assert_eq!(SampleTag::from(SampleFormat::F32), SampleTag::F32);
        assert_eq!(SampleTag::from(SampleFormat::I16), SampleTag::I16);
        assert_eq!(SampleTag::from(SampleFormat::U16), SampleTag::U16);
        assert_eq!(SampleTag::from(SampleFormat::I32), SampleTag::I32);
        assert_eq!(SampleTag::from(SampleFormat::U8), SampleTag::Other);
    }

    #[test]
    fn audio_format_serializes() {
        let format = AudioFormat {
            sample_rate: 48_000,
            channels: 2,
            source_format: SampleTag::F32,
        };
        let json = serde_json::to_string(&format).unwrap();
        assert_eq!(
            json,
            r#"{"sample_rate":48000,"channels":2,"source_format":"f32"}"#
        );
    }
}
