//! kodabi-audio — parallel system-audio (WASAPI loopback) and microphone
//! capture via cpal.
//!
//! Platform IO lives here rather than in `kodabi-core`, which stays a pure,
//! UI-agnostic data layer. `src-tauri` commands are thin wrappers over the
//! public API this crate exposes.

mod capture;
mod combine;
mod convert;
mod drift;
mod error;
mod format;
mod frame;
mod meter;
mod mix;
mod resample;
mod session;
mod source;
mod spill;
mod tuning;

pub use capture::Capture;
pub use combine::{AlignedSession, CombinedSession, Combiner, SessionChannel, SpilledSession};
pub use convert::levels;
pub use error::{AudioError, Result};
pub use format::{AudioFormat, SampleTag};
pub use frame::{AudioFrame, CaptureItem, SegmentReason};
pub use meter::MeterSnapshot;
pub use resample::{MonoResampler, ResampleParams};
pub use session::{CaptureTuning, DualCapture, DualHealth, DualStatus, SourceHealth, SourceStatus};
pub use source::CaptureSource;
pub use spill::{SpillConfig, SpillReader};
