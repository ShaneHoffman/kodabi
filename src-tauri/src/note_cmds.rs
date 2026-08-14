//! Thin Tauri command wrappers over `kodabi_core::note` and
//! `kodabi_core::vault`. The note struct, frontmatter emit/parse, atomic
//! writes, disk enumeration, and all validation live in `kodabi-core`; these
//! commands only own the serde IPC DTOs, mint the note id server-side at
//! creation, resolve the knowledge-base root, and map results to the MCP
//! `NoteSummary` projection. Errors collapse to a message string — the same
//! convention `audio_cmds` uses for IPC results.

use std::path::Path;

use kodabi_core::index::IndexedNote;
use kodabi_core::meeting;
use kodabi_core::note::{self, Note, NoteEdit, NoteId, NoteType, Routing, Source, Tag};
use kodabi_core::sessions;
use kodabi_core::vault::{self, FileNoteOptions, ListedNote, ProjectInfo, RoutedNote};
use tauri::{AppHandle, Emitter, Manager};

use crate::events::VAULT_CHANGED_EVENT;
use crate::index_state::IndexState;
use crate::transcribe::knowledge_base_dir;

/// Broadcasts `vault:changed` so every open window refetches its disk-backed
/// lists immediately after an in-app write, without waiting on the file
/// watcher's debounce — and without depending on the watcher having started at
/// all (it is best-effort; see `index_state`). The watcher's own post-reconcile
/// broadcast, when it lands, is then a harmless, later no-op refetch.
fn broadcast_vault_changed(app: &AppHandle) {
    let _ = app.emit(VAULT_CHANGED_EVENT, ());
}

/// Lands one moved or re-filed note's row on the background index worker.
///
/// Every command that relocates a note does this eagerly rather than leaving it
/// to the file watcher, so the change reflects in search at once and still
/// converges if the watcher never started. The upsert is keyed by the note's
/// stable `id`, so a note that only moved updates its existing row instead of
/// being deleted and reinserted — which is what keeps its embeddings alive.
/// Best-effort by construction: the `.md` file is already the source of truth,
/// so a failed index write may never fail the command (see `index_state`).
fn index_moved_note(index: &IndexState, listed: &ListedNote, kb: &Path) {
    let rel = listed.path.strip_prefix(kb).unwrap_or(&listed.path);
    let mut indexed = IndexedNote::from_note(
        &listed.note,
        &listed.title,
        &rel.to_string_lossy().replace('\\', "/"),
    );
    indexed.meeting = meeting::meeting_facts_for(&listed.note, kb);
    index.index_note_best_effort(indexed);
}

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
    // source of truth, so a failed index never fails the write. The title comes
    // from `effective_title` (the stored frontmatter title, else the de-slugged
    // stem) — the same source a later read/edit uses, so no spurious change.
    let mut indexed = IndexedNote::from_note(
        &note,
        &vault::effective_title(&note, &path),
        &rel.to_string_lossy().replace('\\', "/"),
    );
    // Derive structured facts (`None` for a type that carries none, per
    // `meeting::derives_facts`) so the index can serve `get_note`'s
    // `meeting`/`action_items` without re-reading.
    indexed.meeting = meeting::meeting_facts_for(&note, &kb);
    app.state::<IndexState>().index_note_best_effort(indexed);
    broadcast_vault_changed(app);

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

    // The caller's title is stored in frontmatter (normalized by `with_title`),
    // so it survives past the 40-char filename slug it also seeds.
    let title = input.title;
    Note::new(id, note_type, routing, input.date, tags, source, input.body)
        .map(|note| note.with_title(title))
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
/// projection. `title` is the note's effective title (`vault::effective_title`):
/// its stored frontmatter `title` when present, else the de-slugged filename
/// stem for a legacy or hand-made note.
///
/// `snippet` and `guess` are UI-only extensions beyond the doc'd `NoteSummary`
/// (which is deliberately schema-open); both are derived at list time and never
/// stored. `snippet` gives list rows a body preview without a second
/// `read_note` round-trip. `guess` is the router's current best-guess
/// destination, and is `None` outside Inbox listings — a note that already has
/// a home needs no suggestion of one.
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
    guess: Option<RouteGuessDto>,
}

/// Where the router would file an unfiled note today, mirroring
/// [`routing::RouteGuess`]. `confidence` is the live margin score, distinct
/// from the sibling `confidence` field on the summary: that one is the
/// historical record of why the note landed in the Inbox, this one is the
/// strength of the suggestion being offered now.
#[derive(serde::Serialize)]
pub struct RouteGuessDto {
    project: String,
    confidence: f64,
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
    /// The new display title, or `null`/absent to preserve the stored one.
    /// Callers send `null` when the user did not touch the title: the title
    /// they were shown may be the *effective* one (a de-slugged filename for a
    /// note with no `title` key), and echoing that back would materialize it
    /// into frontmatter. A blank string clears the key, restoring that
    /// fallback. Normalization is `Note::with_title`'s, not this layer's.
    #[serde(default)]
    title: Option<String>,
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
///
/// Inbox listings additionally carry each row's routing guess, so the Inbox can
/// offer a suggested destination per note. Case-insensitive on the sentinel to
/// match `validate_project`'s reservation of the name.
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
    let guesses = if project.eq_ignore_ascii_case(note::INBOX) {
        vault::guess_note_destinations(&kb, &scan.notes)
    } else {
        vec![None; scan.notes.len()]
    };
    Ok(scan
        .notes
        .iter()
        .zip(guesses)
        .map(|(listed, guess)| {
            let mut summary = note_summary(listed, &kb);
            summary.guess = guess.map(|guess| RouteGuessDto {
                project: guess.project,
                confidence: guess.confidence,
            });
            summary
        })
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
/// `(project, id)`, only the editable fields (title, body, tags, date, type)
/// are replaced, and the filename never changes — it keeps its creation slug
/// however the title changes. The locate-merge-rewrite sequence
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
    let mut indexed = IndexedNote::from_note(
        &listed.note,
        &listed.title,
        &rel.to_string_lossy().replace('\\', "/"),
    );
    indexed.meeting = meeting::meeting_facts_for(&listed.note, &kb);
    app.state::<IndexState>().index_note_best_effort(indexed);
    broadcast_vault_changed(&app);

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

    // Keep the index in step with the move: a re-route changes the note's
    // project and path, and the upsert updates the same row.
    index_moved_note(&app.state::<IndexState>(), &routed.note, &kb);
    broadcast_vault_changed(&app);

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

/// Creates an empty project folder via `vault::create_project`, echoing the
/// canonical (casing-adopted) project row so the frontend can navigate to it
/// without waiting for a refetch.
#[tauri::command]
pub async fn create_project(app: AppHandle, project: String) -> Result<ProjectDto, String> {
    let kb = knowledge_base_dir(&app)?;
    let info = vault::create_project(&kb, &project).map_err(|err| err.to_string())?;
    broadcast_vault_changed(&app);
    Ok(project_dto(info))
}

/// The `delete_project` outcome echoed to the frontend, for the confirmation's
/// closing copy.
#[derive(serde::Serialize)]
pub struct DeletedProjectDto {
    slug: String,
    moved_note_count: u32,
}

/// Deletes a project via `vault::delete_project`: its notes (direct and in
/// child projects) move back to the Inbox first, then the folder tree is
/// removed. Index consistency comes in two best-effort layers: a per-note
/// upsert lands each move immediately (a moved note keeps its id, so no row is
/// deleted), and a reconcile sweep converges anything else without waiting on
/// the file watcher.
#[tauri::command]
pub async fn delete_project(app: AppHandle, project: String) -> Result<DeletedProjectDto, String> {
    let kb = knowledge_base_dir(&app)?;
    let deleted = vault::delete_project(&kb, &project).map_err(|err| match err {
        // The core message suggests passing create_project, which only makes
        // sense on the filing path; deletion gets plain copy.
        note::NoteError::MissingProject { project } => {
            format!("project \"{project}\" does not exist")
        }
        other => other.to_string(),
    })?;

    let index = app.state::<IndexState>();
    for listed in &deleted.moved_notes {
        index_moved_note(&index, listed, &kb);
    }
    index.request_reconcile();
    broadcast_vault_changed(&app);

    Ok(DeletedProjectDto {
        slug: deleted.slug,
        moved_note_count: deleted.moved_notes.len() as u32,
    })
}

/// Renames a project via `vault::rename_project`: the folder moves and every
/// contained note (direct and in child projects) is re-filed under the new
/// slug. Echoes the canonical project row, like `create_project`, so the
/// frontend can navigate to the renamed project without waiting for a refetch.
///
/// Index consistency is the two best-effort layers `delete_project` uses: a
/// per-note upsert lands each move immediately (the note keeps its id, so rows
/// are updated, never deleted, and embeddings survive), then a reconcile sweep
/// converges the rest. Both are needed here because the file watcher only
/// reacts to `.md` paths and a directory rename may not produce any.
#[tauri::command]
pub async fn rename_project(
    app: AppHandle,
    project: String,
    new_project: String,
) -> Result<ProjectDto, String> {
    let kb = knowledge_base_dir(&app)?;
    let renamed = vault::rename_project(&kb, &project, &new_project).map_err(|err| match err {
        // The core message suggests passing create_project, which only makes
        // sense on the filing path; renaming gets plain copy.
        note::NoteError::MissingProject { project } => {
            format!("project \"{project}\" does not exist")
        }
        other => other.to_string(),
    })?;

    if !renamed.failed_rewrites.is_empty() {
        eprintln!(
            "rename_project: {} note(s) moved to {} but still name the old project",
            renamed.failed_rewrites.len(),
            renamed.info.slug
        );
    }

    let index = app.state::<IndexState>();
    for listed in &renamed.renamed_notes {
        index_moved_note(&index, listed, &kb);
    }
    index.request_reconcile();
    broadcast_vault_changed(&app);

    Ok(project_dto(renamed.info))
}

/// The `delete_note` outcome echoed to the frontend: the removed note's id, its
/// display title and former project (both `null` when the note was already
/// gone), and whether a paired session (retained recording + transcript) was
/// removed with it. Enough for a closing toast; the lists refresh off
/// `vault:changed`. `project` is `null` for an Inbox note (the [`note_summary`]
/// convention).
#[derive(serde::Serialize)]
pub struct DeletedNoteDto {
    id: String,
    title: Option<String>,
    project: Option<String>,
    session_deleted: bool,
}

/// Permanently deletes the note carrying `id`, wherever it lives, via
/// `vault::delete_note`, then cleans up what the note leaves behind: its paired
/// session artifacts (the retained recording and transcript, for a distilled
/// note) and its derived index rows. Both cleanups are best-effort — the `.md`
/// removal already succeeded and is the source of truth, so neither may fail the
/// command.
///
/// An unknown or already-deleted id is idempotent success (`title` / `project`
/// `null`): a destructive confirm plus cross-window `vault:changed` races make
/// "already gone" a real path, and the caller's success handling (close, then
/// navigate or refetch) is correct for it.
#[tauri::command]
pub async fn delete_note(app: AppHandle, id: String) -> Result<DeletedNoteDto, String> {
    let kb = knowledge_base_dir(&app)?;
    let note_id = parse_note_id(&id)?;

    let Some(deleted) = vault::delete_note(&kb, &note_id).map_err(|err| err.to_string())? else {
        return Ok(DeletedNoteDto {
            id,
            title: None,
            project: None,
            session_deleted: false,
        });
    };

    // Best-effort cleanup: the note file is already gone.
    let session_deleted =
        sessions::delete_session_for_source(&kb, &deleted.source).unwrap_or_else(|err| {
            eprintln!("failed to clean up the session for note {id}: {err}");
            false
        });
    app.state::<IndexState>()
        .delete_note_best_effort(note_id.as_str());
    broadcast_vault_changed(&app);

    Ok(DeletedNoteDto {
        id,
        title: Some(deleted.title),
        project: deleted.former_project,
        session_deleted,
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
        // Verbatim: `Note::with_title` is the one authority on title
        // normalization, so every entry point folds identically.
        title: input.title.clone(),
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
        // Filled in by `list_notes` for Inbox rows only; every other projection
        // of a note (read, save, file) answers "where is it", not "where might
        // it go", and carries no guess.
        guess: None,
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
        // An absent title means "preserve", not "clear".
        assert_eq!(input.title, None);
        assert_eq!(note_edit_from_input(&input).unwrap().title, None);
    }

    #[test]
    fn note_edit_parses_the_wire_strings_and_folds_tag_case() {
        let input = save_input(
            r#"{"id":"n_a1b2c3","project":"Ops","type":"note","title":"  Renamed ",
                "date":"2026-07-12","tags":["Follow-Up"],"body":"Rewritten."}"#,
        );
        let edit = note_edit_from_input(&input).unwrap();
        assert_eq!(edit.note_type, NoteType::Note);
        assert_eq!(edit.tags, vec![Tag::parse("follow-up").unwrap()]);
        assert_eq!(edit.date, "2026-07-12");
        assert_eq!(edit.body, "Rewritten.");
        // Passed through untrimmed: normalizing here would duplicate
        // `Note::with_title`, which does it on the way into the note.
        assert_eq!(edit.title.as_deref(), Some("  Renamed "));
    }

    #[test]
    fn note_edit_passes_an_explicit_null_title_through_as_preserve() {
        let input = save_input(
            r#"{"id":"n_a1b2c3","project":"Ops","type":"note","title":null,
                "date":"2026-07-12","body":"Rewritten."}"#,
        );
        assert_eq!(note_edit_from_input(&input).unwrap().title, None);
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
        // The guess is `list_notes`' to fill in, per-listing; the projection
        // itself never invents one.
        assert!(dto.guess.is_none());
    }

    #[test]
    fn note_summary_serializes_guess_as_an_object_or_null() {
        let kb = Path::new("kb-root");
        let note = Note::new(
            NoteId::parse("n_a1b2c3").unwrap(),
            NoteType::Note,
            Routing::Routed {
                project: note::INBOX.to_string(),
                confidence: 0.12,
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

        // Absent: an explicit `null`, not a missing key — the frontend types
        // the field as nullable, not optional.
        let json = serde_json::to_value(note_summary(&listed, kb)).unwrap();
        assert_eq!(json["guess"], serde_json::Value::Null);

        let mut dto = note_summary(&listed, kb);
        dto.guess = Some(RouteGuessDto {
            project: "Briarwood Golf".to_string(),
            confidence: 0.41,
        });
        let json = serde_json::to_value(dto).unwrap();
        assert_eq!(json["guess"]["project"], "Briarwood Golf");
        assert_eq!(json["guess"]["confidence"], 0.41);
        // The stored score stays its own field: the two numbers answer
        // different questions and must not be conflated on the wire.
        assert_eq!(json["confidence"], 0.12);
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
