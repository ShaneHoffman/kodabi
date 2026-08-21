//! Startup configuration: where the index database, knowledge-base root and
//! commitment ledger live, plus the few user settings the server cannot read.
//!
//! All of it is read from the environment so the Tauri shell can inject it into
//! the spawned process (see the repo-root `.mcp.json`). The three paths are
//! distinct by design: the index is a machine-local cache that must never live
//! inside the syncable knowledge base, and the ledger sits with the settings
//! files in the config dir rather than beside either.
//!
//! **Two are required and one is optional, deliberately.** The index and the
//! vault back every tool; without them the server has nothing to answer with.
//! The ledger backs only the commitment tools, and a `.mcp.json` written before
//! they existed (or by hand) names neither — so a missing `KODABI_LEDGER_DB`
//! leaves the other tools working and fails only the two that need it, naming
//! the variable. It is never *defaulted*: this process has no idea where the
//! app's config dir is, and inventing a path would create an empty second
//! ledger that silently disagrees with the real one.

use std::path::PathBuf;

use kodabi_core::ledger::AgingConfig;

/// Paths and settings the server resolves at startup.
pub struct ServerConfig {
    /// The SQLite note index (`KODABI_INDEX_DB`) — backs `search_notes` and
    /// `get_note`.
    pub index_db: PathBuf,
    /// The knowledge-base (vault) root (`KODABI_KB_ROOT`) — backs
    /// `list_projects`, which scans the folder tree on disk.
    pub kb_root: PathBuf,
    /// The commitment ledger (`KODABI_LEDGER_DB`) — backs `list_commitments`
    /// and `update_action_item`. `None` when the variable is unset.
    pub ledger_db: Option<PathBuf>,
    /// The user's aging thresholds, from `KODABI_AGING_AFTER_DAYS` and
    /// `KODABI_STALE_AFTER_DAYS`.
    ///
    /// Unlike the paths, an absent or unparseable value falls back to the
    /// documented defaults rather than failing: a tier is a shading on an
    /// answer, so getting it from the defaults is a cosmetic difference from
    /// the desktop view, where a wrong *path* would be a wrong answer.
    pub aging: AgingConfig,
}

impl ServerConfig {
    /// Reads the environment. `KODABI_INDEX_DB` and `KODABI_KB_ROOT` are
    /// required; a missing or empty value is an error naming the variable.
    pub fn from_env() -> Result<Self, String> {
        Ok(Self {
            index_db: require_path("KODABI_INDEX_DB")?,
            kb_root: require_path("KODABI_KB_ROOT")?,
            ledger_db: optional_path("KODABI_LEDGER_DB"),
            aging: AgingConfig {
                aging_after_days: days(
                    "KODABI_AGING_AFTER_DAYS",
                    AgingConfig::default().aging_after_days,
                ),
                stale_after_days: days(
                    "KODABI_STALE_AFTER_DAYS",
                    AgingConfig::default().stale_after_days,
                ),
            },
        })
    }
}

fn require_path(name: &str) -> Result<PathBuf, String> {
    optional_path(name).ok_or_else(|| {
        format!("{name} is unset; the MCP server needs it to locate the knowledge base")
    })
}

/// A path variable, treating empty as unset — the rule Kodabi applies to every
/// environment variable it reads.
fn optional_path(name: &str) -> Option<PathBuf> {
    match std::env::var_os(name) {
        Some(value) if !value.is_empty() => Some(PathBuf::from(value)),
        _ => None,
    }
}

/// A day-count variable, falling back to `fallback` when it is unset, empty, or
/// not a positive number.
fn days(name: &str, fallback: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|days| *days > 0)
        .unwrap_or(fallback)
}
