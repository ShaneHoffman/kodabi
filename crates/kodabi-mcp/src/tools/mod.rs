//! The `tools/call` router and the five tool handlers (three read, two write).
//!
//! Each handler deserializes its arguments into a kodabi-core param type, calls
//! one core function, and wraps the result in the success/business-error
//! envelope. A handler returns `Err(RpcError)` for a protocol-level fault
//! (malformed arguments, unavailable backend) and `Ok(business_error(..))` for a
//! business fault the model should reason about (a missing note id, a missing
//! target project, an `on_conflict: "error"` glossary hit).

mod add_glossary_term;
mod file_note_to_project;
mod get_note;
mod list_projects;
mod search_notes;

use std::path::Path;

use serde_json::Value;

use kodabi_core::index::{IndexError, NoteRow, NoteType};
use kodabi_core::note::NoteError;
use kodabi_core::vault::{AddGlossaryTermError, ListedNote};

use crate::envelope;
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
        "file_note_to_project" => file_note_to_project::call(server, arguments),
        "add_glossary_term" => add_glossary_term::call(server, arguments),
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

/// Routes a `file_note_to_project` [`NoteError`] to the right channel. Unlike
/// [`map_note_error`], a missing target project or a vault-wide duplicate id are
/// business faults (`isError`) the model should reason about, not internal
/// errors: a malformed field (Inbox target, over-long reason, bad slug, or a
/// confidence out of range) stays `invalid_params`; I/O and the routing-examples
/// log stay internal.
fn map_file_note_error(error: NoteError) -> Result<Value, RpcError> {
    match error {
        NoteError::InvalidField { .. } => Err(RpcError::invalid_params(error.to_string())),
        NoteError::MissingProject { .. } | NoteError::DuplicateNoteId { .. } => {
            Ok(envelope::business_error(error.to_string()))
        }
        other => Err(RpcError::internal(other.to_string())),
    }
}

/// Routes an [`AddGlossaryTermError`] to the right channel: an invalid slug or
/// field is `invalid_params`; a missing project or an `on_conflict: "error"` hit
/// are business faults (`isError`); a storage failure is internal.
fn map_add_glossary_term_error(error: AddGlossaryTermError) -> Result<Value, RpcError> {
    match error {
        AddGlossaryTermError::InvalidProject(_) | AddGlossaryTermError::InvalidInput { .. } => {
            Err(RpcError::invalid_params(error.to_string()))
        }
        AddGlossaryTermError::MissingProject { .. } | AddGlossaryTermError::Conflict { .. } => {
            Ok(envelope::business_error(error.to_string()))
        }
        AddGlossaryTermError::Storage(_) => Err(RpcError::internal(error.to_string())),
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

impl NoteSummaryDto {
    /// Builds the summary from a vault-side note (e.g. the result of a re-route),
    /// mirroring `src-tauri`'s `written_note`/`note_summary`: the Inbox sentinel
    /// becomes `project: null`, `path` is KB-relative with forward slashes, and
    /// routing supplies project + confidence. Distinct from [`From<&NoteRow>`]
    /// because a fresh vault write has no index row yet.
    fn from_listed_note(listed: &ListedNote, kb_root: &Path) -> Self {
        let note = &listed.note;
        let project = note.routing.project();
        let relative = listed.path.strip_prefix(kb_root).unwrap_or(&listed.path);
        Self {
            id: note.id.as_str().to_string(),
            path: relative.to_string_lossy().replace('\\', "/"),
            title: listed.title.clone(),
            note_type: map_note_type(note.note_type),
            project: (project != kodabi_core::note::INBOX).then(|| project.to_string()),
            date: note.date.clone(),
            tags: note
                .tags
                .iter()
                .map(|tag| tag.as_str().to_string())
                .collect(),
            source: note.source.as_yaml().to_string(),
            confidence: note.routing.confidence(),
        }
    }
}

/// Bridges the vault-side note type to the index-side [`NoteType`] the wire DTO
/// serializes. Both are the same closed `meeting | note | chat` set.
fn map_note_type(note_type: kodabi_core::note::NoteType) -> NoteType {
    match note_type {
        kodabi_core::note::NoteType::Meeting => NoteType::Meeting,
        kodabi_core::note::NoteType::Note => NoteType::Note,
        kodabi_core::note::NoteType::Chat => NoteType::Chat,
    }
}
