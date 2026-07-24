//! `get_note`: a note's full metadata and body by stable id.

use serde_json::{json, Value};

use super::{map_index_error, NoteSummaryDto};
use crate::envelope;
use crate::protocol::RpcError;
use crate::server::Server;

fn default_true() -> bool {
    true
}

/// `get_note` arguments.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GetNoteParams {
    id: String,
    #[serde(default = "default_true")]
    include_body: bool,
    // Accepted for schema conformance but not yet effective: action items are
    // not index-backed, so the returned list is always empty regardless (a
    // follow-up wires real meeting data). Kept so `deny_unknown_fields` still
    // accepts the argument rather than rejecting the call.
    #[serde(default = "default_true")]
    #[allow(dead_code)]
    include_action_items: bool,
}

/// Fetches a note by id. A missing id is a business error (`isError`), since the
/// caller asserted the note exists. The `meeting` and `action_items` fields are
/// stubbed (null / empty) until the index carries meeting metadata; the note's
/// body still carries any action items as markdown.
pub fn call(server: &Server, arguments: Value) -> Result<Value, RpcError> {
    let backend = server.backend()?;
    let params: GetNoteParams = serde_json::from_value(arguments).map_err(|error| {
        RpcError::invalid_params(format!("invalid get_note arguments: {error}"))
    })?;

    match backend.index.get_note(&params.id) {
        Ok(Some(row)) => {
            let body_markdown = params.include_body.then(|| row.body.clone());
            let payload = json!({
                "note": NoteSummaryDto::from(&row),
                "meeting": Value::Null,
                "body_markdown": body_markdown,
                "action_items": [],
            });
            Ok(envelope::success(&payload))
        }
        Ok(None) => Ok(envelope::business_error(format!(
            "note not found: {}",
            params.id
        ))),
        Err(error) => Err(map_index_error(error)),
    }
}
