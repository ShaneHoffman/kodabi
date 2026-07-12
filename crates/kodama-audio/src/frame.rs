//! The items that flow out of a capture session: sample chunks and segment
//! boundaries. A segment boundary marks a point where the format may have
//! changed (e.g. the default output device was switched mid-capture), so
//! consumers always know which format applies to the frames that follow it.

use crate::format::AudioFormat;

/// Why a new segment started.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentReason {
    /// The first segment of a capture session.
    Start,
    /// The default device for this stream changed mid-capture and the
    /// stream was rebuilt on the new device.
    DeviceChanged,
}

/// One cpal callback's worth of interleaved `f32` samples, tagged with the
/// segment it belongs to and the `format` that applies to it. Carrying the
/// format on every frame keeps a frame self-describing even if the
/// `SegmentStarted` marker for its segment was dropped by a saturated
/// channel — the format is never lost.
#[derive(Clone, Debug)]
pub struct AudioFrame {
    pub segment_id: u32,
    pub samples: Vec<f32>,
    pub format: AudioFormat,
}

impl AudioFrame {
    /// Number of complete sample frames (samples / channels).
    pub fn frames(&self) -> usize {
        let channels = self.format.channels;
        if channels == 0 {
            0
        } else {
            self.samples.len() / channels as usize
        }
    }
}

/// What flows through a capture session's channel.
#[derive(Clone, Debug)]
pub enum CaptureItem {
    /// A new segment has started; `format` applies to every `Frame` that
    /// follows until the next `SegmentStarted`.
    SegmentStarted {
        segment_id: u32,
        format: AudioFormat,
        reason: SegmentReason,
    },
    Frame(AudioFrame),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::SampleTag;

    fn format_with(channels: u16) -> AudioFormat {
        AudioFormat {
            sample_rate: 48_000,
            channels,
            source_format: SampleTag::F32,
        }
    }

    #[test]
    fn frame_count_divides_by_channels() {
        let frame = AudioFrame {
            segment_id: 0,
            samples: vec![0.0; 8],
            format: format_with(2),
        };
        assert_eq!(frame.frames(), 4);
    }

    #[test]
    fn frame_count_is_zero_for_zero_channels() {
        let frame = AudioFrame {
            segment_id: 0,
            samples: vec![0.0; 8],
            format: format_with(0),
        };
        assert_eq!(frame.frames(), 0);
    }

    #[test]
    fn segment_started_carries_format_and_reason() {
        let item = CaptureItem::SegmentStarted {
            segment_id: 1,
            format: AudioFormat {
                sample_rate: 44_100,
                channels: 1,
                source_format: SampleTag::I16,
            },
            reason: SegmentReason::DeviceChanged,
        };
        match item {
            CaptureItem::SegmentStarted { reason, .. } => {
                assert_eq!(reason, SegmentReason::DeviceChanged);
            }
            CaptureItem::Frame(_) => panic!("expected SegmentStarted"),
        }
    }
}
