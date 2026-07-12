//! kodama-audio — WASAPI loopback (system audio) capture via cpal.
//!
//! Platform IO lives here rather than in `kodama-core`, which stays a pure,
//! UI-agnostic data layer. `src-tauri` commands are thin wrappers over the
//! public API this crate exposes.

mod capture;
mod convert;
mod error;
mod format;
mod frame;
mod meter;

pub use capture::LoopbackCapture;
pub use error::{AudioError, Result};
pub use format::{AudioFormat, SampleTag};
pub use frame::{AudioFrame, CaptureItem, SegmentReason};
pub use meter::MeterSnapshot;
