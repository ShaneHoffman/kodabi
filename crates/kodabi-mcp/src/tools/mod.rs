//! The `tools/call` router and the eight tool handlers (six read, two write).
//!
//! Each handler deserializes its arguments into a kodabi-core param type, calls
//! one core function, and wraps the result in the success/business-error
//! envelope. A handler returns `Err(RpcError)` for a protocol-level fault
//! (malformed arguments, unavailable backend) and `Ok(business_error(..))` for a
//! business fault the model should reason about (a missing note id, a missing
//! target project, an `on_conflict: "error"` glossary hit).

mod add_glossary_term;
mod file_note_to_project;
mod get_meeting_transcript;
mod get_note;
mod get_project_context;
mod list_outstanding_items;
mod list_projects;
mod search_notes;

use std::path::Path;

use chrono::NaiveDate;
use serde_json::Value;

use kodabi_core::index::{ActionItemRow, ActionItemStatus, IndexError, NoteRow, NoteType};
use kodabi_core::note::NoteError;
use kodabi_core::project_context::ProjectContextError;
use kodabi_core::sessions::SessionsError;
use kodabi_core::vault::{GlossaryOpError, ListedNote};

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
        "get_meeting_transcript" => get_meeting_transcript::call(server, arguments),
        "list_outstanding_items" => list_outstanding_items::call(server, arguments),
        "list_projects" => list_projects::call(server, arguments),
        "get_project_context" => get_project_context::call(server, arguments),
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

/// Maps a [`SessionsError`] to the right JSON-RPC channel. A malformed cursor
/// is the caller's fault (`invalid_params`); everything reaching here otherwise
/// is a storage or parse fault (`internal`). `InvalidSource` never reaches this
/// mapper — `sessions::read_transcript_page` already folds a keyword `source`
/// into `transcript_available: false`, which is the documented success shape,
/// not an error.
fn map_sessions_error(error: SessionsError) -> RpcError {
    match error {
        SessionsError::Cursor(_) => RpcError::invalid_params(error.to_string()),
        other => RpcError::internal(other.to_string()),
    }
}

/// Routes a [`ProjectContextError`]: a slug naming no project is a business
/// fault (`isError`) the model should reason about; a malformed slug is invalid
/// params; index, glossary, and I/O faults are internal.
fn map_project_context_error(error: ProjectContextError) -> Result<Value, RpcError> {
    match error {
        ProjectContextError::NotFound { .. } => Ok(envelope::business_error(error.to_string())),
        ProjectContextError::InvalidProject(_) => Err(RpcError::invalid_params(error.to_string())),
        ProjectContextError::Index(inner) => Err(map_index_error(inner)),
        other => Err(RpcError::internal(other.to_string())),
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

/// Routes a [`GlossaryOpError`] to the right channel: an invalid slug or
/// field is `invalid_params`; a missing project or an `on_conflict: "error"` hit
/// are business faults (`isError`); a storage failure is internal.
fn map_add_glossary_term_error(error: GlossaryOpError) -> Result<Value, RpcError> {
    match error {
        GlossaryOpError::InvalidProject(_) | GlossaryOpError::InvalidInput { .. } => {
            Err(RpcError::invalid_params(error.to_string()))
        }
        GlossaryOpError::MissingProject { .. } | GlossaryOpError::Conflict { .. } => {
            Ok(envelope::business_error(error.to_string()))
        }
        // `NotFound` belongs to the update/remove operations, which this
        // add-only tool surface never calls: unreachable here, but the arm
        // keeps the match exhaustive rather than swallowing it into a catch-all.
        GlossaryOpError::NotFound { .. } | GlossaryOpError::Storage(_) => {
            Err(RpcError::internal(error.to_string()))
        }
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
/// [`ActionItemStatus::derive`]); `extracted_date` is omitted when absent,
/// matching its optional status in the schema.
#[derive(serde::Serialize)]
struct ActionItemDto {
    id: String,
    description: String,
    owner: String,
    due_date: Option<String>,
    status: ActionItemStatus,
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
            status: ActionItemStatus::derive(item.done, item.due_date.as_deref(), today),
            source: NoteRefDto {
                id: note.id.clone(),
                path: note.path.clone(),
            },
            extracted_date: item.extracted_date.clone(),
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
