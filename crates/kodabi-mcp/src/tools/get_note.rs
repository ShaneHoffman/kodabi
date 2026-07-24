//! `get_note`: a note's full metadata and body by stable id, plus a meeting
//! note's structured `meeting` metadata and `action_items`.

use chrono::Local;
use serde_json::{json, Value};

use kodabi_core::index::NoteType;

use super::{map_index_error, ActionItemDto, MeetingMetaDto, NoteSummaryDto};
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
    /// When false, the `action_items` list is omitted (returned empty). It does
    /// not affect `meeting` (including its `action_item_count`), which is always
    /// present for a meeting note.
    #[serde(default = "default_true")]
    include_action_items: bool,
}

/// Fetches a note by id. A missing id is a business error (`isError`), since the
/// caller asserted the note exists. For a meeting note, `meeting` carries the
/// index-backed `MeetingMeta` (duration, speaker count, decisions, action-item
/// count) and `action_items` the extracted items; both are absent (`null` / `[]`)
/// for a non-meeting note.
pub fn call(server: &Server, arguments: Value) -> Result<Value, RpcError> {
    let backend = server.backend()?;
    let params: GetNoteParams = serde_json::from_value(arguments).map_err(|error| {
        RpcError::invalid_params(format!("invalid get_note arguments: {error}"))
    })?;

    let row = match backend.index.get_note(&params.id) {
        Ok(Some(row)) => row,
        Ok(None) => {
            return Ok(envelope::business_error(format!(
                "note not found: {}",
                params.id
            )))
        }
        Err(error) => return Err(map_index_error(error)),
    };

    // Meeting metadata and action items are meeting-only. A non-meeting note
    // gets `null` / `[]` without touching the meeting tables.
    let (meeting, action_items) = if row.note_type == NoteType::Meeting {
        let facts = backend
            .index
            .get_meeting_facts(&params.id)
            .map_err(map_index_error)?;
        let items = backend
            .index
            .get_action_items(&params.id)
            .map_err(map_index_error)?;
        let today = Local::now().date_naive();

        let (duration_seconds, speaker_count, decisions) = match facts {
            Some(facts) => (facts.duration_seconds, facts.speaker_count, facts.decisions),
            // A meeting note not yet backfilled: still a meeting, but with no
            // derived scalars/decisions yet.
            None => (None, None, Vec::new()),
        };
        let meeting = MeetingMetaDto {
            note: NoteSummaryDto::from(&row),
            duration_seconds,
            speaker_count,
            decisions,
            action_item_count: items.len() as u32,
        };
        let meeting = serde_json::to_value(&meeting).map_err(|error| {
            RpcError::internal(format!("failed to serialize meeting metadata: {error}"))
        })?;

        // `action_item_count` above already reflects the full set; the list
        // itself is only emitted when the caller asked for it.
        let action_items: Vec<ActionItemDto> = if params.include_action_items {
            items
                .iter()
                .map(|item| ActionItemDto::from_row(item, &row, today))
                .collect()
        } else {
            Vec::new()
        };
        (meeting, action_items)
    } else {
        (Value::Null, Vec::new())
    };

    let body_markdown = params.include_body.then(|| row.body.clone());
    let payload = json!({
        "note": NoteSummaryDto::from(&row),
        "meeting": meeting,
        "body_markdown": body_markdown,
        "action_items": action_items,
    });
    Ok(envelope::success(&payload))
}
