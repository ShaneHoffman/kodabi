//! `list_commitments`: the commitment ledger's working set — what the user is
//! actually tracking, as opposed to every line a note happens to contain.

use chrono::Local;
use serde_json::Value;

use kodabi_core::ledger::commitments::{self, CommitmentsParams};

use super::map_commitments_error;
use crate::envelope;
use crate::protocol::RpcError;
use crate::server::Server;

/// Lists tracked commitments matching the filters.
///
/// The companion to `list_outstanding_items`, and deliberately a separate tool:
/// that one answers "what did they commit to" from raw extraction, this one
/// answers "what am I tracking" from the ledger, which knows what has since
/// been closed, waived, superseded or taken out of the working set. Both read
/// the same commitments; only this one reflects the person's judgements about
/// them, so it renders exactly what the desktop Commitments view renders.
///
/// A filter that matches nothing — including an unknown project or note — is a
/// successful empty page, not an error: absence is a valid answer.
pub fn call(server: &Server, arguments: Value) -> Result<Value, RpcError> {
    let backend = server.backend()?;
    let params: CommitmentsParams = serde_json::from_value(arguments).map_err(|error| {
        RpcError::invalid_params(format!("invalid list_commitments arguments: {error}"))
    })?;

    let ledger = backend.open_ledger()?;

    // The clock is read here, not in kodabi-core: an aging tier and a lapsed
    // snooze are both judged against the device's local calendar day, the same
    // day the due dates in `list_outstanding_items` are written in.
    let today = Local::now().date_naive();

    match commitments::list_commitments(
        &ledger,
        &backend.index,
        &params,
        today,
        backend.config.aging,
    ) {
        Ok(results) => Ok(envelope::success(&results)),
        Err(error) => Err(map_commitments_error(error)),
    }
}
