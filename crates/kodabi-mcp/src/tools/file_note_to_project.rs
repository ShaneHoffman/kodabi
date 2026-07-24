//! `file_note_to_project`: route or re-route a note — the human correction loop.

use serde_json::{json, Value};

use kodabi_core::note::NoteId;
use kodabi_core::vault::{self, FileNoteOptions};

use super::{map_file_note_error, NoteSummaryDto};
use crate::envelope;
use crate::protocol::RpcError;
use crate::server::Server;

/// `file_note_to_project` arguments.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FileNoteParams {
    id: String,
    project: String,
    #[serde(default)]
    create_project: bool,
    #[serde(default)]
    confidence: Option<f64>,
    #[serde(default)]
    reason: Option<String>,
}

/// Routes or re-routes a note through the shared `vault::file_note_to_project`
/// core function (moves the file, rewrites frontmatter `project` + `confidence`
/// preserving the stable id, and records the routing example). A malformed id or
/// field is a protocol error (`invalid_params`); an unknown id, a missing target
/// project, or a vault-wide duplicate id are business errors (`isError`).
///
/// The open desktop app's file watcher observes the write and reconciles its
/// index + emits `vault:changed`; this server process has no `AppHandle` and so
/// touches only the vault files.
pub fn call(server: &Server, arguments: Value) -> Result<Value, RpcError> {
    let backend = server.backend()?;
    let params: FileNoteParams = serde_json::from_value(arguments).map_err(|error| {
        RpcError::invalid_params(format!("invalid file_note_to_project arguments: {error}"))
    })?;

    let id = NoteId::parse(&params.id)
        .map_err(|_| RpcError::invalid_params(format!("invalid note id: {:?}", params.id)))?;
    let options = FileNoteOptions {
        create_project: params.create_project,
        confidence: params.confidence,
        reason: params.reason,
    };

    let kb_root = &backend.config.kb_root;
    match vault::file_note_to_project(kb_root, &id, &params.project, &options) {
        Ok(Some(routed)) => {
            let previous_relative = routed
                .previous_path
                .strip_prefix(kb_root)
                .unwrap_or(&routed.previous_path);
            let payload = json!({
                "note": NoteSummaryDto::from_listed_note(&routed.note, kb_root),
                "previous": {
                    "path": previous_relative.to_string_lossy().replace('\\', "/"),
                    "project": routed.previous_project,
                },
                "moved": routed.moved,
            });
            Ok(envelope::success(&payload))
        }
        Ok(None) => Ok(envelope::business_error(format!(
            "note not found: {}",
            params.id
        ))),
        Err(error) => map_file_note_error(error),
    }
}
