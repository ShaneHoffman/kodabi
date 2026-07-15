//! Kodabi transcription engines that need a native/FFI dependency.
//!
//! Isolated from `kodabi-core` (which stays pure and UI-agnostic) the same
//! way `kodabi-audio` isolates its native audio dependency. [`validate`] is
//! always compiled and dependency-free; [`engine`] pulls in `sherpa-onnx`
//! and only compiles behind the `parakeet` feature; [`vad`] also pulls in
//! `sherpa-onnx` (for its bundled Silero VAD) behind the `vad` feature; and
//! [`whisper`] pulls in `whisper-rs` behind the `whisper`/`whisper-cuda`
//! features. `whisper` always enables `vad` (whisper.cpp has no bundled VAD
//! and hallucinates over silence — see [`vad::VadGate`]), so the default
//! `cargo build/clippy/test --workspace` stays native-free while any Whisper
//! build is compile-enforced to bring its VAD gate along.

pub mod validate;

// Shared Silero-VAD construction for both the Parakeet engine (which builds
// its own) and the `VadGate` (which fronts an arbitrary engine); `parakeet`
// or `vad` — and `whisper`, which co-enables `vad` — pulls it in.
#[cfg(any(feature = "parakeet", feature = "vad"))]
mod silero;

#[cfg(feature = "parakeet")]
mod engine;

#[cfg(feature = "parakeet")]
pub use engine::{ParakeetConfig, ParakeetEngine};

#[cfg(feature = "vad")]
mod vad;

#[cfg(feature = "vad")]
pub use vad::{VadConfig, VadGate};

#[cfg(feature = "whisper")]
mod whisper;

#[cfg(feature = "whisper")]
pub use whisper::{whisper_with_vad, WhisperConfig, WhisperEngine};
