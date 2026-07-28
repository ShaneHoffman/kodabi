//! `get_meeting_transcript`: a meeting note's per-channel transcript segments,
//! paginated, plus its optional `MeetingMeta`.

use serde_json::{json, Value};

use kodabi_core::index::NoteType;
use kodabi_core::sessions;

use super::{map_index_error, map_sessions_error, MeetingMetaDto, NoteRefDto, NoteSummaryDto};
use crate::envelope;
use crate::protocol::RpcError;
use crate::server::Server;

fn default_true() -> bool {
    true
}

fn default_limit() -> u32 {
    200
}

/// `get_meeting_transcript` arguments.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GetMeetingTranscriptParams {
    id: String,
    /// When false, `meeting` is `null`. Does not affect the segments.
    #[serde(default = "default_true")]
    include_metadata: bool,
    #[serde(default = "default_limit")]
    limit: u32,
    #[serde(default)]
    cursor: Option<String>,
}

/// Fetches one page of a meeting note's transcript.
///
/// Both business-error cases the spec names are checked before any disk access:
/// an id that names no note, and an id that names a note which is not a meeting.
/// A meeting with no stored transcript — retention pruned the `.jsonl`, or the
/// note was captured without one — is *not* an error: it returns
/// `transcript_available: false` with empty segments.
pub fn call(server: &Server, arguments: Value) -> Result<Value, RpcError> {
    let backend = server.backend()?;
    let params: GetMeetingTranscriptParams =
        serde_json::from_value(arguments).map_err(|error| {
            RpcError::invalid_params(format!("invalid get_meeting_transcript arguments: {error}"))
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
    if row.note_type != NoteType::Meeting {
        return Ok(envelope::business_error(format!(
            "not a meeting note: {} (type {})",
            params.id, row.note_type
        )));
    }

    let transcript = sessions::read_transcript_page(
        &backend.config.kb_root,
        &row.source,
        params.limit,
        params.cursor.as_deref(),
    )
    .map_err(map_sessions_error)?;

    let meeting = if params.include_metadata {
        let facts = backend
            .index
            .get_meeting_facts(&params.id)
            .map_err(map_index_error)?;
        let action_item_count = backend
            .index
            .get_action_items(&params.id)
            .map_err(map_index_error)?
            .len() as u32;
        // A meeting note not yet backfilled still answers, with no derived
        // scalars or decisions yet — matching `get_note`.
        let (duration_seconds, speaker_count, decisions) = match facts {
            Some(facts) => (facts.duration_seconds, facts.speaker_count, facts.decisions),
            None => (None, None, Vec::new()),
        };
        serde_json::to_value(MeetingMetaDto {
            note: NoteSummaryDto::from(&row),
            duration_seconds,
            speaker_count,
            decisions,
            action_item_count,
        })
        .map_err(|error| {
            RpcError::internal(format!("failed to serialize meeting metadata: {error}"))
        })?
    } else {
        Value::Null
    };

    let payload = json!({
        "note": NoteRefDto { id: row.id.clone(), path: row.path.clone() },
        "meeting": meeting,
        "transcript_available": transcript.transcript_available,
        "segments": transcript.segments,
        "page": transcript.page,
    });
    Ok(envelope::success(&payload))
}
