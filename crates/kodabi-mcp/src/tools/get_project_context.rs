//! `get_project_context`: a single-call briefing for one project.

use chrono::Local;
use serde_json::Value;

use kodabi_core::project_context::{self, ProjectContextParams};

use super::map_project_context_error;
use crate::envelope;
use crate::protocol::RpcError;
use crate::server::Server;

/// Assembles the briefing. A slug that names no project is a business error
/// (`isError`) — the caller asserted it exists. Unlike the list tools, this one
/// carries no cursor: each section is capped by its own `*_limit` and `counts`
/// reports the true totals.
pub fn call(server: &Server, arguments: Value) -> Result<Value, RpcError> {
    let backend = server.backend()?;
    let params: ProjectContextParams = serde_json::from_value(arguments).map_err(|error| {
        RpcError::invalid_params(format!("invalid get_project_context arguments: {error}"))
    })?;

    // The clock is read here, not in kodabi-core, so the outstanding items in
    // the briefing carry the same statuses `list_outstanding_items` would give.
    let today = Local::now().date_naive();

    match project_context::get_project_context(
        &backend.index,
        &backend.config.kb_root,
        &params,
        today,
    ) {
        Ok(context) => Ok(envelope::success(&context)),
        Err(error) => map_project_context_error(error),
    }
}
