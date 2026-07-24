//! `search_notes`: hybrid FTS + semantic retrieval over the note index.

use serde_json::Value;

use kodabi_core::index::SearchParams;

use super::map_index_error;
use crate::envelope;
use crate::protocol::RpcError;
use crate::server::Server;

/// Runs a hybrid search. The embedder is `None`, so this is FTS-only — the
/// default app build ships without an embedder, and the semantic arm degrades to
/// empty regardless. Empty matches are a successful empty page, not an error.
pub fn call(server: &Server, arguments: Value) -> Result<Value, RpcError> {
    let backend = server.backend()?;
    let params: SearchParams = serde_json::from_value(arguments).map_err(|error| {
        RpcError::invalid_params(format!("invalid search_notes arguments: {error}"))
    })?;

    match backend.index.search_notes(&params, None) {
        Ok(results) => Ok(envelope::success(&results)),
        Err(error) => Err(map_index_error(error)),
    }
}
