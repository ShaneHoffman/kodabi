//! Kodabi's stdio MCP server — a hand-rolled JSON-RPC 2.0 shell exposing the
//! tool surface of `docs/MCP_TOOL_SURFACE.md` over `kodabi-core`.
//!
//! The server name is `kodabi`; it speaks newline-delimited JSON-RPC on
//! stdin/stdout and exposes eight tools: six read (`search_notes`, `get_note`,
//! `get_meeting_transcript`, `list_outstanding_items`, `list_projects`,
//! `get_project_context`) and two write (`file_note_to_project`,
//! `add_glossary_term`, the human correction loop). Tool logic lives in
//! `kodabi-core` (the core-vs-shell rule); this crate is protocol plumbing plus
//! per-tool schema/envelope handling.
//!
//! # Configuration
//!
//! Two paths are read from the environment at startup:
//!
//! - `KODABI_INDEX_DB` — the SQLite note index (backs `search_notes`, `get_note`,
//!   and `list_outstanding_items`).
//! - `KODABI_KB_ROOT` — the knowledge-base (vault) root (backs `list_projects`,
//!   the transcript and glossary/README reads of `get_meeting_transcript` and
//!   `get_project_context`, and the two write tools, which read and mutate the
//!   vault files directly).
//!
//! They are distinct by design: the index is a machine-local cache that must not
//! live inside the syncable knowledge base. The future Tauri shell injects both
//! into the spawned process via the `.mcp.json` `env` block. If either is
//! missing, the server still starts (so `initialize`/`tools/list` work) and each
//! `tools/call` returns an internal error naming the unset variable.
//!
//! # Wiring (`.mcp.json`)
//!
//! ```json
//! { "mcpServers": { "kodabi": { "command": "<path-to-kodabi-mcp>", "args": [],
//!   "env": { "KODABI_INDEX_DB": "", "KODABI_KB_ROOT": "" } } } }
//! ```
//!
//! An entry with no `type`/`url` is read by Claude Code as a stdio server; its
//! tools are namespaced `mcp__kodabi__<tool>`.

mod config;
mod dispatch;
mod envelope;
mod protocol;
mod schemas;
mod server;
mod tools;

use server::Server;

/// Runs the server: builds state from the environment, then serves JSON-RPC over
/// stdio until stdin closes. Returns a process exit code (`0` on clean EOF).
pub fn run() -> i32 {
    let server = Server::from_env();
    if let Some(error) = server.init_error() {
        // Surface misconfiguration on stderr now; the loop still runs so a client
        // gets a clear per-call error instead of a dead pipe. Stdout stays clean.
        eprintln!("kodabi-mcp: {error}");
    }

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();

    match protocol::serve(&server, stdin.lock(), &mut writer) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("kodabi-mcp: fatal I/O error: {error}");
            1
        }
    }
}
