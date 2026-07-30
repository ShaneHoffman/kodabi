//! `list_outstanding_items`: not-done action items across notes, each linked
//! back to its source note (a meeting or a chat).

use chrono::Local;
use serde_json::Value;

use kodabi_core::index::OutstandingParams;

use super::map_index_error;
use crate::envelope;
use crate::protocol::RpcError;
use crate::server::Server;

/// Lists action items matching the filters. A filter that matches nothing —
/// including an unknown project or source note — is a successful empty page,
/// not an error: absence is a valid answer.
pub fn call(server: &Server, arguments: Value) -> Result<Value, RpcError> {
    let backend = server.backend()?;
    let params: OutstandingParams = serde_json::from_value(arguments).map_err(|error| {
        RpcError::invalid_params(format!("invalid list_outstanding_items arguments: {error}"))
    })?;

    // The clock is read here, not in kodabi-core: `overdue` is derived against
    // the device's local calendar day, matching how due dates are written.
    let today = Local::now().date_naive();

    match backend.index.list_outstanding_items(&params, today) {
        Ok(results) => Ok(envelope::success(&results)),
        Err(error) => Err(map_index_error(error)),
    }
}
