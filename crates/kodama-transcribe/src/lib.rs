//! Kodama transcription engines that need a native/FFI dependency.
//!
//! Isolated from `kodama-core` (which stays pure and UI-agnostic) the same
//! way `kodama-audio` isolates its native audio dependency. [`validate`] is
//! always compiled and dependency-free; [`engine`] pulls in `sherpa-onnx`
//! and only compiles behind the `parakeet` feature, so the default
//! `cargo build/clippy/test --workspace` stays native-free.

pub mod validate;

#[cfg(feature = "parakeet")]
mod engine;

#[cfg(feature = "parakeet")]
pub use engine::{ParakeetConfig, ParakeetEngine};
