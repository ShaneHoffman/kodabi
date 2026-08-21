//! `get_note`: a note's full metadata and body by stable id, plus its extracted
//! `action_items` and, for a meeting, the structured `meeting` metadata.

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
    /// present for a meeting note. Note the asymmetry this leaves: a chat note
    /// has no `meeting` object, so suppressing the list also suppresses any
    /// count of it.
    #[serde(default = "default_true")]
    include_action_items: bool,
}

/// Fetches a note by id. A missing id is a business error (`isError`), since the
/// caller asserted the note exists. `action_items` carries the extracted items of
/// any note that has them, which is every type. For a meeting note, `meeting`
/// additionally carries the index-backed `MeetingMeta` (duration, speaker count,
/// decisions, action-item count); it is `null` for every other type.
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

    // Action items are not meeting-only: a chat note's commitments are indexed
    // too (`kodabi_core::meeting::derives_facts`), so this read is type-agnostic
    // and a type carrying no facts simply has no rows.
    let items = backend
        .index
        .get_action_items(&params.id)
        .map_err(map_index_error)?;
    let today = Local::now().date_naive();

    // `action_item_count` below already reflects the full set; the list itself is
    // only emitted when the caller asked for it.
    let action_items: Vec<ActionItemDto> = if params.include_action_items {
        items
            .iter()
            .map(|item| ActionItemDto::from_row(item, &row, today))
            .collect()
    } else {
        Vec::new()
    };

    // `meeting`, by contrast, stays meeting-only. `MeetingMeta` leads with
    // `duration_seconds` + `speaker_count`, which are structurally always null
    // for a chat (it has no session recording to measure), and the wire field is
    // literally named `meeting` — so a chat gets `null` here rather than a shape
    // whose two lead fields can never be populated.
    let meeting = if row.note_type == NoteType::Meeting {
        let facts = backend
            .index
            .get_meeting_facts(&params.id)
            .map_err(map_index_error)?;
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
        serde_json::to_value(&meeting).map_err(|error| {
            RpcError::internal(format!("failed to serialize meeting metadata: {error}"))
        })?
    } else {
        Value::Null
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
