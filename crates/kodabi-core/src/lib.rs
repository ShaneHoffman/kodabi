//! kodabi-core — the pure, testable data layer for Kodabi.
//!
//! Holds domain logic with no Tauri/UI dependencies so it can be unit-tested
//! in isolation. This is where the future SQLite index and MCP-facing query
//! surface will live, kept out of the desktop shell.

pub mod benchmark;
pub mod device;
pub mod distill;
pub mod glossary;
pub mod index;
pub mod llm;
pub mod metrics;
pub mod naming;
pub mod note;
pub mod pipeline;
pub mod raw_session;
pub mod retention;
pub mod routing;
pub mod settings;
pub mod transcription;
pub mod vault;

/// Crate version — a trivial placeholder proving the crate compiles, links,
/// and is unit-testable from the Tauri binary and from `cargo test`.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_not_empty() {
        assert!(!version().is_empty());
    }
}
