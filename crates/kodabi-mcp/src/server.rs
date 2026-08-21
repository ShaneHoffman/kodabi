//! The server's shared state: its resolved config and the open note index, or
//! the reason they could not be built.

use kodabi_core::index::NoteIndex;
use kodabi_core::ledger::Ledger;

use crate::config::ServerConfig;
use crate::protocol::RpcError;

/// The read backend: the resolved config plus the open note index.
///
/// The ledger is deliberately *not* held open here, unlike the index. It is
/// opened per call by [`Backend::open_ledger`], for two reasons: its mutators
/// take `&mut self` while a handler only ever sees `&Server`, and the desktop
/// app owns a long-lived writer on the same file — so holding a second handle
/// open for the life of a chat session would keep a WAL reader pinned for no
/// gain. An open costs one file handle and a `user_version` read.
pub struct Backend {
    pub config: ServerConfig,
    pub index: NoteIndex,
}

impl Backend {
    /// Opens the commitment ledger for one call.
    ///
    /// Refuses rather than inventing a path when `KODABI_LEDGER_DB` is unset:
    /// this process cannot know where the app keeps its config dir, and a
    /// guessed path would create a second, empty ledger that disagrees with the
    /// real one — the failure mode a person would never think to look for.
    pub fn open_ledger(&self) -> Result<Ledger, RpcError> {
        let path = self.config.ledger_db.as_ref().ok_or_else(|| {
            RpcError::internal(
                "KODABI_LEDGER_DB is unset; the MCP server needs it to reach the commitment ledger"
                    .to_string(),
            )
        })?;
        Ledger::open(path).map_err(|error| {
            RpcError::internal(format!(
                "failed to open ledger at {}: {error}",
                path.display()
            ))
        })
    }
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
