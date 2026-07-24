//! The server's shared state: its resolved config and the open note index, or
//! the reason they could not be built.

use kodabi_core::index::NoteIndex;

use crate::config::ServerConfig;
use crate::protocol::RpcError;

/// The read backend: the resolved config plus the open note index.
pub struct Backend {
    pub config: ServerConfig,
    pub index: NoteIndex,
}

/// The MCP server. Holds the backend, or the startup error that prevented it —
/// so `initialize`/`tools/list` still succeed and each `tools/call` reports the
/// problem, rather than the process dying on a silent broken pipe.
pub struct Server {
    backend: Result<Backend, String>,
}

impl Server {
    /// Builds the server from the environment (`KODABI_INDEX_DB`,
    /// `KODABI_KB_ROOT`). A failure is captured, not fatal.
    pub fn from_env() -> Self {
        Self {
            backend: build_backend(),
        }
    }

    /// The startup error, if the backend could not be built.
    pub fn init_error(&self) -> Option<&str> {
        self.backend.as_ref().err().map(String::as_str)
    }

    /// The backend, or a `-32603` error naming why it is unavailable.
    pub fn backend(&self) -> Result<&Backend, RpcError> {
        self.backend
            .as_ref()
            .map_err(|message| RpcError::internal(message.clone()))
    }

    /// Test constructor: a server with a ready backend.
    #[cfg(test)]
    pub fn with_backend(config: ServerConfig, index: NoteIndex) -> Self {
        Self {
            backend: Ok(Backend { config, index }),
        }
    }

    /// Test constructor: a server whose backend never built, for framing tests
    /// that never reach `tools/call`.
    #[cfg(test)]
    pub fn without_backend() -> Self {
        Self {
            backend: Err("no backend configured (test)".to_string()),
        }
    }
}

fn build_backend() -> Result<Backend, String> {
    let config = ServerConfig::from_env()?;
    let index = NoteIndex::open(&config.index_db).map_err(|error| {
        format!(
            "failed to open index at {}: {error}",
            config.index_db.display()
        )
    })?;
    Ok(Backend { config, index })
}
