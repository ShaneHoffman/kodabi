//! Thin Tauri command wrapper over `kodabi_core::note`. The note struct,
//! frontmatter emit/parse, atomic per-project write, and all validation live in
//! `kodabi-core`; this command only owns the serde IPC DTOs, mints the note id
//! server-side at creation, resolves the knowledge-base root, and maps the
//! result to the MCP `NoteSummary` projection. Errors collapse to a message
//! string — the same convention `audio_cmds` uses for IPC results.

use std::path::Path;

use kodabi_core::note::{self, Note, NoteId, NoteType, Routing, Source, Tag};
use tauri::AppHandle;

use crate::transcribe::knowledge_base_dir;

/// A note to create, as sent from the frontend. Fields mirror the flat
/// frontmatter shape (and the MCP `NoteSummary`): `project` plus an optional
/// `confidence` express routing, `source` is a capture keyword or repo-relative
/// path, and `title` seeds the human-readable filename. The `id` is not accepted
/// — it is minted server-side so it is stable from creation and never rewritten.
#[derive(serde::Deserialize)]
pub struct NewNoteInput {
    #[serde(rename = "type")]
    note_type: String,
    project: String,
    #[serde(default)]
    confidence: Option<f64>,
    date: String,
    #[serde(default)]
    tags: Vec<String>,
    source: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    title: Option<String>,
}

/// The written note echoed back to the frontend, in the MCP `NoteSummary`
/// projection: `project: null` and `confidence: null` stand in for the
/// frontmatter's Inbox sentinel and omitted-confidence key, and `path` is
/// relative to the KB root with forward slashes.
#[derive(serde::Serialize)]
pub struct WrittenNote {
    id: String,
    path: String,
    #[serde(rename = "type")]
    note_type: String,
    project: Option<String>,
    date: String,
    tags: Vec<String>,
    source: String,
    confidence: Option<f64>,
}

/// Creates a note file from `input` and returns its `NoteSummary`-shaped
/// metadata. The `id` is generated here (at creation), so later moves and
/// re-routes preserve it.
#[tauri::command]
pub fn write_note(app: AppHandle, input: NewNoteInput) -> Result<WrittenNote, String> {
    write_note_impl(&app, input)
}

fn write_note_impl(app: &AppHandle, input: NewNoteInput) -> Result<WrittenNote, String> {
    let kb = knowledge_base_dir(app)?;
    let title = input.title.clone();

    let id = NoteId::generate().map_err(|err| err.to_string())?;
    let note = note_from_input(input, id)?;

    let path = note::write_note(&kb, &note, title.as_deref()).map_err(|err| err.to_string())?;
    let rel = path.strip_prefix(&kb).unwrap_or(&path);
    Ok(written_note(&note, rel))
}

/// Builds a validated core [`Note`] from the wire input and a freshly minted
/// `id`. Pure (no filesystem), so it can be unit-tested without an `AppHandle`.
fn note_from_input(input: NewNoteInput, id: NoteId) -> Result<Note, String> {
    let note_type = NoteType::parse(&input.note_type).map_err(|err| err.to_string())?;
    let source = Source::parse(&input.source).map_err(|err| err.to_string())?;
    let tags = input
        .tags
        .iter()
        .map(|t| Tag::parse(t))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| err.to_string())?;
    let routing = Routing::from_project_and_confidence(input.project, input.confidence)
        .map_err(|err| err.to_string())?;

    Note::new(id, note_type, routing, input.date, tags, source, input.body)
        .map_err(|err| err.to_string())
}

/// Projects a written [`Note`] to the `NoteSummary` wire shape: the Inbox
/// sentinel becomes `project: null`, and the KB-relative path is normalized to
/// forward slashes.
fn written_note(note: &Note, rel_path: &Path) -> WrittenNote {
    let project = note.routing.project();
    WrittenNote {
        id: note.id.as_str().to_string(),
        path: rel_path.to_string_lossy().replace('\\', "/"),
        note_type: note.note_type.as_str().to_string(),
        project: (project != note::INBOX).then(|| project.to_string()),
        date: note.date.clone(),
        tags: note.tags.iter().map(|t| t.as_str().to_string()).collect(),
        source: note.source.as_yaml().to_string(),
        confidence: note.routing.confidence(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_the_wire_shape_with_type_rename_and_defaults() {
        let input: NewNoteInput = serde_json::from_str(
            r#"{"type":"note","project":"Ops","date":"2026-07-10","source":"manual"}"#,
        )
        .unwrap();
        assert_eq!(input.note_type, "note");
        assert!(input.tags.is_empty());
        assert!(input.confidence.is_none());
        assert!(input.title.is_none());
        assert_eq!(input.body, "");
    }

    #[test]
    fn present_confidence_builds_a_routed_note() {
        let input: NewNoteInput = serde_json::from_str(
            r#"{"type":"meeting","project":"Paradise Golf","confidence":0.94,"date":"2026-07-10","tags":["budgeting"],"source":"manual","title":"Weekly Sync"}"#,
        )
        .unwrap();
        let note = note_from_input(input, NoteId::parse("n_a1b2c3").unwrap()).unwrap();
        assert_eq!(
            note.routing,
            Routing::Routed {
                project: "Paradise Golf".to_string(),
                confidence: 0.94,
            }
        );
    }

    #[test]
    fn absent_confidence_builds_a_manual_note() {
        let input: NewNoteInput = serde_json::from_str(
            r#"{"type":"note","project":"Ops","date":"2026-07-10","source":"manual"}"#,
        )
        .unwrap();
        let note = note_from_input(input, NoteId::parse("n_a1b2c3").unwrap()).unwrap();
        assert_eq!(
            note.routing,
            Routing::Manual {
                project: "Ops".to_string()
            }
        );
    }

    #[test]
    fn inbox_without_confidence_is_rejected() {
        let input: NewNoteInput = serde_json::from_str(
            r#"{"type":"note","project":"Inbox","date":"2026-07-10","source":"quick-capture"}"#,
        )
        .unwrap();
        assert!(note_from_input(input, NoteId::parse("n_a1b2c3").unwrap()).is_err());
    }

    #[test]
    fn written_note_maps_inbox_to_null_and_normalizes_the_path() {
        let note = Note::new(
            NoteId::parse("n_a1b2c3").unwrap(),
            NoteType::Note,
            Routing::Routed {
                project: note::INBOX.to_string(),
                confidence: 0.38,
            },
            "2026-07-10",
            vec![],
            Source::parse("quick-capture").unwrap(),
            "",
        )
        .unwrap();
        let dto = written_note(&note, Path::new("Inbox\\idea.md"));
        assert_eq!(dto.project, None);
        assert_eq!(dto.confidence, Some(0.38));
        assert_eq!(dto.path, "Inbox/idea.md");
    }

    #[test]
    fn written_note_keeps_a_real_project_and_hierarchy() {
        let note = Note::new(
            NoteId::parse("n_a1b2c3").unwrap(),
            NoteType::Note,
            Routing::Manual {
                project: "Growth/Q3".to_string(),
            },
            "2026-07-10",
            vec![],
            Source::parse("manual").unwrap(),
            "",
        )
        .unwrap();
        let dto = written_note(&note, Path::new("Growth\\Q3\\weekly-sync.md"));
        assert_eq!(dto.project.as_deref(), Some("Growth/Q3"));
        assert_eq!(dto.confidence, None);
        assert_eq!(dto.path, "Growth/Q3/weekly-sync.md");
    }
}
