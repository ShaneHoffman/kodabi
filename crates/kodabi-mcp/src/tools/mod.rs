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

use chrono::NaiveDate;
use serde_json::Value;

use kodabi_core::index::{ActionItemRow, IndexError, NoteRow, NoteType};
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

/// The `MeetingMeta` `$def` — a `NoteSummary` (flattened) plus the meeting-only
/// fields. `duration_seconds`/`speaker_count` are `null` when the transcript is
/// unavailable; `action_item_count` counts the note's action items regardless of
/// whether the list itself is included in the response.
#[derive(serde::Serialize)]
struct MeetingMetaDto {
    #[serde(flatten)]
    note: NoteSummaryDto,
    duration_seconds: Option<u32>,
    speaker_count: Option<u32>,
    decisions: Vec<String>,
    action_item_count: u32,
}

/// The `NoteRef` `$def`: a back-reference to the note an action item was
/// extracted from.
#[derive(serde::Serialize)]
struct NoteRefDto {
    id: String,
    path: String,
}

/// The `ActionItem` `$def`. `status` is derived server-side (see
/// [`effective_status`]); `extracted_date` is omitted when absent, matching its
/// optional status in the schema.
#[derive(serde::Serialize)]
struct ActionItemDto {
    id: String,
    description: String,
    owner: String,
    due_date: Option<String>,
    status: &'static str,
    source: NoteRefDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    extracted_date: Option<String>,
}

impl ActionItemDto {
    /// Projects a stored [`ActionItemRow`] to the wire shape, resolving `source`
    /// from the owning note and `status` against `today`.
    fn from_row(item: &ActionItemRow, note: &NoteRow, today: NaiveDate) -> Self {
        Self {
            id: item.id.clone(),
            description: item.description.clone(),
            owner: item.owner.clone(),
            due_date: item.due_date.clone(),
            status: effective_status(item.done, item.due_date.as_deref(), today),
            source: NoteRefDto {
                id: note.id.clone(),
                path: note.path.clone(),
            },
            extracted_date: item.extracted_date.clone(),
        }
    }
}

/// The wire `status` for an action item: a checked item is `done`; otherwise it
/// is `overdue` once its due date is strictly before `today`, else `open`.
/// Derived here rather than stored so it stays correct as the clock advances (an
/// item with no due date is never overdue).
fn effective_status(done: bool, due_date: Option<&str>, today: NaiveDate) -> &'static str {
    if done {
        return "done";
    }
    match due_date.and_then(|due| NaiveDate::parse_from_str(due, "%Y-%m-%d").ok()) {
        Some(due) if due < today => "overdue",
        _ => "open",
    }
}
