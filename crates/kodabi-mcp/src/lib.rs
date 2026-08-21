//! Kodabi's stdio MCP server — a hand-rolled JSON-RPC 2.0 shell exposing the
//! tool surface of `docs/MCP_TOOL_SURFACE.md` over `kodabi-core`.
//!
//! The server name is `kodabi`; it speaks newline-delimited JSON-RPC on
//! stdin/stdout and exposes ten tools: seven read (`search_notes`, `get_note`,
//! `get_meeting_transcript`, `list_outstanding_items`, `list_commitments`,
//! `list_projects`, `get_project_context`) and three write
//! (`file_note_to_project`, `add_glossary_term`, `update_action_item`). Tool
//! logic lives in `kodabi-core` (the core-vs-shell rule); this crate is protocol
//! plumbing plus per-tool schema/envelope handling.
//!
//! # Configuration
//!
//! Read from the environment at startup. Two paths are required:
//!
//! - `KODABI_INDEX_DB` — the SQLite note index (backs `search_notes`, `get_note`,
//!   and `list_outstanding_items`).
//! - `KODABI_KB_ROOT` — the knowledge-base (vault) root (backs `list_projects`,
//!   the transcript and glossary/README reads of `get_meeting_transcript` and
//!   `get_project_context`, and the write tools, which read and mutate the
//!   vault files directly).
//!
//! They are distinct by design: the index is a machine-local cache that must not
//! live inside the syncable knowledge base. The Tauri shell injects them into
//! the spawned process via the `.mcp.json` `env` block. If either is missing,
//! the server still starts (so `initialize`/`tools/list` work) and each
//! `tools/call` returns an internal error naming the unset variable.
//!
//! Three more are optional:
//!
//! - `KODABI_LEDGER_DB` — the commitment ledger, which lives with the settings
//!   files in the app's config dir rather than beside the index, so neither path
//!   above locates it. Backs `list_commitments`, `update_action_item`, and the
//!   `commitments` counts of `get_project_context`. Optional so a `.mcp.json`
//!   written before these tools existed keeps working: unset, the two
//!   commitment tools return an error naming it and every other tool is
//!   unaffected. Never defaulted — this process cannot know where the app keeps
//!   its config dir, and a guessed path would silently create a second, empty
//!   ledger.
//! - `KODABI_AGING_AFTER_DAYS` / `KODABI_STALE_AFTER_DAYS` — the user's aging
//!   thresholds, which live in `settings.toml`. Unlike the paths, an absent or
//!   unparseable value falls back to the documented defaults (14 and 30): a tier
//!   is a shading on an answer, where a wrong path would be a wrong answer.
//!
//! # Wiring (`.mcp.json`)
//!
//! ```json
//! { "mcpServers": { "kodabi": { "command": "<path-to-kodabi-mcp>", "args": [],
//!   "env": { "KODABI_INDEX_DB": "", "KODABI_KB_ROOT": "",
//!            "KODABI_LEDGER_DB": "" } } } }
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
