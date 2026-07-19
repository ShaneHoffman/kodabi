//! Thin Tauri command wrappers over `kodabi_core::note` and
//! `kodabi_core::vault`. The note struct, frontmatter emit/parse, atomic
//! writes, disk enumeration, and all validation live in `kodabi-core`; these
//! commands only own the serde IPC DTOs, mint the note id server-side at
//! creation, resolve the knowledge-base root, and map results to the MCP
//! `NoteSummary` projection. Errors collapse to a message string — the same
//! convention `audio_cmds` uses for IPC results.

use std::path::Path;

use kodabi_core::index::IndexedNote;
use kodabi_core::note::{self, Note, NoteEdit, NoteId, NoteType, Routing, Source, Tag};
use kodabi_core::vault::{self, FileNoteOptions, ListedNote, ProjectInfo, RoutedNote};
use tauri::{AppHandle, Manager};

use crate::index_state::IndexState;
use crate::transcribe::knowledge_base_dir;

/// Maximum length (in `char`s) of a note-list snippet.
const SNIPPET_MAX_CHARS: usize = 160;

/// A one-line snippet of a note body for list rows: leading Markdown *block*
/// markers (`#` headings, `>` quotes, `-`/`*`/`+` bullets) are stripped,
/// thematic breaks and blank lines dropped, remaining lines joined with a space,
/// and the result truncated on a `char` boundary. Pure (no filesystem), so it is
/// unit-tested directly.
fn snippet(body: &str) -> String {
    let joined = body
        .lines()
        .map(strip_block_markers)
        .map(str::trim)
        .filter(|line| !line.is_empty() && !is_thematic_break(line))
        .collect::<Vec<_>>()
        .join(" ");
    joined.chars().take(SNIPPET_MAX_CHARS).collect()
}

/// Strips leading Markdown *block* markers (heading `#`s, a `>` quote, a
/// `-`/`*`/`+` bullet) from one line, peeling nested ones (`> - item`). A marker
/// only counts when whitespace (or end of line) follows it, so inline emphasis
/// like `**bold**` — where the `*` is not followed by a space — is left intact.
fn strip_block_markers(line: &str) -> &str {
    let mut rest = line.trim_start();
    loop {
        let after_marker = match rest.chars().next() {
            Some('#') => rest.trim_start_matches('#'),
            Some('>' | '-' | '*' | '+') => &rest[1..],
            _ => return rest,
        };
        // A genuine block marker is followed by whitespace (or line end);
        // otherwise the char was inline punctuation, so keep the line as-is.
        match after_marker.chars().next() {
            Some(c) if c.is_whitespace() => rest = after_marker.trim_start(),
            None => return "",
            _ => return rest,
        }
    }
}

/// Whether a line is a Markdown thematic break or setext underline (a run of
/// only `-`, `*`, `_`, or `=`, three or more), which carries no preview text.
fn is_thematic_break(line: &str) -> bool {
    let marks = line.chars().filter(|c| !c.is_whitespace());
    let mut count = 0;
    for c in marks {
        if !matches!(c, '-' | '*' | '_' | '=') {
            return false;
        }
        count += 1;
    }
    count >= 3
}

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
/// relative to the KB root with forward slashes. `title` echoes the
/// caller-supplied title that seeded the filename — `null` when none was given,
/// in which case the filename derives from the `id`.
#[derive(serde::Serialize)]
pub struct WrittenNote {
    id: String,
    path: String,
    title: Option<String>,
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
/// re-routes preserve it. `async` (like every command below) so the disk work
/// runs on the async runtime's pool, not the main thread — a sync command
/// would freeze the webview for the duration of the vault I/O.
#[tauri::command]
pub async fn write_note(app: AppHandle, input: NewNoteInput) -> Result<WrittenNote, String> {
    write_note_impl(&app, input)
}

fn write_note_impl(app: &AppHandle, mut input: NewNoteInput) -> Result<WrittenNote, String> {
    let kb = knowledge_base_dir(app)?;
    let title = input.title.clone();

    // Adopt the on-disk casing of any existing project folder, so the
    // frontmatter never disagrees with the folder the file actually lands in.
    input.project =
        vault::canonicalize_project(&kb, &input.project).map_err(|err| err.to_string())?;

    let id = NoteId::generate().map_err(|err| err.to_string())?;
    let note = note_from_input(input, id)?;

    let path = note::write_note(&kb, &note, title.as_deref()).map_err(|err| err.to_string())?;
    let rel = path.strip_prefix(&kb).unwrap_or(&path);

    // Keep the index in step with the disk write. Best-effort: the file is the
    // source of truth, so a failed index never fails the write. The title is
    // derived from the on-disk stem (not the caller's title) so a later edit —
    // which re-derives it the same way — sees no spurious title change.
    let indexed = IndexedNote::from_note(
        &note,
        &vault::display_title(&path),
        &rel.to_string_lossy().replace('\\', "/"),
    );
    app.state::<IndexState>().index_note_best_effort(indexed);

    Ok(written_note(&note, title, rel))
}

/// Builds a validated core [`Note`] from the wire input and a freshly minted
/// `id`. Pure (no filesystem), so it can be unit-tested without an `AppHandle`.
fn note_from_input(input: NewNoteInput, id: NoteId) -> Result<Note, String> {
    let note_type = NoteType::parse(&input.note_type).map_err(|err| err.to_string())?;
    let source = Source::parse(&input.source).map_err(|err| err.to_string())?;
    let tags = parse_tags(&input.tags)?;
    let routing = Routing::from_project_and_confidence(input.project, input.confidence)
        .map_err(|err| err.to_string())?;

    Note::new(id, note_type, routing, input.date, tags, source, input.body)
        .map_err(|err| err.to_string())
}

/// Parses wire tags through the core's lenient input normalization
/// (`Tag::parse_normalized`: trim + case-fold), shared by the create and edit
/// paths so they can't drift.
fn parse_tags(tags: &[String]) -> Result<Vec<Tag>, String> {
    tags.iter()
        .map(|t| Tag::parse_normalized(t))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| err.to_string())
}

/// Projects a written [`Note`] to the `NoteSummary` wire shape: the Inbox
/// sentinel becomes `project: null`, and the KB-relative path is normalized to
/// forward slashes. `title` is the caller-supplied title (if any) that seeded
/// the filename.
fn written_note(note: &Note, title: Option<String>, rel_path: &Path) -> WrittenNote {
    let project = note.routing.project();
    WrittenNote {
        id: note.id.as_str().to_string(),
        path: rel_path.to_string_lossy().replace('\\', "/"),
        title,
        note_type: note.note_type.as_str().to_string(),
        project: (project != note::INBOX).then(|| project.to_string()),
        date: note.date.clone(),
        tags: note.tags.iter().map(|t| t.as_str().to_string()).collect(),
        source: note.source.as_yaml().to_string(),
        confidence: note.routing.confidence(),
    }
}

/// A note listed from or read off the disk, in the MCP `NoteSummary`
/// projection. Unlike [`WrittenNote`] (which echoes the caller-supplied title
/// that seeded the filename), `title` here is derived from the filename stem —
/// the only faithful source, since the title is not frontmatter.
///
/// `snippet` is a UI-only extension beyond the doc'd `NoteSummary` (which is
/// deliberately schema-open); it gives list rows a body preview without a
/// second `read_note` round-trip. It is derived, never stored.
#[derive(serde::Serialize)]
pub struct NoteSummaryDto {
    id: String,
    path: String,
    title: String,
    #[serde(rename = "type")]
    note_type: String,
    project: Option<String>,
    date: String,
    tags: Vec<String>,
    source: String,
    confidence: Option<f64>,
    snippet: String,
}

/// One opened note: the MCP `get_note` vocabulary (`NoteSummary` +
/// `body_markdown`), flattened for IPC ergonomics.
#[derive(serde::Serialize)]
pub struct NoteDetailDto {
    #[serde(flatten)]
    summary: NoteSummaryDto,
    body_markdown: String,
}

/// An edit to an existing note, as sent from the frontend. Deliberately
/// narrow: `id` and `project` only locate the file — the id is preserved
/// verbatim, and moving a note between projects is the re-route flow, not an
/// edit. `source` and `confidence` are absent by design; they are read from
/// the existing file and preserved untouched.
#[derive(serde::Deserialize)]
pub struct SaveNoteInput {
    id: String,
    project: String,
    #[serde(rename = "type")]
    note_type: String,
    date: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    body: String,
}

/// A project discovered on disk, mirroring the MCP `Project` shape. `id` is
/// the slug — with no index there is no separate opaque identity yet.
#[derive(serde::Serialize)]
pub struct ProjectDto {
    id: String,
    slug: String,
    display_name: String,
    parent: Option<String>,
    note_count: u32,
    meeting_count: u32,
    last_activity: Option<String>,
}

/// The sidebar's whole world in one response: the pinned Inbox badge count
/// plus every real project on disk.
#[derive(serde::Serialize)]
pub struct ProjectListDto {
    inbox_note_count: u32,
    projects: Vec<ProjectDto>,
}

/// Lists a project's direct notes, newest first. Unparseable `.md` files are
/// skipped (and logged) rather than failing the whole listing.
#[tauri::command]
pub async fn list_notes(app: AppHandle, project: String) -> Result<Vec<NoteSummaryDto>, String> {
    let kb = knowledge_base_dir(&app)?;
    let scan = vault::scan_project_notes(&kb, &project).map_err(|err| err.to_string())?;
    if !scan.skipped.is_empty() {
        eprintln!(
            "list_notes: skipped {} unparseable file(s) in {project}",
            scan.skipped.len()
        );
    }
    Ok(scan
        .notes
        .iter()
        .map(|listed| note_summary(listed, &kb))
        .collect())
}

/// Opens one note by its stable id within a project.
#[tauri::command]
pub async fn read_note(
    app: AppHandle,
    project: String,
    id: String,
) -> Result<NoteDetailDto, String> {
    let kb = knowledge_base_dir(&app)?;
    let id = parse_note_id(&id)?;
    let listed = vault::find_note(&kb, &project, &id)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("note {} not found in {project}", id.as_str()))?;
    Ok(note_detail(&listed, &kb))
}

/// Saves an edit to an existing note in place: the file is re-located by
/// `(project, id)`, only the editable fields (body, tags, date, type) are
/// replaced, and the filename never changes. The locate-merge-rewrite sequence
/// (and its id/source/routing preservation contract) lives in
/// `vault::save_note_edit`; this command only parses the wire strings.
#[tauri::command]
pub async fn save_note(app: AppHandle, input: SaveNoteInput) -> Result<NoteDetailDto, String> {
    let kb = knowledge_base_dir(&app)?;
    let id = parse_note_id(&input.id)?;
    let edit = note_edit_from_input(&input)?;
    let listed = vault::save_note_edit(&kb, &input.project, &id, edit)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("note {} not found in {}", input.id, input.project))?;

    // Re-index the edited note (best-effort; see `write_note_impl`). Editing the
    // body drops its stale vectors in `upsert_note`, so this re-embeds them.
    let rel = listed.path.strip_prefix(&kb).unwrap_or(&listed.path);
    let indexed = IndexedNote::from_note(
        &listed.note,
        &listed.title,
        &rel.to_string_lossy().replace('\\', "/"),
    );
    app.state::<IndexState>().index_note_best_effort(indexed);

    Ok(note_detail(&listed, &kb))
}

/// A re-route request, mirroring the MCP `file_note_to_project` input. `id` and
/// `project` are required; the rest default to the contract defaults (no
/// creation, confidence recorded as `1.0`, no reason). The Inbox UI's one-click
/// correction sends only `{ id, project }`.
#[derive(serde::Deserialize)]
pub struct FileNoteInput {
    id: String,
    project: String,
    #[serde(default)]
    create_project: bool,
    #[serde(default)]
    confidence: Option<f64>,
    #[serde(default)]
    reason: Option<String>,
}

/// The note's location before a re-route, mirroring the MCP output `previous`.
#[derive(serde::Serialize)]
pub struct PreviousLocationDto {
    path: String,
    project: Option<String>,
}

/// The re-route outcome, mirroring the MCP `file_note_to_project` output:
/// the note after routing, where it came from, and whether the file moved.
#[derive(serde::Serialize)]
pub struct FileNoteOutcomeDto {
    note: NoteSummaryDto,
    previous: PreviousLocationDto,
    moved: bool,
}

/// Routes or re-routes a note to a project — the human correction loop. Moves
/// the file, updates its frontmatter `project` + `confidence`, preserves the
/// stable `id`, and logs the correction as a routing example. The whole
/// mutation (and its contract) lives in `vault::file_note_to_project`, shared
/// with the future MCP tool of the same name; this command only parses the wire
/// strings and projects the result to the `NoteSummary` shape.
#[tauri::command]
pub async fn file_note_to_project(
    app: AppHandle,
    input: FileNoteInput,
) -> Result<FileNoteOutcomeDto, String> {
    let kb = knowledge_base_dir(&app)?;
    let id = parse_note_id(&input.id)?;
    let options = FileNoteOptions {
        create_project: input.create_project,
        confidence: input.confidence,
        reason: input.reason,
    };
    let routed = vault::file_note_to_project(&kb, &id, &input.project, &options)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("note {} not found in the vault", input.id))?;
    Ok(file_note_outcome(routed, &kb))
}

/// Projects a [`RoutedNote`] to the wire outcome: the routed note as a
/// `NoteSummary`, and the previous location with its KB-relative forward-slash
/// path and Inbox → `null` project mapping (same convention as [`note_summary`]).
fn file_note_outcome(routed: RoutedNote, kb: &Path) -> FileNoteOutcomeDto {
    let previous_rel = routed
        .previous_path
        .strip_prefix(kb)
        .unwrap_or(&routed.previous_path);
    FileNoteOutcomeDto {
        note: note_summary(&routed.note, kb),
        previous: PreviousLocationDto {
            path: previous_rel.to_string_lossy().replace('\\', "/"),
            project: routed.previous_project,
        },
        moved: routed.moved,
    }
}

/// Lists every project on disk plus the Inbox note count, for the sidebar.
#[tauri::command]
pub async fn list_projects(app: AppHandle) -> Result<ProjectListDto, String> {
    let kb = knowledge_base_dir(&app)?;
    let projects = vault::list_projects(&kb).map_err(|err| err.to_string())?;
    let inbox = vault::scan_project_notes(&kb, note::INBOX).map_err(|err| err.to_string())?;
    Ok(ProjectListDto {
        inbox_note_count: inbox.notes.len() as u32,
        projects: projects.into_iter().map(project_dto).collect(),
    })
}

fn parse_note_id(id: &str) -> Result<NoteId, String> {
    NoteId::parse(id).map_err(|_| format!("id {id:?} must match ^n_[0-9a-z]{{6,}}$"))
}

/// Parses the wire edit into the core's [`NoteEdit`] (the merge itself —
/// preserving `id`, `source`, routing — is `Note::with_edits` in kodabi-core).
fn note_edit_from_input(input: &SaveNoteInput) -> Result<NoteEdit, String> {
    Ok(NoteEdit {
        note_type: NoteType::parse(&input.note_type).map_err(|err| err.to_string())?,
        date: input.date.clone(),
        tags: parse_tags(&input.tags)?,
        body: input.body.clone(),
    })
}

/// Projects a disk-listed note to the `NoteSummary` wire shape (same mapping
/// as [`written_note`]: Inbox → `null`, KB-relative forward-slash path).
fn note_summary(listed: &ListedNote, kb: &Path) -> NoteSummaryDto {
    let rel = listed.path.strip_prefix(kb).unwrap_or(&listed.path);
    let project = listed.note.routing.project();
    NoteSummaryDto {
        id: listed.note.id.as_str().to_string(),
        path: rel.to_string_lossy().replace('\\', "/"),
        title: listed.title.clone(),
        note_type: listed.note.note_type.as_str().to_string(),
        project: (project != note::INBOX).then(|| project.to_string()),
        date: listed.note.date.clone(),
        tags: listed
            .note
            .tags
            .iter()
            .map(|t| t.as_str().to_string())
            .collect(),
        source: listed.note.source.as_yaml().to_string(),
        confidence: listed.note.routing.confidence(),
        snippet: snippet(&listed.note.body),
    }
}

fn note_detail(listed: &ListedNote, kb: &Path) -> NoteDetailDto {
    NoteDetailDto {
        body_markdown: listed.note.body.clone(),
        summary: note_summary(listed, kb),
    }
}

/// Splits a slug into the sidebar's `display_name` (last segment) and `parent`
/// (everything before it).
fn project_dto(info: ProjectInfo) -> ProjectDto {
    let (parent, display_name) = match info.slug.rsplit_once('/') {
        Some((parent, name)) => (Some(parent.to_string()), name.to_string()),
        None => (None, info.slug.clone()),
    };
    ProjectDto {
        id: info.slug.clone(),
        slug: info.slug,
        display_name,
        parent,
        note_count: info.note_count,
        meeting_count: info.meeting_count,
        last_activity: info.last_activity,
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
            r#"{"type":"meeting","project":"Briarwood Golf","confidence":0.94,"date":"2026-07-10","tags":["budgeting"],"source":"manual","title":"Weekly Sync"}"#,
        )
        .unwrap();
        let note = note_from_input(input, NoteId::parse("n_a1b2c3").unwrap()).unwrap();
        assert_eq!(
            note.routing,
            Routing::Routed {
                project: "Briarwood Golf".to_string(),
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
        let dto = written_note(&note, Some("Idea".to_string()), Path::new("Inbox\\idea.md"));
        assert_eq!(dto.project, None);
        assert_eq!(dto.confidence, Some(0.38));
        assert_eq!(dto.path, "Inbox/idea.md");
        assert_eq!(dto.title.as_deref(), Some("Idea"));
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
        let dto = written_note(&note, None, Path::new("Growth\\Q3\\weekly-sync.md"));
        assert_eq!(dto.project.as_deref(), Some("Growth/Q3"));
        assert_eq!(dto.confidence, None);
        assert_eq!(dto.path, "Growth/Q3/weekly-sync.md");
        assert_eq!(dto.title, None);
    }

    // --- edit path (save_note helpers) -------------------------------------

    fn save_input(json: &str) -> SaveNoteInput {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn save_note_input_deserializes_with_type_rename_and_defaults() {
        let input =
            save_input(r#"{"id":"n_a1b2c3","project":"Ops","type":"note","date":"2026-07-10"}"#);
        assert_eq!(input.note_type, "note");
        assert!(input.tags.is_empty());
        assert_eq!(input.body, "");
    }

    #[test]
    fn note_edit_parses_the_wire_strings_and_folds_tag_case() {
        let input = save_input(
            r#"{"id":"n_a1b2c3","project":"Ops","type":"note",
                "date":"2026-07-12","tags":["Follow-Up"],"body":"Rewritten."}"#,
        );
        let edit = note_edit_from_input(&input).unwrap();
        assert_eq!(edit.note_type, NoteType::Note);
        assert_eq!(edit.tags, vec![Tag::parse("follow-up").unwrap()]);
        assert_eq!(edit.date, "2026-07-12");
        assert_eq!(edit.body, "Rewritten.");
    }

    #[test]
    fn note_edit_rejects_invalid_type_or_tag() {
        // (An invalid date is rejected downstream, by `Note::with_edits`.)
        for json in [
            r#"{"id":"n_a1b2c3","project":"Ops","type":"Meeting","date":"2026-07-10"}"#,
            r#"{"id":"n_a1b2c3","project":"Ops","type":"note","date":"2026-07-10","tags":["bad tag"]}"#,
        ] {
            assert!(note_edit_from_input(&save_input(json)).is_err());
        }
    }

    // --- disk projections ---------------------------------------------------

    #[test]
    fn note_summary_maps_inbox_to_null_and_normalizes_the_path() {
        let kb = Path::new("kb-root");
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
            "Body.",
        )
        .unwrap();
        let listed = ListedNote {
            path: kb.join("Inbox").join("idea.md"),
            title: "idea".to_string(),
            note,
        };
        let dto = note_summary(&listed, kb);
        assert_eq!(dto.project, None);
        assert_eq!(dto.path, "Inbox/idea.md");
        assert_eq!(dto.title, "idea");
        assert_eq!(dto.confidence, Some(0.38));
    }

    #[test]
    fn project_dto_derives_display_name_parent_and_id_from_slug() {
        let nested = project_dto(ProjectInfo {
            slug: "Growth/Q3".to_string(),
            note_count: 8,
            meeting_count: 4,
            last_activity: None,
        });
        assert_eq!(nested.id, "Growth/Q3");
        assert_eq!(nested.display_name, "Q3");
        assert_eq!(nested.parent.as_deref(), Some("Growth"));

        let root = project_dto(ProjectInfo {
            slug: "Ops".to_string(),
            note_count: 1,
            meeting_count: 0,
            last_activity: None,
        });
        assert_eq!(root.display_name, "Ops");
        assert_eq!(root.parent, None);
    }

    // --- snippet ------------------------------------------------------------

    #[test]
    fn snippet_strips_line_markers_drops_blanks_and_joins() {
        let body = "# Heading\n\n> A quote line\n\n- first bullet\n* second bullet";
        assert_eq!(
            snippet(body),
            "Heading A quote line first bullet second bullet"
        );
    }

    #[test]
    fn snippet_keeps_inline_emphasis_and_only_strips_block_markers() {
        // Leading block markers (heading, bullet) go; inline `**`/`*` — a marker
        // char not followed by a space — stays, and nested `> -` is peeled.
        let body = "## Decisions\n**Decision:** ship it\n- do *the* thing\n> - nested";
        assert_eq!(
            snippet(body),
            "Decisions **Decision:** ship it do *the* thing nested"
        );
    }

    #[test]
    fn snippet_drops_thematic_breaks() {
        assert_eq!(snippet("---\nReal text\n* * *"), "Real text");
    }

    #[test]
    fn snippet_is_empty_for_an_empty_body() {
        assert_eq!(snippet(""), "");
        assert_eq!(snippet("\n\n"), "");
    }

    #[test]
    fn snippet_truncates_on_a_char_boundary() {
        let body = "é".repeat(SNIPPET_MAX_CHARS + 40);
        assert_eq!(snippet(&body).chars().count(), SNIPPET_MAX_CHARS);
    }

    // --- file_note_to_project (re-route) -----------------------------------

    #[test]
    fn file_note_input_deserializes_with_required_fields_and_defaults() {
        let input: FileNoteInput =
            serde_json::from_str(r#"{"id":"n_a1b2c3","project":"Ops"}"#).unwrap();
        assert_eq!(input.id, "n_a1b2c3");
        assert_eq!(input.project, "Ops");
        assert!(!input.create_project);
        assert!(input.confidence.is_none());
        assert!(input.reason.is_none());
    }

    #[test]
    fn file_note_outcome_projects_previous_inbox_to_null_and_forward_slashes() {
        let kb = Path::new("kb-root");
        let note = Note::new(
            NoteId::parse("n_a1b2c3").unwrap(),
            NoteType::Note,
            Routing::Routed {
                project: "Ops".to_string(),
                confidence: 1.0,
            },
            "2026-07-10",
            vec![],
            Source::parse("quick-capture").unwrap(),
            "Body.",
        )
        .unwrap();
        let routed = RoutedNote {
            note: ListedNote {
                path: kb.join("Ops").join("idea.md"),
                title: "idea".to_string(),
                note,
            },
            previous_path: kb.join("Inbox").join("idea.md"),
            previous_project: None, // was in the Inbox
            moved: true,
        };

        let dto = file_note_outcome(routed, kb);
        assert_eq!(dto.note.project.as_deref(), Some("Ops"));
        assert_eq!(dto.note.path, "Ops/idea.md");
        assert_eq!(dto.previous.path, "Inbox/idea.md");
        assert_eq!(dto.previous.project, None);
        assert!(dto.moved);
    }
}
