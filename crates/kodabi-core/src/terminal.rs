//! Pure builders for the embedded Claude Code terminal.
//!
//! The terminal view (Phase 3, FOUNDING_DOC §4) hosts an *interactive* `claude`
//! process wired to the `kodabi` MCP server, so chat-over-the-knowledge-base
//! works with zero setup. The side-effecting PTY spawn lives in the Tauri shell
//! (`src-tauri/src/terminal_cmds.rs`); everything here is pure and unit-tested
//! without a subprocess — the argv, the generated `.mcp.json` and Claude Code
//! settings, and the resize validation. This mirrors how `llm`'s pure
//! request/parse logic stays testable apart from `kodabi-llm`'s process spawn.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// The MCP server name, so its tools are namespaced `mcp__kodabi__<tool>`
/// (`docs/MCP_TOOL_SURFACE.md` §Server & wiring).
pub const MCP_SERVER_KEY: &str = "kodabi";

/// The three read tools, pre-approved so chat-over-the-KB needs no per-tool
/// permission prompt. The two write tools (`file_note_to_project`,
/// `add_glossary_term`) are deliberately omitted, so Claude Code still prompts
/// for them — there is a real TTY in the terminal to answer.
pub const READ_TOOL_PERMISSIONS: [&str; 3] = [
    "mcp__kodabi__search_notes",
    "mcp__kodabi__get_note",
    "mcp__kodabi__list_projects",
];

/// Set on the spawned `claude` process to disable its own transcript and
/// prompt-history writing (`~/.claude/projects/…`), so the in-app retention
/// promise holds for chat too (FOUNDING_DOC §3.7, the Phase 2 leftover). Scoped
/// to Kodabi's launch — a per-process env var, so it never touches the user's
/// global Claude Code config or their other sessions.
pub const SKIP_HISTORY_ENV: &str = "CLAUDE_CODE_SKIP_PROMPT_HISTORY";

/// The largest terminal grid we forward to the PTY. A resize can arrive with a
/// wild value from a detached or zero-size container; clamp rather than trust.
const MAX_DIMENSION: u16 = 1000;

/// Resolved, machine-local absolute paths the shell computes and hands in.
#[derive(Debug, Clone)]
pub struct McpPaths {
    /// Absolute path to the `kodabi-mcp` binary.
    pub mcp_binary: PathBuf,
    /// The SQLite note index (`KODABI_INDEX_DB`).
    pub index_db: PathBuf,
    /// The knowledge-base (vault) root (`KODABI_KB_ROOT`).
    pub kb_root: PathBuf,
}

#[derive(Serialize)]
struct McpServerEntry {
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct McpConfigFile {
    #[serde(rename = "mcpServers")]
    mcp_servers: BTreeMap<String, McpServerEntry>,
}

/// Builds the `.mcp.json` body that wires the `kodabi` stdio server into Claude
/// Code, mirroring `.mcp.json.example`: `command` is the resolved binary, `args`
/// is empty, and `env` carries the two paths `kodabi-mcp` reads at startup
/// (`crates/kodabi-mcp/src/config.rs`). An entry with no `type`/`url` field is
/// read by Claude Code as a stdio server.
pub fn mcp_config_json(paths: &McpPaths) -> serde_json::Result<String> {
    let mut env = BTreeMap::new();
    env.insert("KODABI_INDEX_DB".to_owned(), path_string(&paths.index_db));
    env.insert("KODABI_KB_ROOT".to_owned(), path_string(&paths.kb_root));

    let mut servers = BTreeMap::new();
    servers.insert(
        MCP_SERVER_KEY.to_owned(),
        McpServerEntry {
            command: path_string(&paths.mcp_binary),
            args: Vec::new(),
            env,
        },
    );

    serde_json::to_string_pretty(&McpConfigFile {
        mcp_servers: servers,
    })
}

#[derive(Serialize)]
struct Permissions {
    allow: Vec<String>,
}

#[derive(Serialize)]
struct SettingsFile {
    permissions: Permissions,
}

/// Builds the Claude Code settings body passed via `--settings`, pre-allowing
/// exactly the read tools ([`READ_TOOL_PERMISSIONS`]) so chat-over-the-KB works
/// with no permission prompt. Writes are not listed, so Claude Code still
/// prompts for them.
pub fn settings_json() -> serde_json::Result<String> {
    serde_json::to_string_pretty(&SettingsFile {
        permissions: Permissions {
            allow: READ_TOOL_PERMISSIONS
                .iter()
                .map(|tool| (*tool).to_owned())
                .collect(),
        },
    })
}

/// The argv (after the program name) for an *interactive* `claude` launch wired
/// to the generated MCP config and settings. Deliberately the inverse of the
/// headless runner in `kodabi-llm`: no `-p`, no `--output-format json` — there
/// is a real terminal, and a write's permission prompt has a TTY to answer.
///
/// `--strict-mcp-config` loads only Kodabi's server, ignoring any user or
/// project `.mcp.json`. (The headless runner uses it too, verified against a
/// live `claude`; if a future CLI drops it, this is the one line to change.)
/// The knowledge-base root, the child env, and the working directory are the
/// shell's job — they are not argv.
pub fn claude_argv(mcp_config: &Path, settings: &Path) -> Vec<OsString> {
    vec![
        OsString::from("--mcp-config"),
        mcp_config.as_os_str().to_os_string(),
        OsString::from("--strict-mcp-config"),
        OsString::from("--settings"),
        settings.as_os_str().to_os_string(),
    ]
}

/// Validates a resize request from the frontend: rejects a zero dimension (a
/// PTY can't be 0 wide/tall, and xterm briefly reports 0 before first layout),
/// and clamps to [`MAX_DIMENSION`]. `None` means "ignore this resize".
pub fn valid_resize(cols: u16, rows: u16) -> Option<(u16, u16)> {
    if cols == 0 || rows == 0 {
        return None;
    }
    Some((cols.min(MAX_DIMENSION), rows.min(MAX_DIMENSION)))
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_paths() -> McpPaths {
        McpPaths {
            mcp_binary: PathBuf::from("C:/app/resources/kodabi-mcp.exe"),
            index_db: PathBuf::from("C:/app-data/index.db"),
            kb_root: PathBuf::from("C:/vault"),
        }
    }

    #[test]
    fn mcp_config_has_the_documented_wiring_shape() {
        let json = mcp_config_json(&sample_paths()).expect("serializes");
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid json");

        let server = &value["mcpServers"]["kodabi"];
        assert_eq!(server["command"], "C:/app/resources/kodabi-mcp.exe");
        assert_eq!(server["args"], serde_json::json!([]));
        assert_eq!(server["env"]["KODABI_INDEX_DB"], "C:/app-data/index.db");
        assert_eq!(server["env"]["KODABI_KB_ROOT"], "C:/vault");
        // No `type`/`url` field → Claude Code reads it as a stdio server.
        assert!(server.get("type").is_none());
        assert!(server.get("url").is_none());
    }

    #[test]
    fn settings_allow_the_read_tools_and_no_write_tool() {
        let json = settings_json().expect("serializes");
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        let allow = value["permissions"]["allow"]
            .as_array()
            .expect("allow is an array");

        for read_tool in READ_TOOL_PERMISSIONS {
            assert!(
                allow.iter().any(|entry| entry == read_tool),
                "expected {read_tool} to be pre-approved"
            );
        }
        // The write tools must NOT be pre-approved — they still prompt.
        for write_tool in [
            "mcp__kodabi__file_note_to_project",
            "mcp__kodabi__add_glossary_term",
        ] {
            assert!(
                !allow.iter().any(|entry| entry == write_tool),
                "write tool {write_tool} must not be pre-approved"
            );
        }
    }

    #[test]
    fn claude_argv_is_interactive_and_wires_the_config() {
        let argv = claude_argv(
            Path::new("C:/cfg/kodabi.mcp.json"),
            Path::new("C:/cfg/settings.json"),
        );

        assert!(argv.iter().any(|arg| arg == "--mcp-config"));
        assert!(argv.iter().any(|arg| arg == "C:/cfg/kodabi.mcp.json"));
        assert!(argv.iter().any(|arg| arg == "--settings"));
        assert!(argv.iter().any(|arg| arg == "C:/cfg/settings.json"));
        assert!(argv.iter().any(|arg| arg == "--strict-mcp-config"));

        // Interactive: the headless hermetic flags must be absent.
        assert!(!argv.iter().any(|arg| arg == "-p"));
        assert!(!argv.iter().any(|arg| arg == "--output-format"));
    }

    #[test]
    fn resize_rejects_zero_dimensions() {
        assert_eq!(valid_resize(0, 24), None);
        assert_eq!(valid_resize(80, 0), None);
    }

    #[test]
    fn resize_passes_through_and_clamps() {
        assert_eq!(valid_resize(80, 24), Some((80, 24)));
        assert_eq!(
            valid_resize(5000, 5000),
            Some((MAX_DIMENSION, MAX_DIMENSION))
        );
    }
}
