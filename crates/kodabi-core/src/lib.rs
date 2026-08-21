//! kodabi-core — the pure, testable data layer for Kodabi.
//!
//! Holds domain logic with no Tauri/UI dependencies so it can be unit-tested
//! in isolation. This is where the future SQLite index and MCP-facing query
//! surface will live, kept out of the desktop shell.

pub mod benchmark;
pub mod capture;
pub mod category_examples;
pub mod chat;
pub mod chat_distill;
pub mod chats;
pub mod device;
pub mod distill;
pub mod embed;
pub mod glossary;
pub mod index;
pub mod inflight;
pub mod ledger;
pub mod llm;
pub mod meeting;
pub mod metrics;
pub mod models;
pub mod naming;
pub mod note;
pub mod overlay;
pub mod pipeline;
pub mod project_context;
pub mod raw_session;
pub mod reconcile;
pub mod retention;
pub mod routing;
pub mod routing_examples;
pub mod sandbox;
pub mod sessions;
pub mod settings;
pub mod terminal;
pub mod transcription;
pub mod vault;
pub mod watch;

/// How long a SQLite connection waits for a lock before giving up, in
/// milliseconds. Applied by both database openers ([`index::NoteIndex`] and
/// [`ledger::Ledger`]).
///
/// Both files are opened by more than one process: the desktop app writes them
/// while the MCP server, a separate process, reads the index and writes the
/// ledger. rusqlite's default is zero — the loser of a race fails instantly
/// with `SQLITE_BUSY` rather than waiting. Five seconds is far longer than any
/// write here takes (single-row updates inside WAL) and far shorter than a user
/// would tolerate a hang, so it converts a spurious error into a brief wait
/// without masking a genuine deadlock.
pub const BUSY_TIMEOUT_MS: u32 = 5_000;

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
