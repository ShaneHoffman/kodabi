//! Vault-level enumeration: discovering projects and listing notes directly
//! from disk. The Markdown files are the source of truth
//! (`docs/FRONTMATTER_SCHEMA.md`); the SQLite index mirrors them as a derived
//! cache (kept live by `crate::watch` + `crate::reconcile`), but browsing still
//! reads the vault itself — a scan stays O(notes-in-folder) parses, cheap for a
//! per-project folder, and needs no index to be present or current.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};

use crate::glossary;
use crate::note::{
    self, Note, NoteEdit, NoteError, NoteId, NoteType, Result, Routing, Source, INBOX,
};
use crate::routing;
use crate::routing_examples::{self, RoutingExample, RoutingExamples, RoutingExamplesError};

/// A note found on disk: its absolute path, display title, and parsed content.
#[derive(Debug, Clone, PartialEq)]
pub struct ListedNote {
    pub path: PathBuf,
    /// The note's effective display title ([`effective_title`]): its stored
    /// frontmatter `title` when present, else the de-slugged filename stem
    /// (`weekly-sync` → `weekly sync`) for a legacy or hand-made note.
    pub title: String,
    pub note: Note,
}

/// Result of scanning one project folder. `skipped` holds `.md` files that
/// failed to read or parse (corrupt frontmatter, non-note markdown) — one bad
/// file must not make the whole project unbrowsable.
#[derive(Debug, Default)]
pub struct NoteScan {
    /// Sorted by date descending (newest first), tie-broken by filename.
    pub notes: Vec<ListedNote>,
    pub skipped: Vec<PathBuf>,
}

/// A project discovered on disk; the slug mirrors the folder path
/// (`Growth/Q3`). Counts cover the folder's direct notes only — a child
/// project's notes are its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInfo {
    pub slug: String,
    pub note_count: u32,
    pub meeting_count: u32,
    /// Latest modification time among the direct notes, RFC 3339 UTC.
    pub last_activity: Option<String>,
}

/// Sort order for [`list_projects_page`] — the `sort` input of the
/// `list_projects` MCP tool. Serde renders the three lowercase spellings the
/// tool's `inputSchema` enum accepts (`name` | `last_activity` | `note_count`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectSort {
    /// Ascending by display name (the contract default).
    #[default]
    Name,
    /// Most recent activity first; never-active projects last.
    LastActivity,
    /// Most notes first.
    NoteCount,
}

/// The `list_projects` inputs — mirrors the tool's `inputSchema` field-for-field,
/// including its defaults, so an MCP wrapper can deserialize tool arguments
/// straight into it. `deny_unknown_fields` matches the schema's
/// `additionalProperties: false`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectQuery {
    /// List projects under this parent slug; the parent itself is excluded.
    /// Omit to start from the top level.
    #[serde(default)]
    pub parent: Option<String>,
    /// When set, include the full subtree (`true`) or only direct children.
    /// With no `parent`, chooses all projects (`true`) or only top-level ones.
    /// Default `true`.
    #[serde(default = "default_true")]
    pub include_descendants: bool,
    /// Include projects with zero notes. Default `true`.
    #[serde(default = "default_true")]
    pub include_empty: bool,
    /// Sort order. Default [`ProjectSort::Name`].
    #[serde(default)]
    pub sort: ProjectSort,
    /// Max projects per page, clamped to `1..=200`. Default `100`.
    #[serde(default = "default_projects_limit")]
    pub limit: u32,
    /// Opaque pagination token from a prior response's `page.next_cursor`.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// A routing-target project with hierarchy and counts — the `Project` `$def` of
/// `docs/MCP_TOOL_SURFACE.md`. Field names/order match that schema so an MCP
/// wrapper can serialize it straight out. `id` is a stable, informational
/// identifier derived from the slug ([`project_id`]); the slug is the handle
/// tools accept.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectSummary {
    pub id: String,
    pub slug: String,
    pub display_name: String,
    pub parent: Option<String>,
    pub note_count: u32,
    pub meeting_count: u32,
    pub last_activity: Option<String>,
}

/// The `list_projects` output — the matching `projects` plus the pagination
/// `page`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectPage {
    pub projects: Vec<ProjectSummary>,
    pub page: ProjectPageInfo,
}

/// Cursor-based pagination envelope for `list_projects`, mirroring the `PageInfo`
/// `$def`. The cursor names the boundary project by id (keyset, not an offset),
/// so a project added or removed elsewhere in the ordering between pages never
/// re-serves or skips the rows around that boundary. `total_estimate` is always
/// exact here — the disk scan exhausts the candidate set.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectPageInfo {
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub total_estimate: Option<u64>,
}

/// Scans `<vault>/<project>` for direct `.md` notes. The [`INBOX`] sentinel is
/// a valid target. A missing folder is an empty scan, not an error — a project
/// the UI knows about may simply not exist on disk yet.
pub fn scan_project_notes(vault_root: &Path, project: &str) -> Result<NoteScan> {
    note::validate_project(project)?;
    let dir = note::project_dir(vault_root, project);

    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(NoteScan::default()),
        Err(source) => return Err(NoteError::Io { path: dir, source }),
    };

    let mut scan = NoteScan::default();
    for entry in entries {
        let entry = entry.map_err(|source| NoteError::Io {
            path: dir.clone(),
            source,
        })?;
        let path = entry.path();
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) || !is_md_file(&path) {
            continue;
        }
        match fs::read_to_string(&path)
            .map_err(|_| ())
            .and_then(|contents| Note::from_markdown(&contents).map_err(|_| ()))
        {
            Ok(note) => scan.notes.push(ListedNote {
                title: effective_title(&note, &path),
                path,
                note,
            }),
            Err(()) => scan.skipped.push(path),
        }
    }

    scan.notes.sort_by(|a, b| {
        date_sort_key(&b.note.date)
            .cmp(&date_sort_key(&a.note.date))
            .then_with(|| a.path.file_name().cmp(&b.path.file_name()))
    });
    Ok(scan)
}

/// The router's best-guess destination for each of `notes`, scored against the
/// vault's *current* signals — what the Inbox shows on a row so an unfiled note
/// arrives with a suggestion rather than a blank.
///
/// Two things make this a fresh scoring rather than a replay of what the note
/// already carries. It scores the note's title as well as its body, where
/// capture-time routing deliberately passes `title: None` (`capture::route_preview`
/// — a quick capture's first line seeds the filename, and counting it as a title
/// signal too would double-count the same words). And it reads today's
/// glossaries and corrections, so a project taught a new term surfaces on the
/// notes already waiting rather than only on the next capture. The stored
/// `confidence` stays the historical record of why the note landed here; this is
/// the live answer to where it would go now.
///
/// Best-effort by construction: a signal-load failure degrades to all-`None`
/// rather than failing the listing. A guess is an offer, and a listing that
/// refuses to render because the router is briefly unreadable would trade the
/// user's notes for a hint.
///
/// One signal load for the whole batch; scoring itself is pure and the note
/// bodies are already in memory, so this costs no extra file reads per note.
pub fn guess_note_destinations(
    vault_root: &Path,
    notes: &[ListedNote],
) -> Vec<Option<routing::RouteGuess>> {
    let Ok(loaded) = routing::load_project_signals(vault_root) else {
        return vec![None; notes.len()];
    };
    notes
        .iter()
        .map(|listed| {
            routing::best_candidate(
                routing::NoteText {
                    title: Some(&listed.title),
                    body: &listed.note.body,
                },
                &loaded.signals,
            )
        })
        .collect()
}

/// Finds the note carrying `id` inside `project` (linear scan, unparseable
/// files skipped). `Ok(None)` when no direct note has the id. More than one
/// file claiming the id (an external file copy) is
/// [`NoteError::DuplicateNoteId`], not a silent first-match — read/save would
/// otherwise target one file while the list shows both, and an edit could
/// overwrite a file the user never had on screen.
pub fn find_note(vault_root: &Path, project: &str, id: &NoteId) -> Result<Option<ListedNote>> {
    let mut matches = scan_project_notes(vault_root, project)?
        .notes
        .into_iter()
        .filter(|listed| listed.note.id == *id);
    let Some(first) = matches.next() else {
        return Ok(None);
    };
    let duplicates: Vec<PathBuf> = matches.map(|listed| listed.path).collect();
    if !duplicates.is_empty() {
        let mut paths = vec![first.path];
        paths.extend(duplicates);
        return Err(NoteError::DuplicateNoteId {
            id: id.as_str().to_string(),
            paths,
        });
    }
    Ok(Some(first))
}

/// Resolves a typed project slug against the folders already on disk, adopting
/// the existing casing of any segment that matches case-insensitively; a
/// segment with no counterpart passes through verbatim. On Windows'
/// case-insensitive filesystem `create_dir_all("growth")` lands inside an
/// existing `Growth/`, so without this the frontmatter `project:` — the
/// schema's authoritative filing — would disagree with the folder-derived
/// sidebar slug every consumer compares against.
pub fn canonicalize_project(vault_root: &Path, project: &str) -> Result<String> {
    note::validate_project(project)?;
    if project == INBOX {
        return Ok(project.to_string());
    }
    let mut dir = vault_root.to_path_buf();
    let mut segments = Vec::new();
    for segment in project.split('/') {
        let on_disk = fs::read_dir(&dir).ok().and_then(|entries| {
            entries.filter_map(|e| e.ok()).find_map(|e| {
                let name = e.file_name();
                let name = name.to_str()?;
                (e.file_type().map(|t| t.is_dir()).unwrap_or(false)
                    && name.eq_ignore_ascii_case(segment))
                .then(|| name.to_string())
            })
        });
        // ASCII case-folding can't introduce a forbidden character, so an
        // adopted on-disk name is as valid as the typed segment it matched.
        let resolved = on_disk.unwrap_or_else(|| segment.to_string());
        dir.push(&resolved);
        segments.push(resolved);
    }
    Ok(segments.join("/"))
}

/// The whole edit path in one call: locate the note by `(project, id)`, merge
/// via [`Note::with_edits`] (preserving `id`, `source`, and routing), and
/// atomically rewrite the file in place. `Ok(None)` when no note carries the
/// id. Every shell — the Tauri `save_note` command, the coming MCP
/// `edit_note` — should call this rather than re-implementing the sequence.
///
/// The returned [`ListedNote`] carries the *post-edit* effective title, so a
/// caller that indexes or echoes it reflects a retitled note. The filename is
/// untouched: it keeps its creation slug however the title changes.
pub fn save_note_edit(
    vault_root: &Path,
    project: &str,
    id: &NoteId,
    edit: NoteEdit,
) -> Result<Option<ListedNote>> {
    let Some(listed) = find_note(vault_root, project, id)? else {
        return Ok(None);
    };
    let merged = listed.note.with_edits(edit)?;
    note::save_note_at(&listed.path, &merged)?;
    // Recomputed from the merged note, never carried over from `listed`: an
    // edit may replace the title, and the stale value would flow straight into
    // the index and the caller's echo while the file on disk said otherwise.
    let title = effective_title(&merged, &listed.path);
    Ok(Some(ListedNote {
        note: merged,
        path: listed.path,
        title,
    }))
}

/// Maximum length (in `char`s) of a correction reason — mirrors the MCP
/// `file_note_to_project` `reason` `maxLength`.
pub const REASON_MAX_CHARS: usize = 500;

/// The optional inputs of [`file_note_to_project`], mirroring the optional MCP
/// tool parameters.
#[derive(Debug, Clone, Default)]
pub struct FileNoteOptions {
    /// Create the target project folder (and any missing parents) when it does
    /// not exist. When `false`, a missing target is [`NoteError::MissingProject`].
    pub create_project: bool,
    /// Routing score to write. `None` records the human correction as `1.0`
    /// (the contract default).
    pub confidence: Option<f64>,
    /// Optional human-readable reason (≤ [`REASON_MAX_CHARS`] chars), stored in
    /// the target project's routing-examples log.
    pub reason: Option<String>,
}

/// The outcome of a re-route, mirroring the MCP `file_note_to_project` output.
#[derive(Debug)]
pub struct RoutedNote {
    /// The note after routing, at its new path (title re-derived from that path).
    pub note: ListedNote,
    /// The note's path before the move (equals the new path when `!moved`).
    pub previous_path: PathBuf,
    /// The note's project before the move; `None` when it was in the Inbox.
    pub previous_project: Option<String>,
    /// `false` when the note was already in the target project (no file move —
    /// the frontmatter is still rewritten so a repeat call converges).
    pub moved: bool,
}

/// Finds the note carrying `id` anywhere in the vault — the Inbox plus every
/// discovered project ([`list_projects`]) — returning the owning project slug
/// (`INBOX` for the Inbox) alongside the listing. `Ok(None)` when no note has
/// the id. More than one file claiming it, in one folder or across projects, is
/// [`NoteError::DuplicateNoteId`] with every path — the vault-wide analogue of
/// [`find_note`]'s duplicate guard (a re-route has no source project to scope
/// the search, so an id must be unique vault-wide before it can be moved).
pub fn find_note_anywhere(vault_root: &Path, id: &NoteId) -> Result<Option<(String, ListedNote)>> {
    let mut projects = vec![INBOX.to_string()];
    projects.extend(list_projects(vault_root)?.into_iter().map(|p| p.slug));

    let mut matches: Vec<(String, ListedNote)> = Vec::new();
    for project in projects {
        for listed in scan_project_notes(vault_root, &project)?.notes {
            if listed.note.id == *id {
                matches.push((project.clone(), listed));
            }
        }
    }

    if matches.len() > 1 {
        return Err(NoteError::DuplicateNoteId {
            id: id.as_str().to_string(),
            paths: matches.into_iter().map(|(_, listed)| listed.path).collect(),
        });
    }
    Ok(matches.into_iter().next())
}

/// Routes or re-routes the note `id` into `project` — the human correction loop
/// (`docs/MCP_TOOL_SURFACE.md` §7). Locates the note vault-wide, rewrites its
/// frontmatter `project` + `confidence` (preserving the stable `id` and every
/// other field), moves the file into the target folder, and records the
/// correction as a routing example in that folder. Returns `Ok(None)` when no
/// note carries the id (mirroring [`save_note_edit`]).
///
/// This is the shared body of the Tauri `file_note_to_project` command and the
/// future MCP tool of the same name, so both stay identical.
///
/// **Confidence:** `options.confidence` overrides the score; omitted, a manual
/// correction is recorded as `1.0`. A re-route is always a routing action, so
/// the note ends `Routed` with a `confidence` even when re-filed by hand
/// (`docs/FRONTMATTER_SCHEMA.md`).
///
/// **Failure consistency:** the note files are the source of truth. A failure
/// before or during the move leaves the vault untouched (the move rolls back
/// its own link on a removal failure). A failure while writing the derived
/// routing-examples log (after a completed move) returns an error but does *not*
/// roll the move back — the note's location and frontmatter stay consistent;
/// only the correction signal may be missing.
pub fn file_note_to_project(
    vault_root: &Path,
    id: &NoteId,
    project: &str,
    options: &FileNoteOptions,
) -> Result<Option<RoutedNote>> {
    // ① The Inbox is a routing sentinel, never a re-route target. The exact
    // sentinel slips past `validate_project`; other casings (`inbox`, `Inbox/x`)
    // are rejected by `canonicalize_project` below.
    if project == INBOX {
        return Err(NoteError::InvalidField {
            field: "project",
            detail: "a note cannot be re-routed into the Inbox".to_string(),
        });
    }
    // ② Bound the reason before doing any work.
    if let Some(reason) = &options.reason {
        if reason.chars().count() > REASON_MAX_CHARS {
            return Err(NoteError::InvalidField {
                field: "reason",
                detail: format!("reason must be at most {REASON_MAX_CHARS} characters"),
            });
        }
    }
    // ③ Adopt on-disk casing and validate the slug shape.
    let target = canonicalize_project(vault_root, project)?;

    // ④ Locate the note (unknown id → Ok(None)).
    let Some((source_project, listed)) = find_note_anywhere(vault_root, id)? else {
        return Ok(None);
    };
    let previous_path = listed.path.clone();
    let previous_project = (source_project != INBOX).then(|| source_project.clone());

    // ⑤ Rebuild the note with the new routing; every other field verbatim.
    // `Note::new` validates the confidence range.
    let confidence = options.confidence.unwrap_or(1.0);
    let example_excerpt = routing_examples::excerpt(&listed.note.body);
    let example_title = listed.title.clone();
    let moved_note = Note::new(
        listed.note.id.clone(),
        listed.note.note_type,
        Routing::Routed {
            project: target.clone(),
            confidence,
        },
        listed.note.date.clone(),
        listed.note.tags.clone(),
        listed.note.source.clone(),
        listed.note.body.clone(),
    )?;

    // ⑥ Enforce create_project *before* any directory is created (the writer
    // would otherwise auto-create the folder). A same-project re-file always
    // passes — the folder holding the note exists.
    let target_dir = note::project_dir(vault_root, &target);
    if !target_dir.is_dir() && !options.create_project {
        return Err(NoteError::MissingProject { project: target });
    }

    // ⑦ Write: same project → rewrite in place; else move the file.
    let (new_path, moved) = if source_project == target {
        note::save_note_at(&previous_path, &moved_note)?;
        (previous_path.clone(), false)
    } else {
        let new_path = note::relocate_note_file(&moved_note, &previous_path, &target_dir)?;
        (new_path, true)
    };

    // ⑧ On a move out of a real project, first drop the note's stale entry from
    // that project's log, *before* writing the new one — so if the write below
    // fails the note is left with no correction record (the documented
    // "signal may be missing" mode) rather than one in two logs at once. Skip
    // the save when nothing was removed, so no empty log file sprouts.
    if moved {
        if let Some(prev) = &previous_project {
            let prev_dir = note::project_dir(vault_root, prev);
            let mut prev_log = RoutingExamples::load(&prev_dir).map_err(routing_example_err)?;
            if prev_log.remove(id.as_str()) {
                prev_log.save(&prev_dir).map_err(routing_example_err)?;
            }
        }
    }

    // ⑨ Log the correction in the target project (overwrites any prior entry
    // for this note).
    let mut target_log = RoutingExamples::load(&target_dir).map_err(routing_example_err)?;
    target_log.upsert(RoutingExample {
        note_id: id.as_str().to_string(),
        title: example_title,
        excerpt: example_excerpt,
        previous_project: previous_project.clone(),
        confidence,
        corrected_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        reason: options.reason.clone(),
    });
    target_log.save(&target_dir).map_err(routing_example_err)?;

    // ⑩ Title prefers the note's stored frontmatter title; the (possibly
    //    suffixed) new path is only the de-slug fallback for a note without one.
    Ok(Some(RoutedNote {
        note: ListedNote {
            title: effective_title(&moved_note, &new_path),
            path: new_path,
            note: moved_note,
        },
        previous_path,
        previous_project,
        moved,
    }))
}

/// Collapses a routing-examples log error into a [`NoteError`]: an I/O error
/// keeps its path; a YAML error becomes an I/O error carrying its message (the
/// log is a derived signal, so it borrows the note error surface rather than
/// widening it).
fn routing_example_err(err: RoutingExamplesError) -> NoteError {
    match err {
        RoutingExamplesError::Io { path, source } => NoteError::Io { path, source },
        RoutingExamplesError::Yaml { path, source } => NoteError::Io {
            path,
            source: io::Error::other(source.to_string()),
        },
    }
}

/// Creates an empty project folder (and any missing parents) under the vault
/// root — the explicit sibling of [`file_note_to_project`]'s `create_project`
/// flag, for creating a project *before* any note exists to file into it.
///
/// The slug is validated and adopts the on-disk casing of existing segments
/// ([`canonicalize_project`]), which also rejects reserved and hidden names.
/// An already-existing target — in any casing — is
/// [`NoteError::ProjectExists`]: creation is an explicit user action, so
/// silently succeeding on a duplicate would hide a name collision.
pub fn create_project(vault_root: &Path, project: &str) -> Result<ProjectInfo> {
    // The exact sentinel slips past `validate_project`; every other casing is
    // rejected inside `canonicalize_project` (mirrors `file_note_to_project` ①).
    if project == INBOX {
        return Err(NoteError::InvalidField {
            field: "project",
            detail: "the Inbox is built in and cannot be created as a project".to_string(),
        });
    }
    let canonical = canonicalize_project(vault_root, project)?;
    let dir = note::project_dir(vault_root, &canonical);
    if dir.exists() {
        return Err(NoteError::ProjectExists { project: canonical });
    }
    fs::create_dir_all(&dir).map_err(|source| NoteError::Io {
        path: dir.clone(),
        source,
    })?;
    Ok(ProjectInfo {
        slug: canonical,
        note_count: 0,
        meeting_count: 0,
        last_activity: None,
    })
}

/// Maximum length (in `char`s) of a glossary term — mirrors the MCP
/// `add_glossary_term` `term` `maxLength`.
pub const GLOSSARY_TERM_MAX_CHARS: usize = 200;
/// Maximum length (in `char`s) of a glossary definition — mirrors the MCP
/// `add_glossary_term` `definition` `maxLength`.
pub const GLOSSARY_DEFINITION_MAX_CHARS: usize = 2000;

/// The outcome of [`add_glossary_term`]: the stored term and whether it was new.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlossaryUpsert {
    /// The canonical (casing-adopted) project slug the term was stored under.
    pub project: String,
    /// The term as persisted (its own text trimmed, blank aliases dropped).
    pub term: glossary::GlossaryTerm,
    /// `true` when a new term was created; `false` when an existing normalized
    /// term was updated in place.
    pub created: bool,
}

/// Why [`add_glossary_term`] could not store a term. Kept distinct from
/// [`NoteError`] and [`glossary::GlossaryError`] so a shell can route each case
/// to the right channel: a malformed slug or field is a caller bug, a missing
/// project or an `on_conflict: "error"` hit are actionable business faults, and
/// storage failures are internal.
#[derive(Debug, thiserror::Error)]
pub enum AddGlossaryTermError {
    /// The project slug is malformed (shape or reserved-name violation).
    #[error(transparent)]
    InvalidProject(NoteError),
    /// `term` or `definition` is blank or over its length bound.
    #[error("invalid glossary {field}: {detail}")]
    InvalidInput { field: &'static str, detail: String },
    /// The target project folder does not exist (this tool never creates one).
    #[error("project {project:?} does not exist")]
    MissingProject { project: String },
    /// `on_conflict: "error"` and the normalized term already exists.
    #[error("glossary term {term:?} already exists")]
    Conflict { term: String },
    /// Reading or writing the glossary file failed (I/O, YAML, or a duplicate
    /// term already on disk).
    #[error(transparent)]
    Storage(glossary::GlossaryError),
}

/// Adds or updates a glossary term for `project` — the MCP `add_glossary_term`
/// tool's shared body (`docs/MCP_TOOL_SURFACE.md` §8). Resolves the project slug
/// to its folder (adopting on-disk casing), requires the project to already
/// exist (unlike [`file_note_to_project`], this never creates one), then loads,
/// upserts by normalized term, and saves the project's `_glossary.yml`.
///
/// Returns the stored term (as persisted) and whether it was newly created.
/// `on_conflict` decides an existing normalized term: [`OnConflict::Update`]
/// overwrites it, [`OnConflict::Error`] leaves it untouched and returns
/// [`AddGlossaryTermError::Conflict`].
///
/// [`OnConflict::Update`]: glossary::OnConflict::Update
/// [`OnConflict::Error`]: glossary::OnConflict::Error
pub fn add_glossary_term(
    vault_root: &Path,
    project: &str,
    term: glossary::GlossaryTerm,
    on_conflict: glossary::OnConflict,
) -> std::result::Result<GlossaryUpsert, AddGlossaryTermError> {
    // Mirror the input schema's bounds server-side: a term that trims to empty
    // would persist as a blank key, and both fields are length-capped.
    if term.term.trim().is_empty() {
        return Err(AddGlossaryTermError::InvalidInput {
            field: "term",
            detail: "term must not be blank".to_string(),
        });
    }
    if term.term.chars().count() > GLOSSARY_TERM_MAX_CHARS {
        return Err(AddGlossaryTermError::InvalidInput {
            field: "term",
            detail: format!("term must be at most {GLOSSARY_TERM_MAX_CHARS} characters"),
        });
    }
    if term.definition.trim().is_empty() {
        return Err(AddGlossaryTermError::InvalidInput {
            field: "definition",
            detail: "definition must not be blank".to_string(),
        });
    }
    if term.definition.chars().count() > GLOSSARY_DEFINITION_MAX_CHARS {
        return Err(AddGlossaryTermError::InvalidInput {
            field: "definition",
            detail: format!(
                "definition must be at most {GLOSSARY_DEFINITION_MAX_CHARS} characters"
            ),
        });
    }

    // Validate the slug shape and adopt on-disk casing (rejects reserved/hidden
    // names and the Inbox). A malformed slug is a caller bug, not a missing
    // project.
    let canonical =
        canonicalize_project(vault_root, project).map_err(AddGlossaryTermError::InvalidProject)?;
    let dir = note::project_dir(vault_root, &canonical);
    if !dir.is_dir() {
        return Err(AddGlossaryTermError::MissingProject { project: canonical });
    }

    let lookup = term.term.clone();
    let mut glossary = glossary::Glossary::load(&dir).map_err(glossary_upsert_err)?;
    let created = glossary
        .upsert(term, on_conflict)
        .map_err(glossary_upsert_err)?;
    glossary.save(&dir).map_err(glossary_upsert_err)?;

    // Echo exactly what landed on disk. `upsert` just inserted/updated this
    // term, so the lookup is guaranteed to hit.
    let stored = glossary
        .get(&lookup)
        .cloned()
        .expect("the just-upserted term is present in the glossary");
    Ok(GlossaryUpsert {
        project: canonical,
        term: stored,
        created,
    })
}

/// Routes a [`glossary::GlossaryError`] into [`AddGlossaryTermError`]: an
/// `on_conflict: "error"` hit is a distinct business fault the shell surfaces
/// differently from an I/O or YAML failure.
fn glossary_upsert_err(err: glossary::GlossaryError) -> AddGlossaryTermError {
    match err {
        glossary::GlossaryError::Conflict { term } => AddGlossaryTermError::Conflict { term },
        other => AddGlossaryTermError::Storage(other),
    }
}

/// The outcome of a project deletion: the canonical slug removed, and every
/// contained note (direct and descendant) relocated into the Inbox at its new
/// path, so the caller can re-index the moves.
#[derive(Debug)]
pub struct DeletedProject {
    pub slug: String,
    pub moved_notes: Vec<ListedNote>,
}

/// Deletes a project: every parseable contained note — direct and in child
/// projects — is moved back to the Inbox, the per-project infra files
/// (`_glossary.yml`, `_routing_examples.yml`, writer scratch temps) are
/// removed, and the folder tree is deleted.
///
/// **No-data-loss guard:** the tree is walked *before* anything is touched, and
/// any item Kodabi does not manage (an attachment, an unparseable `.md`, a
/// hidden directory) fails the whole call up front. Removal then deletes only
/// the classified infra files and the emptied directories — never a recursive
/// `remove_dir_all` — so nothing unclassified can be destroyed even if the tree
/// changes mid-flight.
///
/// Moved notes keep every field and their filename stem (numbered suffix on an
/// Inbox collision), and land as `Routed { Inbox, 0.0 }` — the same shape
/// routing writes for a note with no routing evidence. Leaving the project is
/// not a correction, so no routing example is recorded; the deleted project's
/// own log dies with its folder, and `previous_project` strings in other
/// projects' logs stay as historical provenance.
///
/// **Failure consistency:** the note files are the source of truth. A failure
/// mid-move leaves the already-moved notes validly in the Inbox and the rest
/// untouched in the still-existing project; a retry converges. Reconcile keeps
/// the index truthful in every intermediate state.
pub fn delete_project(vault_root: &Path, project: &str) -> Result<DeletedProject> {
    // Mirrors `file_note_to_project` ①: the exact sentinel slips past
    // `validate_project`; other casings are rejected by `canonicalize_project`.
    if project == INBOX {
        return Err(NoteError::InvalidField {
            field: "project",
            detail: "the Inbox is built in and cannot be deleted".to_string(),
        });
    }
    let canonical = canonicalize_project(vault_root, project)?;
    let dir = note::project_dir(vault_root, &canonical);
    if !dir.is_dir() {
        return Err(NoteError::MissingProject { project: canonical });
    }

    // ① Pre-flight: classify the whole tree before touching anything.
    let mut scan = TreeScan::default();
    walk_project_tree(&dir, &mut scan)?;
    if let Some(first) = scan.unmanaged.first() {
        let example = first
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| first.display().to_string());
        return Err(NoteError::InvalidField {
            field: "project",
            detail: format!(
                "project {canonical:?} contains {} item(s) Kodabi does not manage \
                 (e.g. {example:?}); move or remove them first",
                scan.unmanaged.len()
            ),
        });
    }

    // ② Move every note into the Inbox, preserving its stem and every field
    // but the routing. `Note::new` re-validates; `relocate_note_file` creates
    // the Inbox dir, disambiguates collisions, and rolls back on failure.
    let inbox_dir = note::project_dir(vault_root, INBOX);
    let mut moved_notes = Vec::with_capacity(scan.notes.len());
    for (path, found) in scan.notes {
        let moved = Note::new(
            found.id,
            found.note_type,
            Routing::Routed {
                project: INBOX.to_string(),
                confidence: 0.0,
            },
            found.date,
            found.tags,
            found.source,
            found.body,
        )?;
        let new_path = note::relocate_note_file(&moved, &path, &inbox_dir)?;
        moved_notes.push(ListedNote {
            title: effective_title(&moved, &new_path),
            path: new_path,
            note: moved,
        });
    }

    // ③ Remove the infra files, then the directories deepest-first (`dirs` is
    // in pre-order, so reverse iteration removes children before parents). The
    // non-recursive `remove_dir` is the guard made real: a directory holding
    // an unexpected new file fails rather than deletes it.
    for file in &scan.infra_files {
        fs::remove_file(file).map_err(|source| NoteError::Io {
            path: file.clone(),
            source,
        })?;
    }
    for tree_dir in scan.dirs.iter().rev() {
        fs::remove_dir(tree_dir).map_err(|source| NoteError::Io {
            path: tree_dir.clone(),
            source,
        })?;
    }

    Ok(DeletedProject {
        slug: canonical,
        moved_notes,
    })
}

/// One project tree classified for [`delete_project`]. `dirs` is in pre-order
/// (parent before child), so reverse iteration removes children first.
#[derive(Debug, Default)]
struct TreeScan {
    notes: Vec<(PathBuf, Note)>,
    infra_files: Vec<PathBuf>,
    dirs: Vec<PathBuf>,
    unmanaged: Vec<PathBuf>,
}

/// Depth-first classification for [`delete_project`]: parseable `.md` files
/// are notes to move, the per-project infra files are removable, and anything
/// else — an unparseable `.md`, an attachment, a hidden or infra directory, a
/// non-file-non-dir entry — is unmanaged and blocks deletion. Unlike
/// enumeration's tolerant walks, an unreadable entry here is an error:
/// deletion must account for every item it is about to disturb.
fn walk_project_tree(dir: &Path, scan: &mut TreeScan) -> Result<()> {
    scan.dirs.push(dir.to_path_buf());
    let entries = fs::read_dir(dir).map_err(|source| NoteError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| NoteError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| NoteError::Io {
            path: path.clone(),
            source,
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            scan.unmanaged.push(path);
            continue;
        };
        if file_type.is_dir() {
            if is_project_segment(name) {
                walk_project_tree(&path, scan)?;
            } else {
                scan.unmanaged.push(path);
            }
        } else if file_type.is_file() {
            if is_md_file(&path) {
                match fs::read_to_string(&path)
                    .ok()
                    .and_then(|contents| Note::from_markdown(&contents).ok())
                {
                    Some(found) => scan.notes.push((path, found)),
                    None => scan.unmanaged.push(path),
                }
            } else if is_removable_infra(name) {
                scan.infra_files.push(path);
            } else {
                scan.unmanaged.push(path);
            }
        } else {
            scan.unmanaged.push(path);
        }
    }
    Ok(())
}

/// The outcome of a note deletion: the removed note's stable id, its former
/// project (`None` when it sat in the Inbox), its path just before removal, its
/// display title, and its `source:` — enough for the caller to clean up the
/// note's paired session artifacts and drop its index rows without re-reading
/// disk.
#[derive(Debug)]
pub struct DeletedNote {
    pub id: NoteId,
    pub former_project: Option<String>,
    pub path: PathBuf,
    pub title: String,
    pub source: Source,
}

/// Permanently deletes the note carrying `id`, wherever it lives — the Inbox or
/// any project. Locates it vault-wide ([`find_note_anywhere`], so a delete needs
/// no source-project scope), removes its `.md` file, and returns a
/// [`DeletedNote`] describing what was removed. `Ok(None)` when no note carries
/// the id (mirroring [`file_note_to_project`] / [`save_note_edit`]); an id
/// claimed by more than one file is [`NoteError::DuplicateNoteId`] and deletes
/// nothing — an ambiguous pair is never nuked.
///
/// Only the note file is touched here: the derived index rows and any paired
/// session artifacts (its retained recording and transcript) are the caller's
/// to remove, keeping `vault` free of an `index` / `sessions` dependency. A file
/// that vanished between the locate and the removal (a racing delete) is
/// success — the note is already gone, which is the goal.
pub fn delete_note(vault_root: &Path, id: &NoteId) -> Result<Option<DeletedNote>> {
    let Some((project, listed)) = find_note_anywhere(vault_root, id)? else {
        return Ok(None);
    };
    let deleted = DeletedNote {
        id: id.clone(),
        former_project: (project != INBOX).then_some(project),
        path: listed.path.clone(),
        title: listed.title,
        source: listed.note.source.clone(),
    };
    match fs::remove_file(&listed.path) {
        Ok(()) => Ok(Some(deleted)),
        // A racing delete (retention sweep, another window) already removed it —
        // the goal is met; report the same success.
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(Some(deleted)),
        Err(source) => Err(NoteError::Io {
            path: listed.path,
            source,
        }),
    }
}

/// The files Kodabi itself plants in a project folder and may therefore delete
/// with it: the glossary and routing-examples logs, plus their (and the note
/// writer's) crash-leftover scratch temps.
fn is_removable_infra(name: &str) -> bool {
    name == glossary::GLOSSARY_FILE
        || name == routing_examples::ROUTING_EXAMPLES_FILE
        || (name.ends_with(".tmp")
            && (name.starts_with(".note.")
                || name.starts_with(glossary::GLOSSARY_FILE)
                || name.starts_with(routing_examples::ROUTING_EXAMPLES_FILE)))
}

/// Discovers project folders under the KB root, sorted by slug (so a parent
/// precedes its children). Every directory whose name passes the filters is a
/// project, including a note-free one (zero counts, no activity) — so a
/// freshly created empty project ([`create_project`]) is enumerable and
/// filable immediately, matching routing's project discovery and the MCP
/// `list_projects` default (`include_empty: true`). Excluded outright:
/// the Inbox sentinel and the reserved root dirs (`note::RESERVED_ROOT_DIRS`,
/// any casing), and at any depth dirs starting with `.` or `_` or whose name
/// is not a legal project segment. A missing root yields an empty list. An
/// unreadable subtree is skipped, not fatal — only a root that itself fails to
/// read errors.
pub fn list_projects(vault_root: &Path) -> Result<Vec<ProjectInfo>> {
    let mut projects = Vec::new();
    let entries = match fs::read_dir(vault_root) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(projects),
        Err(source) => {
            return Err(NoteError::Io {
                path: vault_root.to_path_buf(),
                source,
            })
        }
    };

    for entry in entries {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if is_reserved_root_dir(name) || !is_project_segment(name) {
            continue;
        }
        collect_project(&entry.path(), name.to_string(), &mut projects);
    }

    projects.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(projects)
}

/// Depth-first walk of one candidate project folder. Appends the folder and
/// every name-qualifying descendant to `out` with their direct note counts —
/// existence is what makes a directory a project, not its contents.
/// An unreadable directory (ACL-denied, a cloud-sync placeholder) skips just
/// that subtree — the directory-level mirror of the "one bad file must not
/// make the whole project unbrowsable" contract, so one locked folder can't
/// blank the entire sidebar.
fn collect_project(dir: &Path, slug: String, out: &mut Vec<ProjectInfo>) {
    let mut note_count = 0u32;
    let mut meeting_count = 0u32;
    let mut latest: Option<SystemTime> = None;
    let mut child_dirs = Vec::new();

    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if is_project_segment(name) {
                child_dirs.push((path, format!("{slug}/{name}")));
            }
        } else if file_type.is_file()
            && is_md_file(&path)
            && fs::read_to_string(&path).is_ok_and(|contents| {
                match Note::from_markdown(&contents) {
                    Ok(note) => {
                        note_count += 1;
                        if note.note_type == NoteType::Meeting {
                            meeting_count += 1;
                        }
                        true
                    }
                    Err(_) => false,
                }
            })
        {
            if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
                latest = Some(latest.map_or(modified, |prev| prev.max(modified)));
            }
        }
    }

    for (path, child_slug) in child_dirs {
        collect_project(&path, child_slug, out);
    }

    out.push(ProjectInfo {
        slug,
        note_count,
        meeting_count,
        last_activity: latest
            .map(|time| DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)),
    });
}

fn default_true() -> bool {
    true
}

fn default_projects_limit() -> u32 {
    DEFAULT_PROJECTS_LIMIT
}

/// `list_projects` page-size bounds, from the tool's `inputSchema`.
const MIN_PROJECTS_LIMIT: u32 = 1;
const MAX_PROJECTS_LIMIT: u32 = 200;
const DEFAULT_PROJECTS_LIMIT: u32 = 100;

/// A decoded [`list_projects_page`] cursor: which project the prior page ended
/// on, and how many positions prior pages consumed.
struct ProjectCursor {
    /// Positions served by prior pages. Only a fallback when the boundary id is
    /// gone; never trusted to locate the boundary, so a stale value costs a
    /// little work and cannot misplace a page.
    served: usize,
    id: String,
}

/// A stable, informational project id derived from the slug: `p_` followed by
/// the 64-bit FNV-1a hash of the slug bytes as 16 lowercase hex digits. Hex is a
/// subset of `[0-9a-z]`, so this always satisfies the `Project.id` pattern
/// `^p_[0-9a-z]{6,}$`; it is deterministic across runs and collision-negligible
/// for a personal knowledge base. The slug — not this id — is the handle every
/// tool accepts, so a non-cryptographic hash is sufficient.
fn project_id(slug: &str) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for byte in slug.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("p_{hash:016x}")
}

/// Projects a scanned [`ProjectInfo`] to the wire [`ProjectSummary`] (the
/// `Project` `$def`). Shared by `list_projects_page` and
/// [`crate::project_context`] so the same project reads identically through
/// either tool.
pub(crate) fn project_summary(info: ProjectInfo) -> ProjectSummary {
    let (parent, display_name) = split_slug(&info.slug);
    ProjectSummary {
        id: project_id(&info.slug),
        slug: info.slug,
        display_name,
        parent,
        note_count: info.note_count,
        meeting_count: info.meeting_count,
        last_activity: info.last_activity,
    }
}

/// Splits a slug into (`parent`, `display_name`): the last path segment is the
/// display name, everything before it the parent slug (`None` at top level).
/// Mirrors the desktop shell's `project_dto`.
fn split_slug(slug: &str) -> (Option<String>, String) {
    match slug.rsplit_once('/') {
        Some((parent, name)) => (Some(parent.to_string()), name.to_string()),
        None => (None, slug.to_string()),
    }
}

/// Encodes a [`list_projects_page`] cursor: the positions consumed so far and
/// the boundary project's id. A project id never contains `:`, so it can occupy
/// the unbounded last field.
fn encode_projects_cursor(served: usize, id: &str) -> String {
    format!("v1:{served}:{id}")
}

/// Decodes a [`list_projects_page`] cursor, rejecting anything this function did
/// not produce. A malformed token is a caller error, surfaced as
/// [`NoteError::InvalidField`] on the `cursor` field.
fn decode_projects_cursor(raw: &str) -> Result<ProjectCursor> {
    let bad = || NoteError::InvalidField {
        field: "cursor",
        detail: format!("malformed pagination cursor {raw:?}"),
    };
    let mut parts = raw.splitn(3, ':');
    let version = parts.next().ok_or_else(bad)?;
    let served = parts.next().ok_or_else(bad)?;
    let id = parts.next().ok_or_else(bad)?;
    if version != "v1" || id.is_empty() {
        return Err(bad());
    }
    let served: usize = served.parse().map_err(|_| bad())?;
    Ok(ProjectCursor {
        served,
        id: id.to_string(),
    })
}

/// Enumerates routing-target projects with hierarchy and counts, paginated —
/// the disk-backed engine behind the `list_projects` MCP tool.
///
/// Wraps [`list_projects`] (a full vault scan: sorted by slug, empty folders
/// included, Inbox and reserved roots excluded) and layers on the tool's
/// `parent` filter, the `include_descendants`/`include_empty` toggles, `sort`,
/// and keyset cursor pagination, synthesizing each project's
/// `id`/`display_name`/`parent` from its slug. A `parent` that matches nothing
/// yields an empty page — absence is a valid answer. `total_estimate` is always
/// the exact matched count, since the scan exhausts the candidate set. A
/// malformed `cursor` is a [`NoteError::InvalidField`].
pub fn list_projects_page(vault_root: &Path, query: &ProjectQuery) -> Result<ProjectPage> {
    let limit = query.limit.clamp(MIN_PROJECTS_LIMIT, MAX_PROJECTS_LIMIT) as usize;
    // Validate the cursor before the scan, so a malformed token is rejected the
    // same way whatever the disk holds.
    let cursor = query
        .cursor
        .as_deref()
        .map(decode_projects_cursor)
        .transpose()?;

    let mut infos = list_projects(vault_root)?;

    // Parent filter. A `parent/` prefix (with the trailing slash) matches only
    // true descendants — never a sibling like `Growthx` under `Growth` — and
    // excludes the parent folder itself.
    if let Some(parent) = query.parent.as_deref() {
        let prefix = format!("{parent}/");
        infos.retain(|info| match info.slug.strip_prefix(&prefix) {
            Some(rest) => query.include_descendants || !rest.contains('/'),
            None => false,
        });
    } else if !query.include_descendants {
        infos.retain(|info| !info.slug.contains('/'));
    }

    if !query.include_empty {
        infos.retain(|info| info.note_count > 0);
    }

    let mut projects: Vec<ProjectSummary> = infos.into_iter().map(project_summary).collect();

    // Total order with `slug` (globally unique) as the tiebreaker, so keyset
    // pagination by id stays deterministic even when the primary key ties.
    match query.sort {
        ProjectSort::Name => projects.sort_by(|a, b| {
            a.display_name
                .cmp(&b.display_name)
                .then_with(|| a.slug.cmp(&b.slug))
        }),
        ProjectSort::LastActivity => projects.sort_by(|a, b| {
            // Descending: `Some` (active) outranks `None`, and among timestamps a
            // lexical compare of the UTC RFC 3339 strings is a chronological one.
            b.last_activity
                .cmp(&a.last_activity)
                .then_with(|| a.slug.cmp(&b.slug))
        }),
        ProjectSort::NoteCount => projects.sort_by(|a, b| {
            b.note_count
                .cmp(&a.note_count)
                .then_with(|| a.slug.cmp(&b.slug))
        }),
    }

    let total = projects.len();

    // Resume by boundary id (keyset). A project inserted above the boundary
    // since the prior page is simply not re-served; a boundary project that has
    // since vanished falls back to the served count so the walk still advances.
    let start = match &cursor {
        Some(key) => projects
            .iter()
            .position(|project| project.id == key.id)
            .map_or_else(|| key.served.min(total), |index| index + 1),
        None => 0,
    };
    let end = start.saturating_add(limit).min(total);
    let has_more = end < total;
    let next_cursor = has_more
        .then(|| {
            projects
                .get(end.saturating_sub(1))
                .map(|boundary| encode_projects_cursor(end, &boundary.id))
        })
        .flatten();

    let page = projects.get(start..end).unwrap_or(&[]).to_vec();

    Ok(ProjectPage {
        projects: page,
        page: ProjectPageInfo {
            has_more,
            next_cursor,
            total_estimate: Some(total as u64),
        },
    })
}

/// Every raw-artifact `source:` value claimed by a note anywhere in the vault,
/// in one walk.
///
/// The Inbox plus every project subtree is visited; the other reserved root
/// dirs (`note::RESERVED_ROOT_DIRS`) and hidden/infrastructure folders never
/// hold notes and are skipped. Notes carrying a keyword source contribute
/// nothing.
///
/// Tolerant by the same contract as [`list_projects`]: an unreadable subtree or
/// an unparseable note is skipped rather than fatal, so one ACL-denied folder
/// can't blank a caller's whole view. Only a vault root that itself fails to
/// read errors. Callers that use this to find *unclaimed* artifacts should know
/// the direction of that tolerance: a note skipped here claims nothing, so its
/// session can resurface as unclaimed.
pub fn collect_raw_artifact_sources(vault_root: &Path) -> Result<HashSet<String>> {
    let mut sources = HashSet::new();
    let entries = match fs::read_dir(vault_root) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(sources),
        Err(source) => {
            return Err(NoteError::Io {
                path: vault_root.to_path_buf(),
                source,
            })
        }
    };

    for entry in entries {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        // The Inbox is a real note folder even though it is never a *project*,
        // so it is the one reserved root name that is walked.
        let is_inbox = name.eq_ignore_ascii_case(INBOX);
        if !is_inbox && (is_reserved_root_dir(name) || !is_project_segment(name)) {
            continue;
        }
        collect_sources_in(&entry.path(), &mut sources);
    }
    Ok(sources)
}

/// Depth-first half of [`collect_raw_artifact_sources`]. Silent on error at
/// every level, per that function's tolerance contract.
fn collect_sources_in(dir: &Path, out: &mut HashSet<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if is_project_segment(name) {
                collect_sources_in(&path, out);
            }
        } else if file_type.is_file() && is_md_file(&path) {
            let Ok(contents) = fs::read_to_string(&path) else {
                continue;
            };
            if let Ok(Note {
                source: Source::RawArtifact(artifact),
                ..
            }) = Note::from_markdown(&contents)
            {
                out.insert(artifact);
            }
        }
    }
}

pub(crate) fn is_md_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
}

/// Root entries that are never project folders: the Inbox sentinel (owned by
/// routing, pinned separately in the UI) plus the reserved root dirs
/// (`note::RESERVED_ROOT_DIRS` — the same list `validate_project` rejects, so
/// nothing writable can ever be unlistable).
fn is_reserved_root_dir(name: &str) -> bool {
    name.eq_ignore_ascii_case(INBOX)
        || note::RESERVED_ROOT_DIRS
            .iter()
            .any(|reserved| name.eq_ignore_ascii_case(reserved))
}

/// Whether a directory name can be a project path segment: not hidden or
/// infrastructure (`.`/`_` prefixes) and a name the writer itself would emit.
fn is_project_segment(name: &str) -> bool {
    !name.starts_with('.') && !name.starts_with('_') && note::validate_folder_segment(name).is_ok()
}

/// The display title for a note: its stored frontmatter `title` when present,
/// else the de-slugged filename stem ([`display_title`]).
///
/// This is the single seam that prefers the stored title over the lossy
/// filename fallback. Both the disk listing ([`ListedNote::title`]) and the
/// index writer derive the title this way, so a note's indexed title matches
/// what a later read shows. A legacy or hand-made note without the `title` key
/// falls through to the de-slugged stem, unchanged — and because that value is
/// identical to what the index already holds, it does not spuriously re-index.
pub fn effective_title(note: &Note, path: &Path) -> String {
    note.title.clone().unwrap_or_else(|| display_title(path))
}

/// De-slugged display title from the filename stem: hyphens become spaces
/// (`weekly-sync` → `weekly sync`); an id-fallback stem (`n_a1b2c3`) has no
/// hyphens and passes through unchanged.
///
/// The fallback used by [`effective_title`] when a note carries no frontmatter
/// `title` — the filename is the slug of the title, so the stem is the only
/// faithful source. Lossy for long titles (capped at the 40-char slug length)
/// and for casing/punctuation, which is exactly why a stored `title` is
/// preferred when present.
pub fn display_title(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .replace('-', " ")
}

/// Collapses a schema-valid date (RFC 3339 with offset, or date-only) to a
/// sortable UTC instant; date-only sorts at midnight UTC. Unreachable fallback
/// (parsed notes are validated): the epoch.
fn date_sort_key(date: &str) -> DateTime<Utc> {
    if let Ok(timestamp) = DateTime::parse_from_rfc3339(date) {
        return timestamp.with_timezone(&Utc);
    }
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .and_then(|day| day.and_hms_opt(0, 0, 0))
        .map(|midnight| midnight.and_utc())
        .unwrap_or(DateTime::UNIX_EPOCH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note::{Routing, Source, Tag};
    use tempfile::tempdir;

    // --- helpers ----------------------------------------------------------

    fn note_in(project: &str, id: &str, date: &str, note_type: NoteType) -> Note {
        let routing = if project == INBOX {
            Routing::Routed {
                project: project.to_string(),
                confidence: 0.4,
            }
        } else {
            Routing::Manual {
                project: project.to_string(),
            }
        };
        Note::new(
            NoteId::parse(id).unwrap(),
            note_type,
            routing,
            date,
            vec![Tag::parse("fixture").unwrap()],
            Source::parse("manual").unwrap(),
            "Body.",
        )
        .unwrap()
    }

    fn write(vault: &Path, project: &str, id: &str, date: &str, title: Option<&str>) -> PathBuf {
        note::write_note(vault, &note_in(project, id, date, NoteType::Note), title).unwrap()
    }

    // --- effective_title --------------------------------------------------

    #[test]
    fn effective_title_prefers_stored_title_else_de_slugs_the_path() {
        let path = Path::new("/vault/Ops/weekly-sync-notes.md");

        // No frontmatter title: fall back to the de-slugged filename stem.
        let untitled = note_in("Ops", "n_aaaaaa", "2026-07-10", NoteType::Note);
        assert_eq!(effective_title(&untitled, path), "weekly sync notes");

        // A stored title wins verbatim — the casing and length the slug loses.
        let titled =
            untitled.with_title(Some("Weekly Sync: Q3 planning and headcount".to_string()));
        assert_eq!(
            effective_title(&titled, path),
            "Weekly Sync: Q3 planning and headcount"
        );
    }

    // --- scan_project_notes ------------------------------------------------

    #[test]
    fn scan_missing_or_empty_project_dir_yields_empty_scan() {
        let vault = tempdir().unwrap();
        let scan = scan_project_notes(vault.path(), "Ops").unwrap();
        assert!(scan.notes.is_empty() && scan.skipped.is_empty());

        fs::create_dir(vault.path().join("Ops")).unwrap();
        let scan = scan_project_notes(vault.path(), "Ops").unwrap();
        assert!(scan.notes.is_empty() && scan.skipped.is_empty());
    }

    #[test]
    fn scan_lists_notes_sorted_by_date_desc_across_date_shapes() {
        let vault = tempdir().unwrap();
        // Offsets chosen so verbatim string order differs from instant order:
        // 2026-07-11T23:00-07:00 is 2026-07-12T06:00Z, after the 12th date-only.
        write(
            vault.path(),
            "Ops",
            "n_aaaaaa",
            "2026-07-10",
            Some("oldest"),
        );
        write(
            vault.path(),
            "Ops",
            "n_bbbbbb",
            "2026-07-12",
            Some("middle"),
        );
        write(
            vault.path(),
            "Ops",
            "n_cccccc",
            "2026-07-11T23:00:00-07:00",
            Some("newest"),
        );

        let scan = scan_project_notes(vault.path(), "Ops").unwrap();
        let titles: Vec<&str> = scan.notes.iter().map(|n| n.title.as_str()).collect();
        assert_eq!(titles, ["newest", "middle", "oldest"]);
        assert!(scan.skipped.is_empty());
    }

    #[test]
    fn scan_derives_title_from_filename_stem() {
        let vault = tempdir().unwrap();
        write(
            vault.path(),
            "Ops",
            "n_aaaaaa",
            "2026-07-10",
            Some("Weekly Sync!"),
        );
        write(vault.path(), "Ops", "n_bbbbbb", "2026-07-11", None); // id fallback

        let scan = scan_project_notes(vault.path(), "Ops").unwrap();
        let titles: Vec<&str> = scan.notes.iter().map(|n| n.title.as_str()).collect();
        assert_eq!(titles, ["n_bbbbbb", "weekly sync"]);
    }

    #[test]
    fn scan_skips_corrupt_and_non_note_md_and_reports_them() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Ops", "n_aaaaaa", "2026-07-10", Some("good"));
        let readme = vault.path().join("Ops").join("README.md");
        fs::write(&readme, "# Just a readme\n\nNo frontmatter here.\n").unwrap();
        let corrupt = vault.path().join("Ops").join("corrupt.md");
        fs::write(&corrupt, "---\nid: not-a-note-id\n---\n").unwrap();

        let scan = scan_project_notes(vault.path(), "Ops").unwrap();
        assert_eq!(scan.notes.len(), 1);
        assert_eq!(scan.notes[0].title, "good");
        let mut skipped = scan.skipped.clone();
        skipped.sort();
        assert_eq!(skipped, vec![readme, corrupt]); // byte-wise: 'R' < 'c'
    }

    #[test]
    fn scan_ignores_non_md_files_and_subdirectories() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Ops", "n_aaaaaa", "2026-07-10", Some("only"));
        fs::write(
            vault.path().join("Ops").join("_glossary.yml"),
            "terms: []\n",
        )
        .unwrap();
        fs::write(vault.path().join("Ops").join(".note.123.0.tmp"), "half").unwrap();
        fs::create_dir(vault.path().join("Ops").join("Sub")).unwrap();

        let scan = scan_project_notes(vault.path(), "Ops").unwrap();
        assert_eq!(scan.notes.len(), 1);
        assert!(scan.skipped.is_empty());
    }

    #[test]
    fn scan_rejects_traversal_and_invalid_project() {
        let vault = tempdir().unwrap();
        assert!(scan_project_notes(vault.path(), "../outside").is_err());
        assert!(scan_project_notes(vault.path(), "a\\b").is_err());
        assert!(scan_project_notes(vault.path(), "").is_err());
    }

    // --- guess_note_destinations -------------------------------------------

    /// An Inbox note whose body is the routing evidence under test.
    fn write_inbox_note_with_body(vault: &Path, id: &str, title: &str, body: &str) -> PathBuf {
        let note = Note::new(
            NoteId::parse(id).unwrap(),
            NoteType::Note,
            Routing::Routed {
                project: INBOX.to_string(),
                confidence: 0.1,
            },
            "2026-07-10",
            Vec::new(),
            Source::parse("manual").unwrap(),
            body,
        )
        .unwrap();
        note::write_note(vault, &note, Some(title)).unwrap()
    }

    #[test]
    fn guess_note_destinations_scores_inbox_notes_against_current_signals() {
        let vault = tempdir().unwrap();
        // Two projects exist as folders, so discovery finds them as candidates.
        fs::create_dir(vault.path().join("Briarwood Golf")).unwrap();
        fs::create_dir(vault.path().join("Riverbend Deck")).unwrap();

        write_inbox_note_with_body(
            vault.path(),
            "n_aaaaaa",
            "sprinkler quotes",
            "Vendor quotes for the Briarwood Golf irrigation heads.",
        );
        write_inbox_note_with_body(
            vault.path(),
            "n_bbbbbb",
            "drive home ideas",
            "A shared punch list, one place where all of it lives.",
        );

        let scan = scan_project_notes(vault.path(), INBOX).unwrap();
        let guesses = guess_note_destinations(vault.path(), &scan.notes);
        assert_eq!(guesses.len(), scan.notes.len());

        let named = scan
            .notes
            .iter()
            .zip(&guesses)
            .find(|(listed, _)| listed.note.id.as_str() == "n_aaaaaa")
            .map(|(_, guess)| guess)
            .unwrap();
        assert_eq!(
            named.as_ref().map(|guess| guess.project.as_str()),
            Some("Briarwood Golf")
        );

        // No evidence for either candidate is no suggestion, not a coin flip.
        let unmatched = scan
            .notes
            .iter()
            .zip(&guesses)
            .find(|(listed, _)| listed.note.id.as_str() == "n_bbbbbb")
            .map(|(_, guess)| guess)
            .unwrap();
        assert_eq!(*unmatched, None);
    }

    #[test]
    fn guess_note_destinations_is_all_none_for_an_empty_vault() {
        let vault = tempdir().unwrap();
        write_inbox_note_with_body(
            vault.path(),
            "n_aaaaaa",
            "sprinkler quotes",
            "Vendor quotes for the Briarwood Golf irrigation heads.",
        );
        let scan = scan_project_notes(vault.path(), INBOX).unwrap();
        let guesses = guess_note_destinations(vault.path(), &scan.notes);
        assert_eq!(guesses, vec![None]);
    }

    // --- find_note ---------------------------------------------------------

    #[test]
    fn find_note_locates_by_id_and_returns_none_for_unknown() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Ops", "n_aaaaaa", "2026-07-10", Some("first"));
        write(
            vault.path(),
            "Ops",
            "n_bbbbbb",
            "2026-07-11",
            Some("second"),
        );
        note::write_note(
            vault.path(),
            &note_in(INBOX, "n_cccccc", "2026-07-12", NoteType::Note),
            Some("inbox idea"),
        )
        .unwrap();

        let hit = find_note(vault.path(), "Ops", &NoteId::parse("n_aaaaaa").unwrap()).unwrap();
        assert_eq!(hit.unwrap().title, "first");
        let inboxed = find_note(vault.path(), INBOX, &NoteId::parse("n_cccccc").unwrap()).unwrap();
        assert_eq!(inboxed.unwrap().title, "inbox idea");
        let miss = find_note(vault.path(), "Ops", &NoteId::parse("n_zzzzzz").unwrap()).unwrap();
        assert!(miss.is_none());
    }

    #[test]
    fn find_note_errors_on_duplicate_ids_instead_of_first_match() {
        let vault = tempdir().unwrap();
        // Two files carrying the same id — what an Explorer copy-paste of a
        // note produces. A silent first-match would let a save overwrite a
        // file the user never had on screen.
        write(
            vault.path(),
            "Ops",
            "n_aaaaaa",
            "2026-07-10",
            Some("original"),
        );
        write(vault.path(), "Ops", "n_aaaaaa", "2026-07-10", Some("copy"));

        let result = find_note(vault.path(), "Ops", &NoteId::parse("n_aaaaaa").unwrap());
        assert!(matches!(
            result,
            Err(NoteError::DuplicateNoteId { ref id, ref paths })
                if id == "n_aaaaaa" && paths.len() == 2
        ));
    }

    // --- canonicalize_project ----------------------------------------------

    #[test]
    fn canonicalize_project_adopts_existing_folder_casing_per_segment() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Growth/Q3", "n_aaaaaa", "2026-07-10", None);

        assert_eq!(
            canonicalize_project(vault.path(), "growth/q3").unwrap(),
            "Growth/Q3"
        );
        // A new child under an existing parent: parent casing adopted, new
        // segment kept verbatim.
        assert_eq!(
            canonicalize_project(vault.path(), "GROWTH/Q4").unwrap(),
            "Growth/Q4"
        );
        // Nothing on disk: the typed slug passes through.
        assert_eq!(
            canonicalize_project(vault.path(), "Brand New").unwrap(),
            "Brand New"
        );
        assert_eq!(canonicalize_project(vault.path(), INBOX).unwrap(), INBOX);
        // Still validating: reserved and malformed slugs are rejected here too.
        assert!(canonicalize_project(vault.path(), "sessions").is_err());
        assert!(canonicalize_project(vault.path(), "a//b").is_err());
    }

    // --- save_note_edit -----------------------------------------------------

    #[test]
    fn save_note_edit_merges_in_place_and_preserves_identity() {
        let vault = tempdir().unwrap();
        let path = write(vault.path(), "Ops", "n_aaaaaa", "2026-07-10", Some("keep"));
        let id = NoteId::parse("n_aaaaaa").unwrap();

        let edit = NoteEdit {
            note_type: NoteType::Meeting,
            title: None,
            date: "2026-07-12".to_string(),
            tags: vec![Tag::parse("follow-up").unwrap()],
            body: "Rewritten.".to_string(),
        };
        let saved = save_note_edit(vault.path(), "Ops", &id, edit)
            .unwrap()
            .expect("note exists");

        // Same file, same identity, edited fields replaced.
        assert_eq!(saved.path, path);
        assert_eq!(saved.note.id, id);
        assert_eq!(saved.note.body, "Rewritten.");
        let reread = find_note(vault.path(), "Ops", &id).unwrap().unwrap();
        assert_eq!(reread.note, saved.note);

        let missing = save_note_edit(
            vault.path(),
            "Ops",
            &NoteId::parse("n_zzzzzz").unwrap(),
            NoteEdit {
                note_type: NoteType::Note,
                title: None,
                date: "2026-07-12".to_string(),
                tags: vec![],
                body: String::new(),
            },
        )
        .unwrap();
        assert!(missing.is_none());
    }

    #[test]
    fn save_note_edit_retitles_and_echoes_the_new_effective_title() {
        let vault = tempdir().unwrap();
        let path = write(vault.path(), "Ops", "n_aaaaaa", "2026-07-10", Some("keep"));
        let id = NoteId::parse("n_aaaaaa").unwrap();

        let saved = save_note_edit(
            vault.path(),
            "Ops",
            &id,
            NoteEdit {
                note_type: NoteType::Note,
                title: Some("Renamed Meeting".to_string()),
                date: "2026-07-10".to_string(),
                tags: vec![Tag::parse("fixture").unwrap()],
                body: "Body.".to_string(),
            },
        )
        .unwrap()
        .expect("note exists");

        // The regression this pins: the echoed title used to be the pre-edit
        // one, so the index and the UI disagreed with the file on disk.
        assert_eq!(saved.title, "Renamed Meeting");
        assert_eq!(saved.note.title.as_deref(), Some("Renamed Meeting"));

        // The filename does not follow the title.
        assert_eq!(saved.path, path);
        assert_eq!(path.file_name().unwrap(), "keep.md");
        let reread = find_note(vault.path(), "Ops", &id).unwrap().unwrap();
        assert_eq!(reread.note.title.as_deref(), Some("Renamed Meeting"));
        assert_eq!(reread.title, "Renamed Meeting");
    }

    #[test]
    fn save_note_edit_adds_the_title_key_to_a_note_that_lacked_one() {
        // The fixture writer names the file but never writes a `title` key, so
        // this note displays its de-slugged stem — the edge case the editor has
        // to handle.
        let vault = tempdir().unwrap();
        write(
            vault.path(),
            "Ops",
            "n_aaaaaa",
            "2026-07-10",
            Some("weekly sync"),
        );
        let id = NoteId::parse("n_aaaaaa").unwrap();

        let untouched = save_note_edit(
            vault.path(),
            "Ops",
            &id,
            NoteEdit {
                note_type: NoteType::Note,
                title: None,
                date: "2026-07-10".to_string(),
                tags: vec![Tag::parse("fixture").unwrap()],
                body: "Body.".to_string(),
            },
        )
        .unwrap()
        .expect("note exists");
        // No title sent: the key stays absent and display still de-slugs.
        assert_eq!(untouched.note.title, None);
        assert_eq!(untouched.title, "weekly sync");

        let retitled = save_note_edit(
            vault.path(),
            "Ops",
            &id,
            NoteEdit {
                note_type: NoteType::Note,
                title: Some("Weekly Sync".to_string()),
                date: "2026-07-10".to_string(),
                tags: vec![Tag::parse("fixture").unwrap()],
                body: "Body.".to_string(),
            },
        )
        .unwrap()
        .expect("note exists");
        assert_eq!(retitled.note.title.as_deref(), Some("Weekly Sync"));
        assert_eq!(retitled.title, "Weekly Sync");
    }

    // --- file_note_to_project (re-route) -----------------------------------

    /// A routed note in `project` with an explicit confidence (unlike `note_in`,
    /// which files a non-Inbox note as `Manual`).
    fn routed_note_in(project: &str, id: &str, date: &str, confidence: f64) -> Note {
        Note::new(
            NoteId::parse(id).unwrap(),
            NoteType::Note,
            Routing::Routed {
                project: project.to_string(),
                confidence,
            },
            date,
            vec![Tag::parse("fixture").unwrap()],
            Source::parse("manual").unwrap(),
            "Body of the note to be re-routed.",
        )
        .unwrap()
    }

    /// Writes an Inbox note and returns its path.
    fn write_inbox(vault: &Path, id: &str, title: &str) -> PathBuf {
        note::write_note(
            vault,
            &note_in(INBOX, id, "2026-07-10", NoteType::Note),
            Some(title),
        )
        .unwrap()
    }

    /// Re-parses the note file at `path`.
    fn read_note_at(path: &Path) -> Note {
        Note::from_markdown(&fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn find_note_anywhere_locates_in_inbox_and_projects_and_misses_unknown() {
        let vault = tempdir().unwrap();
        write_inbox(vault.path(), "n_aaaaaa", "unfiled");
        write(vault.path(), "Ops", "n_bbbbbb", "2026-07-11", Some("filed"));

        let (proj, listed) = find_note_anywhere(vault.path(), &NoteId::parse("n_aaaaaa").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(proj, INBOX);
        assert_eq!(listed.title, "unfiled");

        let (proj, _) = find_note_anywhere(vault.path(), &NoteId::parse("n_bbbbbb").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(proj, "Ops");

        assert!(
            find_note_anywhere(vault.path(), &NoteId::parse("n_zzzzzz").unwrap())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn find_note_anywhere_errors_on_cross_project_duplicate_id() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Ops", "n_aaaaaa", "2026-07-10", Some("here"));
        write(
            vault.path(),
            "Growth",
            "n_aaaaaa",
            "2026-07-11",
            Some("there"),
        );

        let result = find_note_anywhere(vault.path(), &NoteId::parse("n_aaaaaa").unwrap());
        assert!(matches!(
            result,
            Err(NoteError::DuplicateNoteId { ref paths, .. }) if paths.len() == 2
        ));
    }

    #[test]
    fn reroute_from_inbox_moves_file_updates_frontmatter_and_logs_example() {
        let vault = tempdir().unwrap();
        // A real target folder so create_project isn't needed.
        write(
            vault.path(),
            "Ops",
            "n_existing",
            "2026-07-01",
            Some("seed"),
        );
        let old_path = write_inbox(vault.path(), "n_aaaaaa", "Bright Idea");

        let routed = file_note_to_project(
            vault.path(),
            &NoteId::parse("n_aaaaaa").unwrap(),
            "Ops",
            &FileNoteOptions::default(),
        )
        .unwrap()
        .expect("note exists");

        // Moved, stem preserved, previous location reported.
        assert!(routed.moved);
        assert_eq!(routed.previous_project, None);
        assert_eq!(routed.previous_path, old_path);
        assert_eq!(
            routed.note.path,
            vault.path().join("Ops").join("bright-idea.md")
        );
        assert!(!old_path.exists());

        // Frontmatter: new project + 1.0 confidence, id preserved.
        let on_disk = read_note_at(&routed.note.path);
        assert_eq!(
            on_disk.routing,
            Routing::Routed {
                project: "Ops".to_string(),
                confidence: 1.0,
            }
        );
        assert_eq!(on_disk.id, NoteId::parse("n_aaaaaa").unwrap());

        // Correction logged in the target, previous_project null (was Inbox).
        let log = RoutingExamples::load(&vault.path().join("Ops")).unwrap();
        assert_eq!(log.examples().len(), 1);
        assert_eq!(log.examples()[0].note_id, "n_aaaaaa");
        assert_eq!(log.examples()[0].previous_project, None);
        assert_eq!(log.examples()[0].confidence, 1.0);
    }

    #[test]
    fn reroute_to_same_project_rewrites_in_place_moved_false() {
        let vault = tempdir().unwrap();
        let path = note::write_note(
            vault.path(),
            &routed_note_in("Ops", "n_aaaaaa", "2026-07-10", 0.7),
            Some("keep"),
        )
        .unwrap();

        let routed = file_note_to_project(
            vault.path(),
            &NoteId::parse("n_aaaaaa").unwrap(),
            "Ops",
            &FileNoteOptions::default(),
        )
        .unwrap()
        .unwrap();

        assert!(!routed.moved);
        assert_eq!(routed.note.path, path);
        assert_eq!(routed.previous_project, Some("Ops".to_string()));
        // Confidence stamped to 1.0 in place.
        assert_eq!(read_note_at(&path).routing.confidence(), Some(1.0));
    }

    #[test]
    fn confidence_override_is_written_and_invalid_is_rejected() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Ops", "n_seedaa", "2026-07-01", Some("seed"));
        write_inbox(vault.path(), "n_aaaaaa", "idea");

        let routed = file_note_to_project(
            vault.path(),
            &NoteId::parse("n_aaaaaa").unwrap(),
            "Ops",
            &FileNoteOptions {
                confidence: Some(0.42),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            read_note_at(&routed.note.path).routing.confidence(),
            Some(0.42)
        );

        write_inbox(vault.path(), "n_bbbbbb", "other");
        let bad = file_note_to_project(
            vault.path(),
            &NoteId::parse("n_bbbbbb").unwrap(),
            "Ops",
            &FileNoteOptions {
                confidence: Some(1.5),
                ..Default::default()
            },
        );
        assert!(matches!(
            bad,
            Err(NoteError::InvalidField {
                field: "confidence",
                ..
            })
        ));
    }

    #[test]
    fn missing_target_without_create_errors_and_creates_nothing() {
        let vault = tempdir().unwrap();
        write_inbox(vault.path(), "n_aaaaaa", "idea");

        let result = file_note_to_project(
            vault.path(),
            &NoteId::parse("n_aaaaaa").unwrap(),
            "Brand New",
            &FileNoteOptions::default(),
        );
        assert!(matches!(
            result,
            Err(NoteError::MissingProject { ref project }) if project == "Brand New"
        ));
        assert!(!vault.path().join("Brand New").exists());
        // The note stayed put.
        assert!(vault.path().join("Inbox").join("idea.md").exists());
    }

    #[test]
    fn create_project_true_creates_the_folder_and_files_the_note() {
        let vault = tempdir().unwrap();
        write_inbox(vault.path(), "n_aaaaaa", "idea");

        let routed = file_note_to_project(
            vault.path(),
            &NoteId::parse("n_aaaaaa").unwrap(),
            "Growth/Q4",
            &FileNoteOptions {
                create_project: true,
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();

        assert!(routed.moved);
        assert_eq!(
            routed.note.path,
            vault.path().join("Growth").join("Q4").join("idea.md")
        );
        assert_eq!(
            read_note_at(&routed.note.path).routing.project(),
            "Growth/Q4"
        );
    }

    #[test]
    fn target_inbox_is_rejected_in_any_casing() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Ops", "n_aaaaaa", "2026-07-10", Some("filed"));
        for target in ["Inbox", "inbox", "INBOX", "Inbox/x"] {
            let result = file_note_to_project(
                vault.path(),
                &NoteId::parse("n_aaaaaa").unwrap(),
                target,
                &FileNoteOptions::default(),
            );
            assert!(result.is_err(), "target {target:?} should be rejected");
        }
    }

    #[test]
    fn reason_over_the_cap_is_rejected() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Ops", "n_seedaa", "2026-07-01", Some("seed"));
        write_inbox(vault.path(), "n_aaaaaa", "idea");

        let result = file_note_to_project(
            vault.path(),
            &NoteId::parse("n_aaaaaa").unwrap(),
            "Ops",
            &FileNoteOptions {
                reason: Some("x".repeat(REASON_MAX_CHARS + 1)),
                ..Default::default()
            },
        );
        assert!(matches!(
            result,
            Err(NoteError::InvalidField {
                field: "reason",
                ..
            })
        ));
    }

    #[test]
    fn reason_is_stored_and_overwritten_on_a_second_correction() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Ops", "n_seedaa", "2026-07-01", Some("seed"));
        write_inbox(vault.path(), "n_aaaaaa", "idea");
        let id = NoteId::parse("n_aaaaaa").unwrap();

        file_note_to_project(
            vault.path(),
            &id,
            "Ops",
            &FileNoteOptions {
                reason: Some("first pass".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        // Re-file to the same project with a new reason → the single entry updates.
        file_note_to_project(
            vault.path(),
            &id,
            "Ops",
            &FileNoteOptions {
                reason: Some("clearly operations".to_string()),
                ..Default::default()
            },
        )
        .unwrap();

        let log = RoutingExamples::load(&vault.path().join("Ops")).unwrap();
        assert_eq!(log.examples().len(), 1);
        assert_eq!(
            log.examples()[0].reason.as_deref(),
            Some("clearly operations")
        );
    }

    #[test]
    fn a_second_reroute_moves_the_example_between_project_logs() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Ops", "n_seedaa", "2026-07-01", Some("seed"));
        write(
            vault.path(),
            "Growth",
            "n_seedbb",
            "2026-07-01",
            Some("seed"),
        );
        write_inbox(vault.path(), "n_aaaaaa", "idea");
        let id = NoteId::parse("n_aaaaaa").unwrap();

        // Inbox → Ops, then Ops → Growth.
        file_note_to_project(vault.path(), &id, "Ops", &FileNoteOptions::default()).unwrap();
        file_note_to_project(vault.path(), &id, "Growth", &FileNoteOptions::default()).unwrap();

        // Ops's log no longer holds the note; Growth's does, from Ops.
        let ops_log = RoutingExamples::load(&vault.path().join("Ops")).unwrap();
        assert!(ops_log.examples().iter().all(|e| e.note_id != "n_aaaaaa"));
        let growth_log = RoutingExamples::load(&vault.path().join("Growth")).unwrap();
        let entry = growth_log
            .examples()
            .iter()
            .find(|e| e.note_id == "n_aaaaaa")
            .expect("moved into Growth's log");
        assert_eq!(entry.previous_project.as_deref(), Some("Ops"));
    }

    #[test]
    fn target_casing_is_canonicalized_to_the_existing_folder() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Ops", "n_seedaa", "2026-07-01", Some("seed"));
        write_inbox(vault.path(), "n_aaaaaa", "idea");

        // Target typed lowercase; the existing `Ops/` casing must win.
        let routed = file_note_to_project(
            vault.path(),
            &NoteId::parse("n_aaaaaa").unwrap(),
            "ops",
            &FileNoteOptions::default(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(read_note_at(&routed.note.path).routing.project(), "Ops");
    }

    #[test]
    fn reroute_of_unknown_id_is_ok_none() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Ops", "n_seedaa", "2026-07-01", Some("seed"));
        let missing = file_note_to_project(
            vault.path(),
            &NoteId::parse("n_zzzzzz").unwrap(),
            "Ops",
            &FileNoteOptions::default(),
        )
        .unwrap();
        assert!(missing.is_none());
    }

    // --- list_projects ------------------------------------------------------

    #[test]
    fn list_projects_discovers_nested_dirs_with_counts_and_parent_chain() {
        let vault = tempdir().unwrap();
        note::write_note(
            vault.path(),
            &note_in("Growth/Q3", "n_aaaaaa", "2026-07-10", NoteType::Meeting),
            Some("kickoff"),
        )
        .unwrap();
        note::write_note(
            vault.path(),
            &note_in("Growth/Q3", "n_bbbbbb", "2026-07-11", NoteType::Note),
            Some("follow up"),
        )
        .unwrap();

        let projects = list_projects(vault.path()).unwrap();
        let slugs: Vec<&str> = projects.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(slugs, ["Growth", "Growth/Q3"]);

        let growth = &projects[0];
        assert_eq!((growth.note_count, growth.meeting_count), (0, 0));
        assert!(growth.last_activity.is_none());

        let q3 = &projects[1];
        assert_eq!((q3.note_count, q3.meeting_count), (2, 1));
        assert!(q3.last_activity.is_some());
    }

    #[test]
    fn list_projects_excludes_reserved_and_hidden_but_lists_note_free_dirs() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Ops", "n_aaaaaa", "2026-07-10", Some("real"));
        note::write_note(
            vault.path(),
            &note_in(INBOX, "n_bbbbbb", "2026-07-11", NoteType::Note),
            Some("unfiled"),
        )
        .unwrap();
        // The decoys are fully valid note files, so exclusion is proven by the
        // name rules alone, not by a parse failure.
        let decoy = note_in("Ops", "n_zzzzzz", "2026-07-01", NoteType::Note).to_markdown();
        for reserved in [
            "sessions",
            "raw",
            "chats",
            "EBWebView",
            ".obsidian",
            "_scratch",
        ] {
            let dir = vault.path().join(reserved);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("decoy.md"), &decoy).unwrap();
        }
        // A note-free dir with a legal name IS a project (existence is the
        // rule) — an empty project must be enumerable to be filed into.
        fs::create_dir(vault.path().join("Empty")).unwrap();

        let projects = list_projects(vault.path()).unwrap();
        let slugs: Vec<&str> = projects.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(slugs, ["Empty", "Ops"]);

        let empty = &projects[0];
        assert_eq!((empty.note_count, empty.meeting_count), (0, 0));
        assert!(empty.last_activity.is_none());
    }

    #[test]
    fn list_projects_missing_root_yields_empty() {
        let vault = tempdir().unwrap();
        let missing = vault.path().join("never-created");
        assert!(list_projects(&missing).unwrap().is_empty());
    }

    // --- create_project -----------------------------------------------------

    #[test]
    fn create_project_creates_an_empty_listed_project() {
        let vault = tempdir().unwrap();

        let info = create_project(vault.path(), "Ops").unwrap();
        assert_eq!(info.slug, "Ops");
        assert_eq!((info.note_count, info.meeting_count), (0, 0));
        assert!(info.last_activity.is_none());

        assert!(vault.path().join("Ops").is_dir());
        let projects = list_projects(vault.path()).unwrap();
        let slugs: Vec<&str> = projects.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(slugs, ["Ops"]);
    }

    #[test]
    fn create_project_nested_slug_creates_parents() {
        let vault = tempdir().unwrap();

        let info = create_project(vault.path(), "Growth/Q3").unwrap();
        assert_eq!(info.slug, "Growth/Q3");

        let projects = list_projects(vault.path()).unwrap();
        let slugs: Vec<&str> = projects.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(slugs, ["Growth", "Growth/Q3"]);
    }

    #[test]
    fn create_project_adopts_on_disk_casing() {
        let vault = tempdir().unwrap();
        create_project(vault.path(), "Growth").unwrap();

        let info = create_project(vault.path(), "growth/Q3").unwrap();
        assert_eq!(info.slug, "Growth/Q3");
        assert!(vault.path().join("Growth").join("Q3").is_dir());
    }

    #[test]
    fn create_project_rejects_reserved_hidden_and_invalid_names() {
        let vault = tempdir().unwrap();
        for name in [
            "Inbox",
            "inbox",
            "Inbox/x",
            "sessions",
            "raw",
            "chats",
            "EBWebView",
            "_x",
            ".x",
            "con",
            "a:b",
        ] {
            let result = create_project(vault.path(), name);
            assert!(
                matches!(result, Err(NoteError::InvalidField { .. })),
                "{name:?} should be rejected"
            );
        }
        // Nothing was created by any rejected attempt.
        assert_eq!(fs::read_dir(vault.path()).unwrap().count(), 0);
    }

    #[test]
    fn create_project_duplicate_in_any_casing_is_project_exists() {
        let vault = tempdir().unwrap();
        create_project(vault.path(), "Ops").unwrap();

        let exact = create_project(vault.path(), "Ops");
        assert!(matches!(
            exact,
            Err(NoteError::ProjectExists { project }) if project == "Ops"
        ));
        // A differently-cased duplicate reports the canonical on-disk slug.
        let cased = create_project(vault.path(), "ops");
        assert!(matches!(
            cased,
            Err(NoteError::ProjectExists { project }) if project == "Ops"
        ));
    }

    // --- delete_project -----------------------------------------------------

    #[test]
    fn delete_project_removes_an_empty_project() {
        let vault = tempdir().unwrap();
        create_project(vault.path(), "Ops").unwrap();

        let deleted = delete_project(vault.path(), "Ops").unwrap();
        assert_eq!(deleted.slug, "Ops");
        assert!(deleted.moved_notes.is_empty());
        assert!(!vault.path().join("Ops").exists());
        assert!(list_projects(vault.path()).unwrap().is_empty());
    }

    #[test]
    fn delete_project_moves_all_notes_to_inbox_and_removes_tree() {
        let vault = tempdir().unwrap();
        write(
            vault.path(),
            "Growth",
            "n_aaaaaa",
            "2026-07-10",
            Some("plan"),
        );
        write(
            vault.path(),
            "Growth/Q4",
            "n_bbbbbb",
            "2026-07-11",
            Some("kickoff"),
        );

        let deleted = delete_project(vault.path(), "Growth").unwrap();
        assert_eq!(deleted.slug, "Growth");
        assert_eq!(deleted.moved_notes.len(), 2);
        assert!(!vault.path().join("Growth").exists());

        // Both notes landed in the Inbox, stems intact, every field preserved
        // but the routing, which is the zero-evidence Inbox landing.
        for (stem, id) in [("plan", "n_aaaaaa"), ("kickoff", "n_bbbbbb")] {
            let path = vault.path().join(INBOX).join(format!("{stem}.md"));
            let note = read_note_at(&path);
            assert_eq!(note.id.as_str(), id);
            assert_eq!(
                note.routing,
                Routing::Routed {
                    project: INBOX.to_string(),
                    confidence: 0.0,
                }
            );
            assert_eq!(note.body, "Body.");
            assert!(deleted.moved_notes.iter().any(|listed| listed.path == path));
        }
    }

    #[test]
    fn delete_project_disambiguates_inbox_stem_collisions() {
        let vault = tempdir().unwrap();
        write_inbox(vault.path(), "n_aaaaaa", "kickoff");
        write(
            vault.path(),
            "Growth",
            "n_bbbbbb",
            "2026-07-11",
            Some("kickoff"),
        );

        delete_project(vault.path(), "Growth").unwrap();

        let original = read_note_at(&vault.path().join(INBOX).join("kickoff.md"));
        let moved = read_note_at(&vault.path().join(INBOX).join("kickoff-2.md"));
        assert_eq!(original.id.as_str(), "n_aaaaaa");
        assert_eq!(moved.id.as_str(), "n_bbbbbb");
    }

    #[test]
    fn delete_project_removes_infra_files_with_the_folder() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Ops", "n_aaaaaa", "2026-07-10", Some("real"));
        let dir = vault.path().join("Ops");
        fs::write(dir.join(glossary::GLOSSARY_FILE), "terms: []\n").unwrap();
        fs::write(
            dir.join(routing_examples::ROUTING_EXAMPLES_FILE),
            "examples: []\n",
        )
        .unwrap();
        fs::write(dir.join(".note.123.0.tmp"), "half-written scratch").unwrap();

        delete_project(vault.path(), "Ops").unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn delete_project_blocks_on_unmanaged_files_without_touching_anything() {
        let vault = tempdir().unwrap();
        let note_path = write(vault.path(), "Ops", "n_aaaaaa", "2026-07-10", Some("real"));
        let dir = vault.path().join("Ops");
        fs::write(dir.join("photo.png"), [0u8; 4]).unwrap();
        fs::write(dir.join("broken.md"), "no frontmatter here").unwrap();

        let result = delete_project(vault.path(), "Ops");
        assert!(matches!(result, Err(NoteError::InvalidField { .. })));

        // Nothing moved, nothing deleted: the guard fires before any mutation.
        assert!(note_path.is_file());
        assert!(dir.join("photo.png").is_file());
        assert!(!vault.path().join(INBOX).exists());
    }

    #[test]
    fn delete_project_blocks_on_hidden_subdirectories() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Ops", "n_aaaaaa", "2026-07-10", Some("real"));
        fs::create_dir(vault.path().join("Ops").join(".obsidian")).unwrap();

        let result = delete_project(vault.path(), "Ops");
        assert!(matches!(result, Err(NoteError::InvalidField { .. })));
        assert!(vault.path().join("Ops").is_dir());
    }

    #[test]
    fn delete_project_rejects_inbox_and_missing_project() {
        let vault = tempdir().unwrap();
        assert!(matches!(
            delete_project(vault.path(), INBOX),
            Err(NoteError::InvalidField { .. })
        ));
        assert!(matches!(
            delete_project(vault.path(), "inbox"),
            Err(NoteError::InvalidField { .. })
        ));
        assert!(matches!(
            delete_project(vault.path(), "Nope"),
            Err(NoteError::MissingProject { project }) if project == "Nope"
        ));
    }

    // --- delete_note --------------------------------------------------------

    /// Builds a note with an explicit `source:` value (the `note_in` helper
    /// hard-codes `manual`), for the session-pairing assertions.
    fn note_with_source(project: &str, id: &str, source: &str) -> Note {
        Note::new(
            NoteId::parse(id).unwrap(),
            NoteType::Meeting,
            Routing::Manual {
                project: project.to_string(),
            },
            "2026-07-12",
            Vec::new(),
            Source::parse(source).unwrap(),
            "Body.",
        )
        .unwrap()
    }

    #[test]
    fn delete_note_removes_an_inbox_note() {
        let vault = tempdir().unwrap();
        let path = write_inbox(vault.path(), "n_aaaaaa", "unfiled");

        let deleted = delete_note(vault.path(), &NoteId::parse("n_aaaaaa").unwrap())
            .unwrap()
            .expect("the note exists");

        assert_eq!(deleted.id.as_str(), "n_aaaaaa");
        assert_eq!(deleted.former_project, None); // Inbox → None
        assert_eq!(deleted.title, "unfiled");
        assert_eq!(deleted.path, path);
        assert!(!path.exists());
    }

    #[test]
    fn delete_note_removes_a_filed_note_and_reports_its_project() {
        let vault = tempdir().unwrap();
        let path = write(vault.path(), "Ops", "n_bbbbbb", "2026-07-11", Some("filed"));

        let deleted = delete_note(vault.path(), &NoteId::parse("n_bbbbbb").unwrap())
            .unwrap()
            .expect("the note exists");

        assert_eq!(deleted.former_project.as_deref(), Some("Ops"));
        assert_eq!(deleted.title, "filed");
        assert!(!path.exists());
    }

    #[test]
    fn delete_note_of_an_unknown_id_is_none() {
        let vault = tempdir().unwrap();
        write_inbox(vault.path(), "n_aaaaaa", "kept");

        assert!(
            delete_note(vault.path(), &NoteId::parse("n_zzzzzz").unwrap())
                .unwrap()
                .is_none()
        );
        // The unrelated note is untouched.
        assert!(vault.path().join(INBOX).join("kept.md").is_file());
    }

    #[test]
    fn delete_note_refuses_a_duplicate_id_without_removing_either() {
        let vault = tempdir().unwrap();
        // The same id in two folders — the vault-wide duplicate guard must fire
        // and delete nothing rather than nuke an ambiguous pair.
        let inbox_path = write_inbox(vault.path(), "n_aaaaaa", "one");
        let ops_path = write(vault.path(), "Ops", "n_aaaaaa", "2026-07-11", Some("two"));

        let result = delete_note(vault.path(), &NoteId::parse("n_aaaaaa").unwrap());
        assert!(matches!(result, Err(NoteError::DuplicateNoteId { .. })));
        assert!(inbox_path.is_file());
        assert!(ops_path.is_file());
    }

    #[test]
    fn delete_note_carries_the_notes_source() {
        let vault = tempdir().unwrap();
        // A distilled note pairs with a raw session artifact.
        let session_source = "sessions/20260712T140335123Z-k4m2xp7q.jsonl";
        note::write_note(
            vault.path(),
            &note_with_source("Ops", "n_cccccc", session_source),
            Some("distilled"),
        )
        .unwrap();
        // A manual note carries a capture keyword instead.
        write_inbox(vault.path(), "n_dddddd", "typed");

        let distilled = delete_note(vault.path(), &NoteId::parse("n_cccccc").unwrap())
            .unwrap()
            .expect("the note exists");
        assert_eq!(distilled.source.as_yaml(), session_source);

        let manual = delete_note(vault.path(), &NoteId::parse("n_dddddd").unwrap())
            .unwrap()
            .expect("the note exists");
        assert_eq!(manual.source.as_yaml(), "manual");
    }

    #[test]
    fn delete_note_leaves_sibling_notes_untouched() {
        let vault = tempdir().unwrap();
        write(
            vault.path(),
            "Ops",
            "n_aaaaaa",
            "2026-07-10",
            Some("target"),
        );
        let kept = write(
            vault.path(),
            "Ops",
            "n_bbbbbb",
            "2026-07-11",
            Some("sibling"),
        );

        delete_note(vault.path(), &NoteId::parse("n_aaaaaa").unwrap())
            .unwrap()
            .expect("the note exists");

        assert!(kept.is_file());
        let scan = scan_project_notes(vault.path(), "Ops").unwrap();
        assert_eq!(scan.notes.len(), 1);
        assert_eq!(scan.notes[0].note.id.as_str(), "n_bbbbbb");
    }

    // --- list_projects_page -----------------------------------------------

    fn write_typed(
        vault: &Path,
        project: &str,
        id: &str,
        date: &str,
        note_type: NoteType,
    ) -> PathBuf {
        note::write_note(vault, &note_in(project, id, date, note_type), None).unwrap()
    }

    fn base_query() -> ProjectQuery {
        ProjectQuery {
            parent: None,
            include_descendants: true,
            include_empty: true,
            sort: ProjectSort::Name,
            limit: 100,
            cursor: None,
        }
    }

    fn slugs(page: &ProjectPage) -> Vec<String> {
        page.projects.iter().map(|p| p.slug.clone()).collect()
    }

    /// Matches the `Project.id` pattern `^p_[0-9a-z]{6,}$` without a regex dep.
    fn id_is_wellformed(id: &str) -> bool {
        id.strip_prefix("p_").is_some_and(|rest| {
            rest.len() >= 6
                && rest
                    .chars()
                    .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
        })
    }

    #[test]
    fn list_projects_page_synthesizes_id_display_name_parent_and_counts() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Growth/Q3", "n_a00001", "2026-07-10", None);
        write_typed(
            vault.path(),
            "Growth/Q3",
            "n_a00002",
            "2026-07-11",
            NoteType::Meeting,
        );

        let page = list_projects_page(vault.path(), &base_query()).unwrap();
        let q3 = page
            .projects
            .iter()
            .find(|p| p.slug == "Growth/Q3")
            .expect("Growth/Q3 is listed");

        assert_eq!(q3.display_name, "Q3");
        assert_eq!(q3.parent.as_deref(), Some("Growth"));
        assert_eq!(q3.note_count, 2);
        assert_eq!(q3.meeting_count, 1);
        assert!(q3.last_activity.is_some());
        assert!(
            id_is_wellformed(&q3.id),
            "id {:?} must match ^p_[0-9a-z]{{6,}}$",
            q3.id
        );

        // Deterministic across runs.
        let again = list_projects_page(vault.path(), &base_query()).unwrap();
        let q3_again = again
            .projects
            .iter()
            .find(|p| p.slug == "Growth/Q3")
            .unwrap();
        assert_eq!(q3.id, q3_again.id);
    }

    #[test]
    fn list_projects_page_parent_filter_excludes_siblings_and_the_parent_itself() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Growth", "n_a00001", "2026-07-10", None);
        write(vault.path(), "Growth/Q3", "n_a00002", "2026-07-10", None);
        write(vault.path(), "Growthx", "n_a00003", "2026-07-10", None);

        let query = ProjectQuery {
            parent: Some("Growth".to_string()),
            ..base_query()
        };
        let page = list_projects_page(vault.path(), &query).unwrap();
        // Only the true descendant — never the `Growthx` sibling, never `Growth`.
        assert_eq!(slugs(&page), vec!["Growth/Q3".to_string()]);
    }

    #[test]
    fn list_projects_page_direct_children_only_when_descendants_off() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Growth/Q3", "n_a00001", "2026-07-10", None);
        write(
            vault.path(),
            "Growth/Q3/Week1",
            "n_a00002",
            "2026-07-10",
            None,
        );

        let query = ProjectQuery {
            parent: Some("Growth".to_string()),
            include_descendants: false,
            ..base_query()
        };
        let page = list_projects_page(vault.path(), &query).unwrap();
        assert_eq!(slugs(&page), vec!["Growth/Q3".to_string()]);
    }

    #[test]
    fn list_projects_page_top_level_only_when_descendants_off_and_no_parent() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Alpha", "n_a00001", "2026-07-10", None);
        write(vault.path(), "Alpha/Nested", "n_a00002", "2026-07-10", None);
        write(vault.path(), "Beta", "n_a00003", "2026-07-10", None);

        let query = ProjectQuery {
            include_descendants: false,
            ..base_query()
        };
        let page = list_projects_page(vault.path(), &query).unwrap();
        assert_eq!(slugs(&page), vec!["Alpha".to_string(), "Beta".to_string()]);
    }

    #[test]
    fn list_projects_page_include_empty_toggle() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Ops", "n_a00001", "2026-07-10", None);
        fs::create_dir(vault.path().join("Empty")).unwrap();

        let with_empty = list_projects_page(vault.path(), &base_query()).unwrap();
        assert_eq!(
            slugs(&with_empty),
            vec!["Empty".to_string(), "Ops".to_string()]
        );

        let query = ProjectQuery {
            include_empty: false,
            ..base_query()
        };
        let without_empty = list_projects_page(vault.path(), &query).unwrap();
        assert_eq!(slugs(&without_empty), vec!["Ops".to_string()]);
    }

    #[test]
    fn list_projects_page_sorts_by_name_and_note_count() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Beta", "n_a00001", "2026-07-10", None);
        write(vault.path(), "Beta", "n_a00002", "2026-07-10", None);
        write(vault.path(), "Alpha", "n_a00003", "2026-07-10", None);

        let by_name = list_projects_page(vault.path(), &base_query()).unwrap();
        assert_eq!(
            slugs(&by_name),
            vec!["Alpha".to_string(), "Beta".to_string()]
        );

        let by_count = list_projects_page(
            vault.path(),
            &ProjectQuery {
                sort: ProjectSort::NoteCount,
                ..base_query()
            },
        )
        .unwrap();
        // Beta (2 notes) outranks Alpha (1 note).
        assert_eq!(by_count.projects[0].slug, "Beta");
        assert_eq!(by_count.projects[1].slug, "Alpha");
    }

    #[test]
    fn list_projects_page_last_activity_sort_is_non_increasing_with_empty_last() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Active", "n_a00001", "2026-07-10", None);
        write(vault.path(), "AlsoActive", "n_a00002", "2026-07-10", None);
        fs::create_dir(vault.path().join("Empty")).unwrap();

        let page = list_projects_page(
            vault.path(),
            &ProjectQuery {
                sort: ProjectSort::LastActivity,
                ..base_query()
            },
        )
        .unwrap();

        // Whatever the filesystem mtimes are, the order is non-increasing and a
        // never-active project (last_activity: None) sorts last.
        for pair in page.projects.windows(2) {
            assert!(
                pair[0].last_activity >= pair[1].last_activity,
                "expected non-increasing last_activity, got {:?} then {:?}",
                pair[0].last_activity,
                pair[1].last_activity
            );
        }
        assert_eq!(page.projects.last().unwrap().slug, "Empty");
        assert!(page.projects.last().unwrap().last_activity.is_none());
    }

    #[test]
    fn list_projects_page_cursor_walks_disjoint_pages_covering_the_whole_set() {
        let vault = tempdir().unwrap();
        for (index, name) in ["A", "B", "C", "D", "E"].iter().enumerate() {
            write(
                vault.path(),
                name,
                &format!("n_a0000{index}"),
                "2026-07-10",
                None,
            );
        }

        let mut seen = Vec::new();
        let mut cursor = None;
        loop {
            let page = list_projects_page(
                vault.path(),
                &ProjectQuery {
                    limit: 2,
                    cursor: cursor.clone(),
                    ..base_query()
                },
            )
            .unwrap();
            assert_eq!(page.page.total_estimate, Some(5));
            seen.extend(slugs(&page));
            match page.page.next_cursor {
                Some(next) => {
                    assert!(page.page.has_more);
                    cursor = Some(next);
                }
                None => {
                    assert!(!page.page.has_more);
                    break;
                }
            }
        }

        assert_eq!(seen, vec!["A", "B", "C", "D", "E"]);
    }

    #[test]
    fn list_projects_page_missing_parent_is_empty_success() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Ops", "n_a00001", "2026-07-10", None);

        let page = list_projects_page(
            vault.path(),
            &ProjectQuery {
                parent: Some("Nope".to_string()),
                ..base_query()
            },
        )
        .unwrap();
        assert!(page.projects.is_empty());
        assert!(!page.page.has_more);
        assert_eq!(page.page.next_cursor, None);
        assert_eq!(page.page.total_estimate, Some(0));
    }

    #[test]
    fn list_projects_page_rejects_a_malformed_cursor() {
        let vault = tempdir().unwrap();
        let result = list_projects_page(
            vault.path(),
            &ProjectQuery {
                cursor: Some("not-a-real-cursor".to_string()),
                ..base_query()
            },
        );
        assert!(matches!(
            result,
            Err(NoteError::InvalidField {
                field: "cursor",
                ..
            })
        ));
    }

    #[test]
    fn projects_cursor_round_trips_and_rejects_garbage() {
        let encoded = encode_projects_cursor(4, "p_00000000deadbeef");
        let decoded = decode_projects_cursor(&encoded).unwrap();
        assert_eq!(decoded.served, 4);
        assert_eq!(decoded.id, "p_00000000deadbeef");

        for bad in ["", "v1", "v2:4:p_x", "v1::p_x", "v1:notnum:p_x", "v1:4:"] {
            assert!(
                decode_projects_cursor(bad).is_err(),
                "should reject {bad:?}"
            );
        }
    }

    // --- add_glossary_term -------------------------------------------------

    fn glossary_term(term: &str, definition: &str, aliases: &[&str]) -> glossary::GlossaryTerm {
        glossary::GlossaryTerm {
            term: term.to_string(),
            definition: definition.to_string(),
            aliases: aliases.iter().map(|a| a.to_string()).collect(),
        }
    }

    #[test]
    fn add_glossary_term_creates_then_updates_in_place() {
        let vault = tempdir().unwrap();
        fs::create_dir(vault.path().join("Growth")).unwrap();

        let created = add_glossary_term(
            vault.path(),
            "Growth",
            glossary_term("MERIDIAN", "A systems-migration project.", &["meridian"]),
            glossary::OnConflict::Update,
        )
        .unwrap();
        assert!(created.created);
        assert_eq!(created.project, "Growth");
        assert_eq!(created.term.term, "MERIDIAN");
        assert_eq!(created.term.aliases, vec!["meridian".to_string()]);

        // The same normalized term (different casing) updates in place.
        let updated = add_glossary_term(
            vault.path(),
            "Growth",
            glossary_term("meridian", "Updated definition.", &[]),
            glossary::OnConflict::Update,
        )
        .unwrap();
        assert!(!updated.created);
        assert_eq!(updated.term.definition, "Updated definition.");

        // One collapsed entry persisted on disk.
        let loaded = glossary::Glossary::load(&vault.path().join("Growth")).unwrap();
        assert_eq!(loaded.terms().len(), 1);
        assert_eq!(
            loaded.get("MERIDIAN").unwrap().definition,
            "Updated definition."
        );
    }

    #[test]
    fn add_glossary_term_on_conflict_error_rejects_and_preserves() {
        let vault = tempdir().unwrap();
        fs::create_dir(vault.path().join("Growth")).unwrap();
        add_glossary_term(
            vault.path(),
            "Growth",
            glossary_term("TeeTrack", "Tee-sheet vendor.", &[]),
            glossary::OnConflict::Update,
        )
        .unwrap();

        let err = add_glossary_term(
            vault.path(),
            "Growth",
            glossary_term("teetrack", "Different.", &[]),
            glossary::OnConflict::Error,
        )
        .unwrap_err();
        assert!(matches!(err, AddGlossaryTermError::Conflict { .. }));

        // The original definition is untouched.
        let loaded = glossary::Glossary::load(&vault.path().join("Growth")).unwrap();
        assert_eq!(
            loaded.get("TeeTrack").unwrap().definition,
            "Tee-sheet vendor."
        );
    }

    #[test]
    fn add_glossary_term_missing_project_is_an_error() {
        let vault = tempdir().unwrap();
        // No "Growth" folder — this tool never creates one.
        let err = add_glossary_term(
            vault.path(),
            "Growth",
            glossary_term("MERIDIAN", "x.", &[]),
            glossary::OnConflict::Update,
        )
        .unwrap_err();
        assert!(matches!(err, AddGlossaryTermError::MissingProject { .. }));
        assert!(!glossary::glossary_path(&vault.path().join("Growth")).exists());
    }

    #[test]
    fn add_glossary_term_adopts_on_disk_project_casing() {
        let vault = tempdir().unwrap();
        fs::create_dir(vault.path().join("Growth")).unwrap();

        let outcome = add_glossary_term(
            vault.path(),
            "growth", // lower-case request against the existing `Growth/`
            glossary_term("MERIDIAN", "x.", &[]),
            glossary::OnConflict::Update,
        )
        .unwrap();
        assert_eq!(outcome.project, "Growth");
        // The term landed in the existing folder, not a new lower-case one.
        assert!(glossary::glossary_path(&vault.path().join("Growth")).exists());
    }

    #[test]
    fn add_glossary_term_rejects_blank_and_oversized_fields() {
        let vault = tempdir().unwrap();
        fs::create_dir(vault.path().join("Growth")).unwrap();

        let blank = add_glossary_term(
            vault.path(),
            "Growth",
            glossary_term("   ", "def", &[]),
            glossary::OnConflict::Update,
        )
        .unwrap_err();
        assert!(matches!(
            blank,
            AddGlossaryTermError::InvalidInput { field: "term", .. }
        ));

        let long_term = "a".repeat(GLOSSARY_TERM_MAX_CHARS + 1);
        let too_long = add_glossary_term(
            vault.path(),
            "Growth",
            glossary_term(&long_term, "def", &[]),
            glossary::OnConflict::Update,
        )
        .unwrap_err();
        assert!(matches!(
            too_long,
            AddGlossaryTermError::InvalidInput { field: "term", .. }
        ));
    }

    #[test]
    fn add_glossary_term_invalid_slug_is_invalid_project() {
        let vault = tempdir().unwrap();
        let err = add_glossary_term(
            vault.path(),
            "", // empty slug — a caller bug, not a missing project
            glossary_term("MERIDIAN", "x.", &[]),
            glossary::OnConflict::Update,
        )
        .unwrap_err();
        assert!(matches!(err, AddGlossaryTermError::InvalidProject(_)));
    }
}
