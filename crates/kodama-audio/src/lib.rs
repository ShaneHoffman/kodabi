//! kodama-audio — parallel system-audio (WASAPI loopback) and microphone
//! capture via cpal.
//!
//! Platform IO lives here rather than in `kodama-core`, which stays a pure,
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

pub use capture::Capture;
pub use combine::{AlignedSession, Combiner, SessionChannel};
pub use convert::levels;
pub use error::{AudioError, Result};
pub use format::{AudioFormat, SampleTag};
pub use frame::{AudioFrame, CaptureItem, SegmentReason};
pub use meter::MeterSnapshot;
pub use session::{DualCapture, DualStatus, SourceStatus};
pub use source::CaptureSource;
