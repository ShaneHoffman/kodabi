//! The `tools/call` router and the three read-tool handlers.
//!
//! Each handler deserializes its arguments into a kodabi-core param type, calls
//! one core function, and wraps the result in the success/business-error
//! envelope. A handler returns `Err(RpcError)` for a protocol-level fault
//! (malformed arguments, unavailable backend) and `Ok(business_error(..))` for a
//! business fault the model should reason about (a missing note id).

mod get_note;
mod list_projects;
mod search_notes;

use serde_json::Value;

use kodabi_core::index::{IndexError, NoteRow, NoteType};
use kodabi_core::note::NoteError;

use crate::protocol::RpcError;
use crate::server::Server;

/// Routes a `tools/call` request to the named tool.
pub fn call(server: &Server, params: Option<&Value>) -> Result<Value, RpcError> {
    let params = params.ok_or_else(|| RpcError::invalid_params("tools/call requires `params`"))?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("tools/call requires a string `name`"))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));

    match name {
        "search_notes" => search_notes::call(server, arguments),
        "get_note" => get_note::call(server, arguments),
        "list_projects" => list_projects::call(server, arguments),
        other => Err(RpcError::invalid_params(format!("unknown tool: {other}"))),
    }
}

/// Maps an [`IndexError`] to the right JSON-RPC channel: inputs a validating
/// client should have rejected (bad cursor/date, oversized filter) are invalid
/// params; storage faults are internal errors.
fn map_index_error(error: IndexError) -> RpcError {
    match error {
        IndexError::Cursor { .. } | IndexError::Date { .. } | IndexError::FilterTooLarge { .. } => {
            RpcError::invalid_params(error.to_string())
        }
        IndexError::Sqlite(_) | IndexError::EmbeddingDim { .. } => {
            RpcError::internal(error.to_string())
        }
    }
}

/// Maps a vault [`NoteError`] to a JSON-RPC channel: a malformed field (the
/// pagination cursor) is invalid params; anything else (I/O) is internal.
fn map_note_error(error: NoteError) -> RpcError {
    match error {
        NoteError::InvalidField { .. } => RpcError::invalid_params(error.to_string()),
        other => RpcError::internal(other.to_string()),
    }
}

/// The `NoteSummary` `$def` shape, serialized field-for-field from a [`NoteRow`].
/// Reused wherever a tool returns note metadata.
#[derive(serde::Serialize)]
struct NoteSummaryDto {
    id: String,
    path: String,
    title: String,
    #[serde(rename = "type")]
    note_type: NoteType,
    project: Option<String>,
    date: String,
    tags: Vec<String>,
    source: String,
    confidence: Option<f64>,
}

impl From<&NoteRow> for NoteSummaryDto {
    fn from(row: &NoteRow) -> Self {
        Self {
            id: row.id.clone(),
            path: row.path.clone(),
            title: row.title.clone(),
            note_type: row.note_type,
            project: row.project.clone(),
            date: row.date.clone(),
            tags: row.tags.clone(),
            source: row.source.clone(),
            confidence: row.confidence,
        }
    }
}
