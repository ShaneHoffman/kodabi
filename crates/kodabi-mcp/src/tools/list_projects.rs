//! `list_projects`: disk-backed enumeration of routing-target projects.

use serde_json::Value;

use kodabi_core::vault::{self, ProjectQuery};

use super::map_note_error;
use crate::envelope;
use crate::protocol::RpcError;
use crate::server::Server;

/// Enumerates projects with hierarchy, counts, and pagination. An empty match
/// (e.g. an unknown `parent`) is a successful empty page, not an error.
pub fn call(server: &Server, arguments: Value) -> Result<Value, RpcError> {
    let backend = server.backend()?;
    let query: ProjectQuery = serde_json::from_value(arguments).map_err(|error| {
        RpcError::invalid_params(format!("invalid list_projects arguments: {error}"))
    })?;

    match vault::list_projects_page(&backend.config.kb_root, &query) {
        Ok(page) => Ok(envelope::success(&page)),
        Err(error) => Err(map_note_error(error)),
    }
}
