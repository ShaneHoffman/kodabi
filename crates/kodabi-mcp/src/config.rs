//! Startup configuration: where the index database and knowledge-base root live.
//!
//! Both are read from the environment so the future Tauri shell can inject them
//! into the spawned process (see the repo-root `.mcp.json`). They are distinct
//! paths by design: the index is a machine-local cache that must never live
//! inside the syncable knowledge base, so the two can diverge.

use std::path::PathBuf;

/// Paths the server resolves at startup.
pub struct ServerConfig {
    /// The SQLite note index (`KODABI_INDEX_DB`) — backs `search_notes` and
    /// `get_note`.
    pub index_db: PathBuf,
    /// The knowledge-base (vault) root (`KODABI_KB_ROOT`) — backs
    /// `list_projects`, which scans the folder tree on disk.
    pub kb_root: PathBuf,
}

impl ServerConfig {
    /// Reads `KODABI_INDEX_DB` and `KODABI_KB_ROOT`. Both are required; a missing
    /// or empty value is an error naming the variable.
    pub fn from_env() -> Result<Self, String> {
        Ok(Self {
            index_db: require_path("KODABI_INDEX_DB")?,
            kb_root: require_path("KODABI_KB_ROOT")?,
        })
    }
}

fn require_path(name: &str) -> Result<PathBuf, String> {
    match std::env::var_os(name) {
        Some(value) if !value.is_empty() => Ok(PathBuf::from(value)),
        _ => Err(format!(
            "{name} is unset; the MCP server needs it to locate the knowledge base"
        )),
    }
}
