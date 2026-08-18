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
use crate::ledger;
use crate::meeting;
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

/// What [`set_action_item_done`] did.
#[derive(Debug, Clone, PartialEq)]
pub enum SetDoneOutcome {
    /// The checkbox was flipped; carries the note as it now stands on disk.
    Updated(Box<ListedNote>),
    /// The box already read the way the caller asked for, so nothing was
    /// written.
    AlreadySet,
    /// No note in the vault carries that id.
    NoteMissing,
    /// The note exists but no line in it mints that item id any more.
    ItemMissing,
}

/// Ticks or unticks one action item's checkbox in its source note.
///
/// **The checkbox is the source of truth for done/not-done**
/// ([`crate::ledger`]), so a surface offering a person a checkbox writes the
/// Markdown, and the ledger records only what a checkbox cannot spell. This is
/// the write behind that click, and the counterpart to
/// [`annotate_action_item`], which deliberately never touches the box.
///
/// Only the marker changes: the owner, description, due date and any trailing
/// text are byte-preserved, so the line re-derives the same
/// [`crate::meeting::ActionItemFact`] id (the checkbox character is not hashed)
/// and the re-index this triggers is a no-op for the ledger's identity
/// tracking.
///
/// Best-effort by contract, like its sibling: [`SetDoneOutcome::NoteMissing`]
/// and [`SetDoneOutcome::ItemMissing`] are ordinary answers for a line that was
/// edited away since the ledger linked it, not errors.
pub fn set_action_item_done(
    vault_root: &Path,
    id: &NoteId,
    action_item_id: &str,
    done: bool,
) -> Result<SetDoneOutcome> {
    // Vault-wide for the same reason annotation is: the note may have been
    // re-filed since the ledger linked it, and the id is what never moves.
    let Some((_, listed)) = find_note_anywhere(vault_root, id)? else {
        return Ok(SetDoneOutcome::NoteMissing);
    };
    let Some(line_index) =
        meeting::action_item_line(listed.note.id.as_str(), &listed.note.body, action_item_id)
    else {
        return Ok(SetDoneOutcome::ItemMissing);
    };

    let mut lines: Vec<String> = listed.note.body.lines().map(str::to_string).collect();
    let Some(line) = lines.get(line_index) else {
        return Ok(SetDoneOutcome::ItemMissing);
    };
    // The line parsed as an action item to mint the id above, so it starts with
    // one of the two markers after its indentation. Split rather than trim so
    // the indentation survives verbatim.
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    let Some(body) = rest
        .strip_prefix(UNCHECKED_MARKER)
        .or_else(|| rest.strip_prefix(CHECKED_MARKER))
    else {
        return Ok(SetDoneOutcome::ItemMissing);
    };
    let marker = if done {
        CHECKED_MARKER
    } else {
        UNCHECKED_MARKER
    };
    let updated = format!("{indent}{marker}{body}");
    if updated == *line {
        return Ok(SetDoneOutcome::AlreadySet);
    }
    lines[line_index] = updated;

    let merged = listed.note.clone().with_edits(NoteEdit {
        note_type: listed.note.note_type,
        title: listed.note.title.clone(),
        date: listed.note.date.clone(),
        tags: listed.note.tags.clone(),
        body: lines.join(
            "
",
        ),
    })?;
    note::save_note_at(&listed.path, &merged)?;
    let title = effective_title(&merged, &listed.path);
    Ok(SetDoneOutcome::Updated(Box::new(ListedNote {
        note: merged,
        path: listed.path,
        title,
    })))
}

/// The two action-item markers, exactly as [`crate::distill::parse_action_line`]
/// accepts them (lowercase `x` only).
const UNCHECKED_MARKER: &str = "- [ ] ";
const CHECKED_MARKER: &str = "- [x] ";

/// The prefix every closure-evidence annotation line carries.
///
/// Chosen to be **inert to the action-item grammar**: `meeting::parse_body`
/// trims each line and then silently skips anything `distill::parse_action_line`
/// rejects, and a line starting `- Closed ` can never start `- [ ] ` or
/// `- [x] `. So an annotated body re-derives byte-identical
/// [`crate::meeting::ActionItemFact`]s, ids included, and annotating never mints
/// a phantom item. The prefix is fixed rather than free-form so that when the
/// grammar is later widened to hand-written notes, the widened parser has one
/// literal to recognize and skip.
pub const ANNOTATION_PREFIX: &str = "- Closed ";

/// Indentation that renders the annotation as a sub-bullet of the item it
/// belongs to. Purely cosmetic to the parser (which trims), load-bearing to the
/// reader.
const ANNOTATION_INDENT: &str = "  ";

/// What [`annotate_action_item`] did.
#[derive(Debug, Clone, PartialEq)]
pub enum AnnotateOutcome {
    /// The line was inserted; carries the note as it now stands on disk.
    Annotated(Box<ListedNote>),
    /// No note in the vault carries that id.
    NoteMissing,
    /// The note exists but no line in it mints that item id any more — it was
    /// edited or deleted since the ledger linked it.
    ItemMissing,
    /// The exact line is already there, so nothing was written.
    AlreadyAnnotated,
}

/// Appends a closure-evidence annotation directly beneath an action item in its
/// source note.
///
/// **Annotate, never destroy.** The human-readable story of a commitment lives
/// in the Markdown, so when the ledger closes an entry on evidence it says so
/// here, in a line a person reads, rather than only in a database row. The
/// checkbox is left exactly as the user left it: this function never ticks a
/// box, never rewrites the item, and never removes anything.
///
/// `annotation` is the sentence after the date, rendered by the caller (the
/// evidence provider knows what it found). It is written as
/// `  - Closed <YYYY-MM-DD>: <annotation>`, which
/// [`crate::meeting::parse_body`] skips, so the note's item ids are unchanged
/// and the re-index this write triggers is a no-op for the ledger.
///
/// Best-effort by contract: [`AnnotateOutcome::ItemMissing`] and
/// [`AnnotateOutcome::NoteMissing`] are ordinary answers. A caller records its
/// evidence in the ledger regardless of what this returns, because the database
/// is the operational truth and the note is the narrative.
pub fn annotate_action_item(
    vault_root: &Path,
    id: &NoteId,
    action_item_id: &str,
    closed_on: &str,
    annotation: &str,
) -> Result<AnnotateOutcome> {
    // Located vault-wide, not by a stored project: a note the ledger linked
    // last month may have been re-filed since, and the id is what never moves.
    let Some((_, listed)) = find_note_anywhere(vault_root, id)? else {
        return Ok(AnnotateOutcome::NoteMissing);
    };
    let Some(line_index) =
        meeting::action_item_line(listed.note.id.as_str(), &listed.note.body, action_item_id)
    else {
        return Ok(AnnotateOutcome::ItemMissing);
    };

    let annotation_line =
        format!("{ANNOTATION_INDENT}{ANNOTATION_PREFIX}{closed_on}: {annotation}");
    let mut lines: Vec<&str> = listed.note.body.lines().collect();
    // Idempotent: a provider that retries after a crash must not stack
    // duplicate lines under one item.
    if lines
        .get(line_index + 1)
        .is_some_and(|next| next.trim() == annotation_line.trim())
    {
        return Ok(AnnotateOutcome::AlreadyAnnotated);
    }
    lines.insert(line_index + 1, &annotation_line);
    let body = lines.join("\n");

    let merged = listed.note.clone().with_edits(NoteEdit {
        note_type: listed.note.note_type,
        title: listed.note.title.clone(),
        date: listed.note.date.clone(),
        tags: listed.note.tags.clone(),
        body,
    })?;
    note::save_note_at(&listed.path, &merged)?;
    let title = effective_title(&merged, &listed.path);
    Ok(AnnotateOutcome::Annotated(Box::new(ListedNote {
        note: merged,
        path: listed.path,
        title,
    })))
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

/// Which glossary an operation targets, echoed back with the canonical slug.
///
/// `None` is the **vault-wide** glossary at the knowledge-base root: the one
/// the transcription pipeline loads to bias every capture, since a session is
/// transcribed before routing has chosen a project. `Some(slug)` is that
/// project's own glossary, which feeds routing signals and project context.
type GlossaryScope = Option<String>;

/// A glossary's full contents, in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlossaryListing {
    /// The scope read: `None` for the vault-wide glossary, else the canonical
    /// (casing-adopted) project slug.
    pub project: GlossaryScope,
    /// Every term, in the order the file stores them.
    pub terms: Vec<glossary::GlossaryTerm>,
}

/// The outcome of a glossary write: the affected term and whether it was new.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlossaryWrite {
    /// The scope written: `None` for the vault-wide glossary, else the
    /// canonical project slug.
    pub project: GlossaryScope,
    /// The term as persisted (or, for a removal, the entry that was removed).
    pub term: glossary::GlossaryTerm,
    /// `true` when a new term was created; `false` when an existing term was
    /// updated in place or removed.
    pub created: bool,
}

/// Why a glossary operation could not complete. Kept distinct from
/// [`NoteError`] and [`glossary::GlossaryError`] so a shell can route each case
/// to the right channel: a malformed slug or field is a caller bug, a missing
/// project, a missing term or an `on_conflict: "error"` hit are actionable
/// business faults, and storage failures are internal.
#[derive(Debug, thiserror::Error)]
pub enum GlossaryOpError {
    /// The project slug is malformed (shape or reserved-name violation).
    #[error(transparent)]
    InvalidProject(NoteError),
    /// `term` or `definition` is blank or over its length bound.
    #[error("invalid glossary {field}: {detail}")]
    InvalidInput { field: &'static str, detail: String },
    /// The target project folder does not exist (these functions never create
    /// one).
    #[error("project {project:?} does not exist")]
    MissingProject { project: String },
    /// `on_conflict: "error"` and the normalized term already exists, or a
    /// rename would land on another entry's term.
    #[error("glossary term {term:?} already exists")]
    Conflict { term: String },
    /// The term to update or remove is not in the target glossary.
    #[error("glossary term {term:?} does not exist")]
    NotFound { term: String },
    /// Reading or writing the glossary file failed (I/O, YAML, or a duplicate
    /// term already on disk).
    #[error(transparent)]
    Storage(glossary::GlossaryError),
}

/// Adds or updates a glossary term for `project` — the MCP `add_glossary_term`
/// tool's shared body (`docs/MCP_TOOL_SURFACE.md` §8).
///
/// A project-scoped shim over [`upsert_glossary_term`], kept because the MCP
/// tool's payload always names a project: it echoes the canonical slug as a
/// plain `String` rather than the scope's `Option`.
pub fn add_glossary_term(
    vault_root: &Path,
    project: &str,
    term: glossary::GlossaryTerm,
    on_conflict: glossary::OnConflict,
) -> std::result::Result<GlossaryUpsert, GlossaryOpError> {
    let write = upsert_glossary_term(vault_root, Some(project), term, on_conflict)?;
    Ok(GlossaryUpsert {
        project: write
            .project
            .expect("a project-scoped upsert echoes its project"),
        term: write.term,
        created: write.created,
    })
}

/// Mirrors the MCP input schema's bounds server-side: a term that trims to
/// empty would persist as a blank key, and both fields are length-capped.
fn validate_glossary_fields(
    term: &glossary::GlossaryTerm,
) -> std::result::Result<(), GlossaryOpError> {
    if term.term.trim().is_empty() {
        return Err(GlossaryOpError::InvalidInput {
            field: "term",
            detail: "term must not be blank".to_string(),
        });
    }
    if term.term.chars().count() > GLOSSARY_TERM_MAX_CHARS {
        return Err(GlossaryOpError::InvalidInput {
            field: "term",
            detail: format!("term must be at most {GLOSSARY_TERM_MAX_CHARS} characters"),
        });
    }
    if term.definition.trim().is_empty() {
        return Err(GlossaryOpError::InvalidInput {
            field: "definition",
            detail: "definition must not be blank".to_string(),
        });
    }
    if term.definition.chars().count() > GLOSSARY_DEFINITION_MAX_CHARS {
        return Err(GlossaryOpError::InvalidInput {
            field: "definition",
            detail: format!(
                "definition must be at most {GLOSSARY_DEFINITION_MAX_CHARS} characters"
            ),
        });
    }
    Ok(())
}

/// Resolves a glossary scope to the directory holding its `_glossary.yml`,
/// echoing the canonical scope alongside it.
///
/// `None` is the vault root itself. A project slug is validated for shape and
/// adopts on-disk casing (rejecting reserved/hidden names and the Inbox), and
/// the folder must already exist — these functions never create a project.
fn glossary_scope_dir(
    vault_root: &Path,
    project: Option<&str>,
) -> std::result::Result<(GlossaryScope, PathBuf), GlossaryOpError> {
    let Some(project) = project else {
        return Ok((None, vault_root.to_path_buf()));
    };
    // A malformed slug is a caller bug, not a missing project.
    let canonical =
        canonicalize_project(vault_root, project).map_err(GlossaryOpError::InvalidProject)?;
    let dir = note::project_dir(vault_root, &canonical);
    if !dir.is_dir() {
        return Err(GlossaryOpError::MissingProject { project: canonical });
    }
    Ok((Some(canonical), dir))
}

/// Opens the glossary for a scope, returning the resolved scope, its directory
/// and the loaded contents — the shared preamble of every operation below.
fn load_scoped_glossary(
    vault_root: &Path,
    project: Option<&str>,
) -> std::result::Result<(GlossaryScope, PathBuf, glossary::Glossary), GlossaryOpError> {
    let (scope, dir) = glossary_scope_dir(vault_root, project)?;
    let glossary = glossary::Glossary::load(&dir).map_err(glossary_op_err)?;
    Ok((scope, dir, glossary))
}

/// Persists a glossary, creating the vault root first when that is the scope.
///
/// A project folder is guaranteed to exist by [`glossary_scope_dir`], but the
/// vault root may not: on a fresh install nothing has written the knowledge
/// base yet, and the first vault-wide term must not fail on a missing parent.
fn save_scoped_glossary(
    glossary: &glossary::Glossary,
    dir: &Path,
    scope: &GlossaryScope,
) -> std::result::Result<(), GlossaryOpError> {
    if scope.is_none() {
        std::fs::create_dir_all(dir).map_err(|source| {
            glossary_op_err(glossary::GlossaryError::Io {
                path: dir.to_path_buf(),
                source,
            })
        })?;
    }
    glossary.save(dir).map_err(glossary_op_err)
}

/// Reads every term in a glossary, in file order.
///
/// `project` is `None` for the vault-wide glossary at the knowledge-base root
/// (the one transcription biases against) or `Some(slug)` for a project's own.
/// A scope with no glossary file yet lists no terms rather than failing.
pub fn list_glossary_terms(
    vault_root: &Path,
    project: Option<&str>,
) -> std::result::Result<GlossaryListing, GlossaryOpError> {
    let (scope, _dir, glossary) = load_scoped_glossary(vault_root, project)?;
    Ok(GlossaryListing {
        project: scope,
        terms: glossary.terms().to_vec(),
    })
}

/// Adds a term to a glossary, or updates the existing entry with the same
/// normalized term.
///
/// `on_conflict` decides an existing normalized term: [`OnConflict::Update`]
/// overwrites it, [`OnConflict::Error`] leaves it untouched and returns
/// [`GlossaryOpError::Conflict`].
///
/// [`OnConflict::Update`]: glossary::OnConflict::Update
/// [`OnConflict::Error`]: glossary::OnConflict::Error
pub fn upsert_glossary_term(
    vault_root: &Path,
    project: Option<&str>,
    term: glossary::GlossaryTerm,
    on_conflict: glossary::OnConflict,
) -> std::result::Result<GlossaryWrite, GlossaryOpError> {
    validate_glossary_fields(&term)?;
    let (scope, dir, mut glossary) = load_scoped_glossary(vault_root, project)?;

    let lookup = term.term.clone();
    let created = glossary
        .upsert(term, on_conflict)
        .map_err(glossary_op_err)?;
    save_scoped_glossary(&glossary, &dir, &scope)?;

    // Echo exactly what landed on disk. `upsert` just inserted/updated this
    // term, so the lookup is guaranteed to hit.
    let stored = glossary
        .get(&lookup)
        .cloned()
        .expect("the just-upserted term is present in the glossary");
    Ok(GlossaryWrite {
        project: scope,
        term: stored,
        created,
    })
}

/// Replaces the entry named by `original_term` with `term`, preserving its
/// position in the file.
///
/// `term.term` may differ from `original_term` — that is a rename — but only
/// onto a key no other entry holds, else [`GlossaryOpError::Conflict`]. An
/// `original_term` that names nothing is [`GlossaryOpError::NotFound`], which
/// is how a caller tells a stale edit from a storage failure.
pub fn update_glossary_term(
    vault_root: &Path,
    project: Option<&str>,
    original_term: &str,
    term: glossary::GlossaryTerm,
) -> std::result::Result<GlossaryWrite, GlossaryOpError> {
    validate_glossary_fields(&term)?;
    let (scope, dir, mut glossary) = load_scoped_glossary(vault_root, project)?;

    let stored = glossary
        .update(original_term, term)
        .map_err(glossary_op_err)?
        .ok_or_else(|| GlossaryOpError::NotFound {
            term: original_term.to_string(),
        })?;
    save_scoped_glossary(&glossary, &dir, &scope)?;

    Ok(GlossaryWrite {
        project: scope,
        term: stored,
        created: false,
    })
}

/// Removes a term from a glossary, echoing the entry that was removed.
///
/// Matches the primary term only (aliases point at an entry, they don't name
/// it). A term that is not present is [`GlossaryOpError::NotFound`].
pub fn remove_glossary_term(
    vault_root: &Path,
    project: Option<&str>,
    term: &str,
) -> std::result::Result<GlossaryWrite, GlossaryOpError> {
    let (scope, dir, mut glossary) = load_scoped_glossary(vault_root, project)?;

    let removed = glossary
        .remove(term)
        .ok_or_else(|| GlossaryOpError::NotFound {
            term: term.to_string(),
        })?;
    save_scoped_glossary(&glossary, &dir, &scope)?;

    Ok(GlossaryWrite {
        project: scope,
        term: removed,
        created: false,
    })
}

/// Routes a [`glossary::GlossaryError`] into [`GlossaryOpError`]: a conflict
/// (an `on_conflict: "error"` hit, or a rename onto an existing term) is a
/// distinct business fault the shell surfaces differently from an I/O or YAML
/// failure.
fn glossary_op_err(err: glossary::GlossaryError) -> GlossaryOpError {
    match err {
        glossary::GlossaryError::Conflict { term } => GlossaryOpError::Conflict { term },
        other => GlossaryOpError::Storage(other),
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
/// (`_glossary.yml`, `_routing_examples.yml`, `_ledger.yml`, writer scratch
/// temps) are removed, and the folder tree is deleted.
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

/// The outcome of a project rename: the renamed project as [`list_projects`]
/// now reports it, every contained note at its new path (whether or not its
/// frontmatter needed rewriting), and the paths whose rewrite failed after the
/// folder had already moved.
#[derive(Debug)]
pub struct RenamedProject {
    pub info: ProjectInfo,
    pub renamed_notes: Vec<ListedNote>,
    /// Notes that moved with the folder but whose `project:` could not be
    /// rewritten. Empty in every ordinary run; non-empty means some notes still
    /// name the old project (see the failure contract below).
    pub failed_rewrites: Vec<PathBuf>,
}

/// Renames a project: the folder moves, and every contained note — direct and
/// in child projects — has its frontmatter `project:` re-filed under the new
/// slug. The slug *is* a project's identity (it is the folder path, the handle
/// MCP tools accept, and the `project:` every note stores), so this rewrites all
/// three rather than storing a display name beside them.
///
/// A nested target re-parents (`Growth/Q3` → `Archive/Q3`), creating missing
/// parents. Both slugs are validated and adopt on-disk casing
/// ([`canonicalize_project`]); a case-only change to the project's own segment
/// (`Ops` → `ops`, `Growth/Q3` → `Growth/q3`) is honoured, since on a
/// case-insensitive filesystem canonicalization would otherwise fold it back
/// into a no-op. A case-only change to a *parent* segment is not: renaming a
/// parent is that project's own rename, and the folder move here cannot spell
/// it. Rejected: the Inbox sentinel on either
/// side, the name the project already has, an existing target in any casing
/// ([`NoteError::ProjectExists`]), and a target inside the project's own
/// subtree.
///
/// **Unmanaged items ride along.** Unlike [`delete_project`], an attachment or
/// an unparseable `.md` is not a blocker: a rename destroys nothing, and
/// `fs::rename` carries the whole tree over untouched. `_glossary.yml`,
/// `_routing_examples.yml` and `_ledger.yml` move with the folder and need no
/// rewrite — none of the three names its own project, which is precisely the
/// invariant that keeps this a move rather than a rewrite, and which a new infra
/// file has to preserve. `previous_project` strings in *other* projects' logs
/// keep naming the old slug, the same historical provenance
/// [`delete_project`] leaves behind.
///
/// **Failure consistency:** the directory move runs before any frontmatter is
/// touched, because it is the step most likely to fail (an open handle anywhere
/// in the tree). A failure there is a clean no-op. Once the folder has moved a
/// per-note rewrite failure is *collected, not fatal* — aborting would strand
/// more notes, and unlike a delete a retry cannot converge, since the old slug
/// no longer exists. The residue is the state an external folder move already
/// produces: notes physically under the new folder whose frontmatter names the
/// old one. Reconcile keeps the index truthful throughout (it re-keys on the
/// note id, updating `path` and leaving `project` to the frontmatter, and never
/// deletes a row whose file is present), and each stale note converges the next
/// time it is edited or re-routed.
pub fn rename_project(
    vault_root: &Path,
    project: &str,
    new_project: &str,
) -> Result<RenamedProject> {
    // The exact sentinel slips past `validate_project`; every other casing is
    // rejected inside `canonicalize_project` (mirrors `delete_project` ①).
    if project == INBOX {
        return Err(NoteError::InvalidField {
            field: "project",
            detail: "the Inbox is built in and cannot be renamed".to_string(),
        });
    }
    if new_project == INBOX {
        return Err(NoteError::InvalidField {
            field: "new_project",
            detail: "the Inbox is built in; no project can be renamed to it".to_string(),
        });
    }
    let canonical_old = canonicalize_project(vault_root, project)?;
    let old_dir = note::project_dir(vault_root, &canonical_old);
    if !old_dir.is_dir() {
        return Err(NoteError::MissingProject {
            project: canonical_old,
        });
    }
    // Validates the new slug (reserved, hidden and illegal names included) and
    // adopts the casing of any parent segment already on disk.
    let canonical_new = canonicalize_project(vault_root, new_project)?;

    let final_new = if canonical_new == canonical_old {
        // Canonicalization folded the new slug onto the existing folder, so the
        // caller either re-typed the current name or changed only its casing.
        //
        // Only the LAST segment can actually be re-cased, and the typed casing
        // is honoured for that segment alone. `fs::rename` renames the leaf; the
        // parent segments of the destination path resolve to the directories
        // already on disk, so a rename to `growth/Q3` under an on-disk `Growth/`
        // moves nothing and leaves the parent spelled `Growth`. Taking the typed
        // slug verbatim would then re-file every note under `growth/Q3` — a slug
        // no directory carries, which `list_projects` never reports and the
        // index's case-sensitive `notes.project = ?` scope never matches.
        let (parent, old_leaf) = match canonical_old.rsplit_once('/') {
            Some((parent, leaf)) => (Some(parent), leaf),
            None => (None, canonical_old.as_str()),
        };
        // Same segment count as `canonical_old`, since canonicalization maps
        // segment-wise and the two canonical forms are equal.
        let typed_leaf = new_project.rsplit('/').next().unwrap_or(new_project);
        if typed_leaf == old_leaf {
            // Nothing this rename can change: either the name was re-typed as
            // it stands, or only a parent's casing differs — and a parent is
            // renamed by renaming *that* project, not this one.
            return Err(NoteError::InvalidField {
                field: "new_project",
                detail: format!("{canonical_old:?} is already this project's name"),
            });
        }
        // A case-only rename: keep the on-disk parent, take the typed leaf, and
        // skip the exists check below, since on a case-insensitive filesystem
        // the destination *is* the source.
        match parent {
            Some(parent) => format!("{parent}/{typed_leaf}"),
            None => typed_leaf.to_string(),
        }
    } else {
        if is_within_project(&canonical_new, &canonical_old) {
            return Err(NoteError::InvalidField {
                field: "new_project",
                detail: format!("cannot move project {canonical_old:?} inside itself"),
            });
        }
        if note::project_dir(vault_root, &canonical_new).exists() {
            return Err(NoteError::ProjectExists {
                project: canonical_new,
            });
        }
        canonical_new
    };

    // ① Pre-flight: parse the tree before touching disk, so the frontmatter
    // rewrite works from a snapshot and an unreadable entry fails up front.
    // Unmanaged items are recorded by the walk and deliberately ignored.
    let mut scan = TreeScan::default();
    walk_project_tree(&old_dir, &mut scan)?;

    // ② Create any parent the target needs, remembering what we made so a
    // failed move leaves no orphaned empty folder behind.
    let new_dir = note::project_dir(vault_root, &final_new);
    let created_parent = match new_dir.parent() {
        Some(parent) if !parent.exists() => {
            fs::create_dir_all(parent).map_err(|source| NoteError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
            Some(parent.to_path_buf())
        }
        _ => None,
    };

    // ③ The move. Everything above this line is reversible by doing nothing;
    // everything below it assumes the folder is already at its new path.
    if let Err(source) = rename_dir(&old_dir, &new_dir) {
        if let Some(parent) = created_parent {
            prune_empty_dirs(&parent, vault_root);
        }
        return Err(NoteError::Io {
            path: new_dir,
            source,
        });
    }

    // ④ Re-file every parsed note. Only notes actually filed under the old slug
    // are rewritten: one naming a different project is external-move residue
    // that predates this rename, and its filing is not ours to change.
    let mut renamed_notes = Vec::with_capacity(scan.notes.len());
    let mut failed_rewrites = Vec::new();
    for (old_path, mut note) in scan.notes {
        let Ok(relative) = old_path.strip_prefix(&old_dir) else {
            // Unreachable: every path here came from walking `old_dir`.
            failed_rewrites.push(old_path);
            continue;
        };
        let new_path = new_dir.join(relative);
        if let Some(refiled) = reproject(note.routing.project(), &canonical_old, &final_new) {
            // Mutating `routing` in place preserves every other field verbatim.
            // Rebuilding through `Note::new` would drop the stored `title`.
            let routing = match &note.routing {
                Routing::Routed { confidence, .. } => Routing::Routed {
                    project: refiled,
                    confidence: *confidence,
                },
                Routing::Manual { .. } => Routing::Manual { project: refiled },
            };
            note.routing = routing;
            if note::save_note_at(&new_path, &note).is_err() {
                failed_rewrites.push(new_path.clone());
            }
        }
        renamed_notes.push(ListedNote {
            title: effective_title(&note, &new_path),
            path: new_path,
            note,
        });
    }

    // ⑤ Re-scan the moved folder so the counts read exactly as `list_projects`
    // would report them.
    let mut scanned = Vec::new();
    collect_project(&new_dir, final_new.clone(), &mut scanned);
    let info = scanned
        .into_iter()
        .find(|candidate| candidate.slug == final_new)
        .unwrap_or(ProjectInfo {
            slug: final_new,
            note_count: 0,
            meeting_count: 0,
            last_activity: None,
        });

    Ok(RenamedProject {
        info,
        renamed_notes,
        failed_rewrites,
    })
}

/// Whether `slug` is `ancestor` itself or a project nested inside it, compared
/// segment-wise and ASCII-case-insensitively: `Growth` contains `Growth/Q3` but
/// never `Growthx`. The same anchoring rule [`crate::index::scope`] uses for a
/// subtree query, so a rename re-files exactly the notes a subtree search finds.
fn is_within_project(slug: &str, ancestor: &str) -> bool {
    slug.get(..ancestor.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(ancestor))
        && matches!(slug.as_bytes().get(ancestor.len()), None | Some(b'/'))
}

/// A note's new `project:` value when it was filed under `old` — the project
/// itself or a descendant — else `None`. Only the ancestor prefix is swapped,
/// so a note in `Growth/Q3` follows a `Growth` → `Acme` rename to `Acme/Q3`.
fn reproject(filed_under: &str, old: &str, new: &str) -> Option<String> {
    is_within_project(filed_under, old).then(|| format!("{new}{}", &filed_under[old.len()..]))
}

/// Moves a project directory, falling back to a two-hop move through a hidden
/// sibling scratch name when the direct rename fails. A case-only rename
/// (`Ops` → `ops`) is what needs it: NTFS and APFS accept one directly, but a
/// filesystem that compares names case-insensitively on *rename* sees the
/// destination as the source and refuses. A failed second hop rolls the first
/// one back, so a failure here leaves the project where it started.
fn rename_dir(old_dir: &Path, new_dir: &Path) -> io::Result<()> {
    let Err(direct) = fs::rename(old_dir, new_dir) else {
        return Ok(());
    };
    let Some(parent) = old_dir.parent() else {
        return Err(direct);
    };
    let scratch = parent.join(format!(".rename.{}.tmp", std::process::id()));
    if scratch.exists() {
        return Err(direct);
    }
    fs::rename(old_dir, &scratch)?;
    if let Err(second) = fs::rename(&scratch, new_dir) {
        let _ = fs::rename(&scratch, old_dir);
        return Err(second);
    }
    Ok(())
}

/// Removes `dir` and each now-empty ancestor below `stop`, ignoring failures —
/// undoing a parent folder created for a re-parenting rename that then failed.
/// `remove_dir` is non-recursive, so a directory holding anything stops the walk
/// rather than deleting it.
fn prune_empty_dirs(dir: &Path, stop: &Path) {
    let mut current = dir;
    while current != stop && current.starts_with(stop) && fs::remove_dir(current).is_ok() {
        let Some(parent) = current.parent() else {
            return;
        };
        current = parent;
    }
}

/// One project tree classified for [`delete_project`] and [`rename_project`].
/// `dirs` is in pre-order (parent before child), so reverse iteration removes
/// children first.
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
///
/// [`rename_project`] shares the walk for its note snapshot but ignores
/// `unmanaged`: it destroys nothing, so an item it does not understand is
/// carried along by the folder move rather than blocking it.
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
/// with it: the glossary, routing-examples, and commitment-ledger files, plus
/// their (and the note writer's) crash-leftover scratch temps.
///
/// Anything else in the folder is a user's own file, and `delete_project`
/// refuses to remove a folder holding one. So **a new infrastructure file must
/// be added here in the same change that starts writing it**, or every project
/// that has ever written one becomes undeletable.
fn is_removable_infra(name: &str) -> bool {
    name == glossary::GLOSSARY_FILE
        || name == routing_examples::ROUTING_EXAMPLES_FILE
        || name == ledger::LEDGER_SNAPSHOT_FILE
        || (name.ends_with(".tmp")
            && (name.starts_with(".note.")
                || name.starts_with(glossary::GLOSSARY_FILE)
                || name.starts_with(routing_examples::ROUTING_EXAMPLES_FILE)
                || name.starts_with(ledger::LEDGER_SNAPSHOT_FILE)))
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

    // --- annotate_action_item ---------------------------------------------

    /// Writes a meeting note whose body carries the distilled action-item
    /// grammar, and returns its path plus the derived facts.
    fn meeting_with_items(vault: &Path, project: &str, id: &str) -> Vec<meeting::ActionItemFact> {
        let note = Note::new(
            NoteId::parse(id).unwrap(),
            NoteType::Meeting,
            Routing::Manual {
                project: project.to_string(),
            },
            "2026-08-01",
            vec![Tag::parse("fixture").unwrap()],
            Source::parse("manual").unwrap(),
            "# Summary\n\nWe met.\n\n## Action items\n\n\
             - [ ] Priya to send the revised deck by 2026-08-20.\n\
             - [ ] You to book the venue.\n",
        )
        .unwrap();
        note::write_note(vault, &note, Some("kickoff")).unwrap();
        meeting::meeting_facts_for(&note, vault)
            .unwrap()
            .action_items
    }

    #[test]
    fn annotate_action_item_writes_under_the_right_line() {
        let vault = tempdir().unwrap();
        let items = meeting_with_items(vault.path(), "Ops", "n_aaaaaa");
        let target = &items[0];

        let outcome = annotate_action_item(
            vault.path(),
            &NoteId::parse("n_aaaaaa").unwrap(),
            &target.id,
            "2026-08-17",
            "PR merged (example.com/pull/42), evidence in n_bbbbbb.",
        )
        .unwrap();

        let AnnotateOutcome::Annotated(listed) = outcome else {
            panic!("expected an annotation, got {outcome:?}");
        };
        let body = &listed.note.body;
        let lines: Vec<&str> = body.lines().collect();
        let item_line = lines
            .iter()
            .position(|line| line.contains("send the revised deck"))
            .unwrap();
        assert_eq!(
            lines[item_line + 1],
            "  - Closed 2026-08-17: PR merged (example.com/pull/42), evidence in n_bbbbbb."
        );
        // The item itself is untouched: the checkbox is the user's.
        assert!(lines[item_line].starts_with("- [ ] "));
        // And the second item did not move or change.
        assert!(body.contains("- [ ] You to book the venue."));
    }

    #[test]
    fn an_annotated_body_re_derives_identical_action_items() {
        // The whole point of the line's shape: it is inert to the grammar, so
        // annotating never re-mints an id and never mints a phantom item.
        let vault = tempdir().unwrap();
        let before = meeting_with_items(vault.path(), "Ops", "n_aaaaaa");

        annotate_action_item(
            vault.path(),
            &NoteId::parse("n_aaaaaa").unwrap(),
            &before[0].id,
            "2026-08-17",
            "closed by evidence.",
        )
        .unwrap();

        let (_, listed) = find_note_anywhere(vault.path(), &NoteId::parse("n_aaaaaa").unwrap())
            .unwrap()
            .unwrap();
        let after = meeting::meeting_facts_for(&listed.note, vault.path())
            .unwrap()
            .action_items;
        assert_eq!(before, after, "annotation must be invisible to extraction");
    }

    #[test]
    fn annotate_action_item_is_idempotent() {
        let vault = tempdir().unwrap();
        let items = meeting_with_items(vault.path(), "Ops", "n_aaaaaa");
        let id = NoteId::parse("n_aaaaaa").unwrap();

        annotate_action_item(vault.path(), &id, &items[0].id, "2026-08-17", "done.").unwrap();
        let second =
            annotate_action_item(vault.path(), &id, &items[0].id, "2026-08-17", "done.").unwrap();

        assert_eq!(second, AnnotateOutcome::AlreadyAnnotated);
        let (_, listed) = find_note_anywhere(vault.path(), &id).unwrap().unwrap();
        assert_eq!(
            listed.note.body.matches("- Closed 2026-08-17").count(),
            1,
            "a retry must not stack duplicate lines"
        );
    }

    #[test]
    fn annotate_action_item_reports_a_missing_note_or_item() {
        let vault = tempdir().unwrap();
        meeting_with_items(vault.path(), "Ops", "n_aaaaaa");

        assert_eq!(
            annotate_action_item(
                vault.path(),
                &NoteId::parse("n_zzzzzz").unwrap(),
                "a_111111",
                "2026-08-17",
                "x",
            )
            .unwrap(),
            AnnotateOutcome::NoteMissing
        );
        assert_eq!(
            annotate_action_item(
                vault.path(),
                &NoteId::parse("n_aaaaaa").unwrap(),
                "a_notreal",
                "2026-08-17",
                "x",
            )
            .unwrap(),
            AnnotateOutcome::ItemMissing
        );
    }

    #[test]
    fn annotate_action_item_disambiguates_duplicate_lines() {
        // Two byte-identical lines differ only by occurrence, so the second's id
        // must land on the second line.
        let vault = tempdir().unwrap();
        let note = Note::new(
            NoteId::parse("n_aaaaaa").unwrap(),
            NoteType::Meeting,
            Routing::Manual {
                project: "Ops".to_string(),
            },
            "2026-08-01",
            vec![Tag::parse("fixture").unwrap()],
            Source::parse("manual").unwrap(),
            "## Action items\n\n\
             - [ ] Priya to send the deck.\n\
             - [ ] Priya to send the deck.\n",
        )
        .unwrap();
        note::write_note(vault.path(), &note, Some("dupes")).unwrap();
        let items = meeting::meeting_facts_for(&note, vault.path())
            .unwrap()
            .action_items;
        assert_eq!(items.len(), 2);

        annotate_action_item(
            vault.path(),
            &NoteId::parse("n_aaaaaa").unwrap(),
            &items[1].id,
            "2026-08-17",
            "the second one.",
        )
        .unwrap();

        let (_, listed) = find_note_anywhere(vault.path(), &NoteId::parse("n_aaaaaa").unwrap())
            .unwrap()
            .unwrap();
        let lines: Vec<&str> = listed.note.body.lines().collect();
        let annotated = lines
            .iter()
            .position(|line| line.contains("- Closed"))
            .unwrap();
        // It sits under the *second* item line, not the first.
        assert!(lines[annotated - 1].contains("send the deck"));
        assert!(lines[annotated - 2].contains("send the deck"));
    }

    // --- set_action_item_done ---------------------------------------------

    #[test]
    fn set_action_item_done_flips_only_the_marker() {
        let vault = tempdir().unwrap();
        let items = meeting_with_items(vault.path(), "Ops", "n_aaaaaa");
        let id = NoteId::parse("n_aaaaaa").unwrap();

        let outcome = set_action_item_done(vault.path(), &id, &items[0].id, true).unwrap();

        let SetDoneOutcome::Updated(listed) = outcome else {
            panic!("expected an update, got {outcome:?}");
        };
        assert!(listed
            .note
            .body
            .contains("- [x] Priya to send the revised deck by 2026-08-20."));
        // The sibling line is untouched.
        assert!(listed.note.body.contains("- [ ] You to book the venue."));
    }

    #[test]
    fn a_ticked_item_keeps_its_id_and_reads_back_done() {
        // The checkbox character is not hashed into the `a_` id, which is what
        // lets the ledger keep tracking a line across a tick.
        let vault = tempdir().unwrap();
        let before = meeting_with_items(vault.path(), "Ops", "n_aaaaaa");
        let id = NoteId::parse("n_aaaaaa").unwrap();

        set_action_item_done(vault.path(), &id, &before[0].id, true).unwrap();

        let (_, listed) = find_note_anywhere(vault.path(), &id).unwrap().unwrap();
        let after = meeting::meeting_facts_for(&listed.note, vault.path())
            .unwrap()
            .action_items;
        assert_eq!(after[0].id, before[0].id, "the id must survive a tick");
        assert!(after[0].done);
        assert!(!after[1].done);
        // And unticking returns the note to exactly where it started.
        set_action_item_done(vault.path(), &id, &before[0].id, false).unwrap();
        let (_, restored) = find_note_anywhere(vault.path(), &id).unwrap().unwrap();
        let restored = meeting::meeting_facts_for(&restored.note, vault.path())
            .unwrap()
            .action_items;
        assert_eq!(restored, before);
    }

    #[test]
    fn set_action_item_done_is_idempotent() {
        let vault = tempdir().unwrap();
        let items = meeting_with_items(vault.path(), "Ops", "n_aaaaaa");
        let id = NoteId::parse("n_aaaaaa").unwrap();

        set_action_item_done(vault.path(), &id, &items[0].id, true).unwrap();
        let second = set_action_item_done(vault.path(), &id, &items[0].id, true).unwrap();

        assert_eq!(second, SetDoneOutcome::AlreadySet);
        // Unticking an already-unticked item is the same answer.
        assert_eq!(
            set_action_item_done(vault.path(), &id, &items[1].id, false).unwrap(),
            SetDoneOutcome::AlreadySet
        );
    }

    #[test]
    fn set_action_item_done_reports_a_missing_note_or_item() {
        let vault = tempdir().unwrap();
        meeting_with_items(vault.path(), "Ops", "n_aaaaaa");

        assert_eq!(
            set_action_item_done(
                vault.path(),
                &NoteId::parse("n_zzzzzz").unwrap(),
                "a_whatever",
                true
            )
            .unwrap(),
            SetDoneOutcome::NoteMissing
        );
        assert_eq!(
            set_action_item_done(
                vault.path(),
                &NoteId::parse("n_aaaaaa").unwrap(),
                "a_gone",
                true
            )
            .unwrap(),
            SetDoneOutcome::ItemMissing
        );
    }

    #[test]
    fn set_action_item_done_disambiguates_duplicate_lines() {
        // Two byte-identical lines: the second id must tick the second line.
        let vault = tempdir().unwrap();
        let note = Note::new(
            NoteId::parse("n_aaaaaa").unwrap(),
            NoteType::Meeting,
            Routing::Manual {
                project: "Ops".to_string(),
            },
            "2026-08-01",
            vec![Tag::parse("fixture").unwrap()],
            Source::parse("manual").unwrap(),
            "## Action items\n\n\
             - [ ] Priya to send the deck.\n\
             - [ ] Priya to send the deck.\n",
        )
        .unwrap();
        note::write_note(vault.path(), &note, Some("dupes")).unwrap();
        let items = meeting::meeting_facts_for(&note, vault.path())
            .unwrap()
            .action_items;

        set_action_item_done(
            vault.path(),
            &NoteId::parse("n_aaaaaa").unwrap(),
            &items[1].id,
            true,
        )
        .unwrap();

        let (_, listed) = find_note_anywhere(vault.path(), &NoteId::parse("n_aaaaaa").unwrap())
            .unwrap()
            .unwrap();
        let ticked: Vec<&str> = listed
            .note
            .body
            .lines()
            .filter(|line| line.starts_with("- [x] "))
            .collect();
        assert_eq!(ticked.len(), 1);
        let lines: Vec<&str> = listed.note.body.lines().collect();
        let ticked_at = lines.iter().position(|l| l.starts_with("- [x] ")).unwrap();
        let first_item = lines.iter().position(|l| l.starts_with("- [ ] ")).unwrap();
        assert!(
            ticked_at > first_item,
            "the second line must be the ticked one"
        );
    }

    #[test]
    fn ticking_an_item_preserves_its_annotation_line() {
        // Annotate, never destroy: a closure line written by an evidence pass
        // survives the user ticking the box above it.
        let vault = tempdir().unwrap();
        let items = meeting_with_items(vault.path(), "Ops", "n_aaaaaa");
        let id = NoteId::parse("n_aaaaaa").unwrap();
        annotate_action_item(vault.path(), &id, &items[0].id, "2026-08-17", "PR merged.").unwrap();

        set_action_item_done(vault.path(), &id, &items[0].id, true).unwrap();

        let (_, listed) = find_note_anywhere(vault.path(), &id).unwrap().unwrap();
        let lines: Vec<&str> = listed.note.body.lines().collect();
        let ticked = lines.iter().position(|l| l.starts_with("- [x] ")).unwrap();
        assert_eq!(lines[ticked + 1], "  - Closed 2026-08-17: PR merged.");
    }

    #[test]
    fn delete_project_removes_the_ledger_snapshot_with_the_folder() {
        // Without `_ledger.yml` in `is_removable_infra`, every project that has
        // ever flushed a snapshot becomes undeletable.
        let vault = tempdir().unwrap();
        write(vault.path(), "Ops", "n_aaaaaa", "2026-07-10", Some("real"));
        let dir = vault.path().join("Ops");
        fs::write(dir.join(ledger::LEDGER_SNAPSHOT_FILE), "version: 1\n").unwrap();
        fs::write(
            dir.join(format!("{}.9999.0.tmp", ledger::LEDGER_SNAPSHOT_FILE)),
            "half written",
        )
        .unwrap();

        let deleted = delete_project(vault.path(), "Ops").unwrap();
        assert_eq!(deleted.moved_notes.len(), 1);
        assert!(!dir.exists(), "the folder and its infra went together");
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

    // --- rename_project -----------------------------------------------------

    /// A routing-scored note (the `write` helper files everything `Manual`), so
    /// the rewrite's confidence preservation is observable.
    fn routed_note(project: &str, id: &str, confidence: f64) -> Note {
        Note::new(
            NoteId::parse(id).unwrap(),
            NoteType::Meeting,
            Routing::Routed {
                project: project.to_string(),
                confidence,
            },
            "2026-07-12",
            vec![Tag::parse("fixture").unwrap()],
            Source::parse("manual").unwrap(),
            "Body.",
        )
        .unwrap()
    }

    #[test]
    fn rename_project_moves_the_folder_and_refiles_its_notes() {
        let vault = tempdir().unwrap();
        write(
            vault.path(),
            "Growth",
            "n_aaaaaa",
            "2026-07-10",
            Some("plan"),
        );

        let renamed = rename_project(vault.path(), "Growth", "Acme").unwrap();

        assert_eq!(renamed.info.slug, "Acme");
        assert_eq!(renamed.info.note_count, 1);
        assert!(renamed.failed_rewrites.is_empty());
        assert!(!vault.path().join("Growth").exists());

        let moved = vault.path().join("Acme").join("plan.md");
        let note = read_note_at(&moved);
        assert_eq!(note.routing.project(), "Acme");
        // Everything but the filing survives verbatim, the stem included.
        assert_eq!(note.id.as_str(), "n_aaaaaa");
        assert_eq!(note.date, "2026-07-10");
        assert_eq!(note.body, "Body.");
        assert_eq!(note.source, Source::parse("manual").unwrap());
        assert_eq!(renamed.renamed_notes.len(), 1);
        assert_eq!(renamed.renamed_notes[0].path, moved);

        let slugs: Vec<String> = list_projects(vault.path())
            .unwrap()
            .into_iter()
            .map(|p| p.slug)
            .collect();
        assert_eq!(slugs, ["Acme"]);
    }

    #[test]
    fn rename_project_refiles_child_project_notes_under_the_new_parent() {
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
            "Growth/Q3",
            "n_bbbbbb",
            "2026-07-11",
            Some("kickoff"),
        );

        rename_project(vault.path(), "Growth", "Acme").unwrap();

        // Only the ancestor prefix is swapped — the child keeps its own segment.
        let child = read_note_at(&vault.path().join("Acme").join("Q3").join("kickoff.md"));
        assert_eq!(child.routing.project(), "Acme/Q3");
        let parent = read_note_at(&vault.path().join("Acme").join("plan.md"));
        assert_eq!(parent.routing.project(), "Acme");

        let slugs: Vec<String> = list_projects(vault.path())
            .unwrap()
            .into_iter()
            .map(|p| p.slug)
            .collect();
        assert_eq!(slugs, ["Acme", "Acme/Q3"]);
    }

    #[test]
    fn rename_project_preserves_title_confidence_and_manual_filing() {
        let vault = tempdir().unwrap();
        let scored = routed_note("Growth", "n_aaaaaa", 0.87).with_title(Some("Q3 review".into()));
        note::write_note(vault.path(), &scored, Some("q3-review")).unwrap();
        write(
            vault.path(),
            "Growth",
            "n_bbbbbb",
            "2026-07-10",
            Some("hand"),
        );

        rename_project(vault.path(), "Growth", "Acme").unwrap();

        // A rebuild through `Note::new` would silently drop the stored title.
        let moved = read_note_at(&vault.path().join("Acme").join("q3-review.md"));
        assert_eq!(moved.title.as_deref(), Some("Q3 review"));
        assert_eq!(
            moved.routing,
            Routing::Routed {
                project: "Acme".to_string(),
                confidence: 0.87,
            }
        );
        // A hand-filed note stays hand-filed; a rename is not a routing event.
        let manual = read_note_at(&vault.path().join("Acme").join("hand.md"));
        assert_eq!(
            manual.routing,
            Routing::Manual {
                project: "Acme".to_string(),
            }
        );
    }

    #[test]
    fn rename_project_accepts_a_case_only_change() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Ops", "n_aaaaaa", "2026-07-10", Some("plan"));

        // Canonicalization would fold `ops` back onto the existing `Ops/`; the
        // case-only branch keeps the typed casing instead of no-op'ing.
        let renamed = rename_project(vault.path(), "Ops", "ops").unwrap();
        assert_eq!(renamed.info.slug, "ops");

        let slugs: Vec<String> = list_projects(vault.path())
            .unwrap()
            .into_iter()
            .map(|p| p.slug)
            .collect();
        assert_eq!(slugs, ["ops"]);
        let note = read_note_at(&vault.path().join("ops").join("plan.md"));
        assert_eq!(note.routing.project(), "ops");
    }

    #[test]
    fn rename_project_recases_a_nested_projects_own_segment_only() {
        let vault = tempdir().unwrap();
        write(
            vault.path(),
            "Growth/Q3",
            "n_aaaaaa",
            "2026-07-10",
            Some("plan"),
        );

        // The leaf is this project's to re-case, and the parent keeps the
        // casing it has on disk even though the caller typed it lowercase.
        let renamed = rename_project(vault.path(), "Growth/Q3", "growth/q3").unwrap();
        assert_eq!(renamed.info.slug, "Growth/q3");

        let slugs: Vec<String> = list_projects(vault.path())
            .unwrap()
            .into_iter()
            .map(|p| p.slug)
            .collect();
        // The echoed slug is one `list_projects` actually reports: the index's
        // `notes.project = ?` scope is case-sensitive, so frontmatter naming a
        // parent casing no directory carries would hide every note here.
        assert!(slugs.contains(&"Growth/q3".to_string()), "{slugs:?}");
        let moved = read_note_at(&vault.path().join("Growth").join("q3").join("plan.md"));
        assert_eq!(moved.routing.project(), "Growth/q3");
    }

    #[test]
    fn rename_project_rejects_a_parent_only_case_change() {
        let vault = tempdir().unwrap();
        write(
            vault.path(),
            "Growth/Q3",
            "n_aaaaaa",
            "2026-07-10",
            Some("plan"),
        );

        // `fs::rename` renames the leaf, so the destination's parent resolves to
        // the `Growth/` already on disk and nothing moves. Honouring the typed
        // casing would re-file the notes under a slug that does not exist.
        assert!(matches!(
            rename_project(vault.path(), "Growth/Q3", "growth/Q3"),
            Err(NoteError::InvalidField { .. })
        ));
        let untouched = read_note_at(&vault.path().join("Growth").join("Q3").join("plan.md"));
        assert_eq!(untouched.routing.project(), "Growth/Q3");
    }

    #[test]
    fn rename_project_rejects_the_name_it_already_has() {
        let vault = tempdir().unwrap();
        create_project(vault.path(), "Ops").unwrap();

        assert!(matches!(
            rename_project(vault.path(), "Ops", "Ops"),
            Err(NoteError::InvalidField { .. })
        ));
        // Reached through a differently-cased source too: both canonicalize to
        // the folder on disk, so this is still "no change".
        assert!(matches!(
            rename_project(vault.path(), "ops", "Ops"),
            Err(NoteError::InvalidField { .. })
        ));
        assert!(vault.path().join("Ops").is_dir());
    }

    #[test]
    fn rename_project_rejects_an_existing_target_in_any_casing() {
        let vault = tempdir().unwrap();
        write(
            vault.path(),
            "Growth",
            "n_aaaaaa",
            "2026-07-10",
            Some("plan"),
        );
        create_project(vault.path(), "Acme").unwrap();

        assert!(matches!(
            rename_project(vault.path(), "Growth", "Acme"),
            Err(NoteError::ProjectExists { project }) if project == "Acme"
        ));
        // A differently-cased collision reports the canonical on-disk slug.
        assert!(matches!(
            rename_project(vault.path(), "Growth", "acme"),
            Err(NoteError::ProjectExists { project }) if project == "Acme"
        ));
        // Nothing moved.
        assert!(vault.path().join("Growth").join("plan.md").is_file());
    }

    #[test]
    fn rename_project_rejects_a_missing_source() {
        let vault = tempdir().unwrap();
        assert!(matches!(
            rename_project(vault.path(), "Nope", "Acme"),
            Err(NoteError::MissingProject { project }) if project == "Nope"
        ));
    }

    #[test]
    fn rename_project_rejects_reserved_hidden_and_invalid_targets() {
        let vault = tempdir().unwrap();
        write(
            vault.path(),
            "Growth",
            "n_aaaaaa",
            "2026-07-10",
            Some("plan"),
        );

        for name in [
            "Inbox",
            "inbox",
            "sessions",
            "raw",
            "chats",
            "EBWebView",
            "_x",
            ".x",
            "con",
            "a:b",
            "Acme/",
            "",
            "a//b",
        ] {
            assert!(
                matches!(
                    rename_project(vault.path(), "Growth", name),
                    Err(NoteError::InvalidField { .. })
                ),
                "{name:?} should be rejected as a rename target"
            );
        }
        // Every rejection happened before the move.
        assert!(vault.path().join("Growth").join("plan.md").is_file());
    }

    #[test]
    fn rename_project_rejects_moving_a_project_inside_itself() {
        let vault = tempdir().unwrap();
        write(
            vault.path(),
            "Growth",
            "n_aaaaaa",
            "2026-07-10",
            Some("plan"),
        );

        assert!(matches!(
            rename_project(vault.path(), "Growth", "Growth/Sub"),
            Err(NoteError::InvalidField { .. })
        ));
        // The `/`-anchored check must not catch a merely similar sibling name.
        rename_project(vault.path(), "Growth", "Growthx").unwrap();
        assert!(vault.path().join("Growthx").is_dir());
    }

    #[test]
    fn rename_project_rejects_the_inbox_on_either_side() {
        let vault = tempdir().unwrap();
        write_inbox(vault.path(), "n_aaaaaa", "unfiled");
        write(
            vault.path(),
            "Growth",
            "n_bbbbbb",
            "2026-07-10",
            Some("plan"),
        );

        for (from, to) in [
            (INBOX, "Acme"),
            ("inbox", "Acme"),
            ("Growth", INBOX),
            ("Growth", "inbox"),
        ] {
            assert!(
                matches!(
                    rename_project(vault.path(), from, to),
                    Err(NoteError::InvalidField { .. })
                ),
                "{from:?} -> {to:?} should be rejected"
            );
        }
        assert!(vault.path().join(INBOX).join("unfiled.md").is_file());
        assert!(vault.path().join("Growth").join("plan.md").is_file());
    }

    #[test]
    fn rename_project_reparents_and_creates_the_missing_parent() {
        let vault = tempdir().unwrap();
        write(
            vault.path(),
            "Growth/Q3",
            "n_aaaaaa",
            "2026-07-10",
            Some("plan"),
        );

        let renamed = rename_project(vault.path(), "Growth/Q3", "Archive/Q3").unwrap();
        assert_eq!(renamed.info.slug, "Archive/Q3");

        let moved = read_note_at(&vault.path().join("Archive").join("Q3").join("plan.md"));
        assert_eq!(moved.routing.project(), "Archive/Q3");
        // The emptied source project survives the move of its child.
        assert!(vault.path().join("Growth").is_dir());
    }

    #[test]
    fn rename_project_adopts_an_existing_parents_casing() {
        let vault = tempdir().unwrap();
        create_project(vault.path(), "Archive").unwrap();
        write(
            vault.path(),
            "Growth",
            "n_aaaaaa",
            "2026-07-10",
            Some("plan"),
        );

        let renamed = rename_project(vault.path(), "Growth", "archive/2026").unwrap();
        assert_eq!(renamed.info.slug, "Archive/2026");
        assert!(vault.path().join("Archive").join("2026").is_dir());
        let moved = read_note_at(&vault.path().join("Archive").join("2026").join("plan.md"));
        assert_eq!(moved.routing.project(), "Archive/2026");
    }

    #[test]
    fn rename_project_carries_infra_and_unmanaged_files_untouched() {
        let vault = tempdir().unwrap();
        write(vault.path(), "Ops", "n_aaaaaa", "2026-07-10", Some("real"));
        let dir = vault.path().join("Ops");
        fs::write(dir.join(glossary::GLOSSARY_FILE), "terms: []\n").unwrap();
        fs::write(
            dir.join(routing_examples::ROUTING_EXAMPLES_FILE),
            "examples: []\n",
        )
        .unwrap();
        fs::write(dir.join(ledger::LEDGER_SNAPSHOT_FILE), "version: 1\n").unwrap();
        fs::write(dir.join("photo.png"), [1u8, 2, 3, 4]).unwrap();
        fs::write(dir.join("broken.md"), "no frontmatter here").unwrap();
        fs::create_dir(dir.join(".obsidian")).unwrap();

        // Unlike a delete, an unmanaged item is carried rather than a blocker.
        let renamed = rename_project(vault.path(), "Ops", "Operations").unwrap();
        assert!(renamed.failed_rewrites.is_empty());

        let moved = vault.path().join("Operations");
        assert!(!dir.exists());
        assert_eq!(
            fs::read_to_string(moved.join(glossary::GLOSSARY_FILE)).unwrap(),
            "terms: []\n"
        );
        assert_eq!(
            fs::read_to_string(moved.join(routing_examples::ROUTING_EXAMPLES_FILE)).unwrap(),
            "examples: []\n"
        );
        // The ledger snapshot rides along unrewritten, which is only safe
        // because it never names its own project.
        assert_eq!(
            fs::read_to_string(moved.join(ledger::LEDGER_SNAPSHOT_FILE)).unwrap(),
            "version: 1\n"
        );
        assert_eq!(fs::read(moved.join("photo.png")).unwrap(), [1u8, 2, 3, 4]);
        assert!(moved.join(".obsidian").is_dir());
        // The unparseable note rode along byte-identical; only parsed notes are
        // rewritten, and it is not one.
        assert_eq!(
            fs::read_to_string(moved.join("broken.md")).unwrap(),
            "no frontmatter here"
        );
        assert_eq!(renamed.renamed_notes.len(), 1);
    }

    #[test]
    fn rename_project_leaves_a_foreign_filed_note_alone() {
        let vault = tempdir().unwrap();
        // External-move residue: the file sits in `Growth/` but claims another
        // project. It moves with the folder; its filing is not ours to rewrite.
        let stray = note_in("Elsewhere", "n_aaaaaa", "2026-07-10", NoteType::Note);
        let dir = vault.path().join("Growth");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("stray.md"), stray.to_markdown()).unwrap();

        rename_project(vault.path(), "Growth", "Acme").unwrap();

        let moved = read_note_at(&vault.path().join("Acme").join("stray.md"));
        assert_eq!(moved.routing.project(), "Elsewhere");
    }

    #[test]
    fn rename_project_heals_frontmatter_that_case_differs_from_the_folder() {
        let vault = tempdir().unwrap();
        // A hand-edited note whose `project:` casing drifted from its folder is
        // still filed here, so it follows the rename.
        let drifted = note_in("growth/q3", "n_aaaaaa", "2026-07-10", NoteType::Note);
        let dir = vault.path().join("Growth").join("Q3");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("drift.md"), drifted.to_markdown()).unwrap();

        rename_project(vault.path(), "Growth", "Acme").unwrap();

        let moved = read_note_at(&vault.path().join("Acme").join("Q3").join("drift.md"));
        assert_eq!(moved.routing.project(), "Acme/q3");
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
        assert!(matches!(err, GlossaryOpError::Conflict { .. }));

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
        assert!(matches!(err, GlossaryOpError::MissingProject { .. }));
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
            GlossaryOpError::InvalidInput { field: "term", .. }
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
            GlossaryOpError::InvalidInput { field: "term", .. }
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
        assert!(matches!(err, GlossaryOpError::InvalidProject(_)));
    }

    #[test]
    fn upsert_glossary_term_vault_scope_writes_where_transcription_reads() {
        let vault = tempdir().unwrap();

        let write = upsert_glossary_term(
            vault.path(),
            None,
            glossary_term("MERIDIAN", "A systems-migration project.", &["meridian"]),
            glossary::OnConflict::Error,
        )
        .unwrap();

        assert!(write.created);
        assert_eq!(write.project, None);
        // The vault-wide glossary is the file the transcription pipeline loads
        // (`Glossary::load(kb_root)` in src-tauri/src/transcribe.rs), so the
        // term has to land at the root itself, not in any project folder.
        let loaded = glossary::Glossary::load(vault.path()).unwrap();
        assert_eq!(loaded.get("meridian").unwrap().term, "MERIDIAN");
    }

    #[test]
    fn upsert_glossary_term_vault_scope_creates_the_root() {
        // A fresh install may not have written the knowledge base yet; the
        // first vault-wide term must not fail on the missing parent.
        let parent = tempdir().unwrap();
        let vault_root = parent.path().join("does-not-exist-yet");

        upsert_glossary_term(
            &vault_root,
            None,
            glossary_term("MERIDIAN", "x.", &[]),
            glossary::OnConflict::Error,
        )
        .unwrap();

        assert!(glossary::glossary_path(&vault_root).exists());
    }

    #[test]
    fn list_glossary_terms_reads_both_scopes_in_file_order() {
        let vault = tempdir().unwrap();
        fs::create_dir(vault.path().join("Growth")).unwrap();
        for name in ["Alpha", "Beta", "Gamma"] {
            upsert_glossary_term(
                vault.path(),
                None,
                glossary_term(name, "Vault-wide.", &[]),
                glossary::OnConflict::Error,
            )
            .unwrap();
        }
        upsert_glossary_term(
            vault.path(),
            Some("Growth"),
            glossary_term("TeeTrack", "Project-scoped.", &[]),
            glossary::OnConflict::Error,
        )
        .unwrap();

        let vault_wide = list_glossary_terms(vault.path(), None).unwrap();
        assert_eq!(vault_wide.project, None);
        let order: Vec<&str> = vault_wide.terms.iter().map(|t| t.term.as_str()).collect();
        assert_eq!(order, vec!["Alpha", "Beta", "Gamma"]);

        // The two scopes are separate files and do not bleed into each other.
        let scoped = list_glossary_terms(vault.path(), Some("Growth")).unwrap();
        assert_eq!(scoped.project, Some("Growth".to_string()));
        assert_eq!(scoped.terms.len(), 1);
        assert_eq!(scoped.terms[0].term, "TeeTrack");
    }

    #[test]
    fn list_glossary_terms_with_no_file_is_empty_not_an_error() {
        let vault = tempdir().unwrap();
        fs::create_dir(vault.path().join("Growth")).unwrap();

        assert!(list_glossary_terms(vault.path(), None)
            .unwrap()
            .terms
            .is_empty());
        assert!(list_glossary_terms(vault.path(), Some("Growth"))
            .unwrap()
            .terms
            .is_empty());
    }

    #[test]
    fn list_glossary_terms_adopts_casing_and_rejects_a_missing_project() {
        let vault = tempdir().unwrap();
        fs::create_dir(vault.path().join("Growth")).unwrap();

        let listing = list_glossary_terms(vault.path(), Some("growth")).unwrap();
        assert_eq!(listing.project, Some("Growth".to_string()));

        let err = list_glossary_terms(vault.path(), Some("Nope")).unwrap_err();
        assert!(matches!(err, GlossaryOpError::MissingProject { .. }));
    }

    #[test]
    fn list_glossary_terms_surfaces_a_hand_edited_duplicate() {
        let vault = tempdir().unwrap();
        // Two entries colliding on the normalized term: the UI has to be able
        // to tell the user their hand-edited file is unreadable.
        fs::write(
            glossary::glossary_path(vault.path()),
            "terms:\n  - term: MERIDIAN\n    definition: First.\n  - term: meridian\n    definition: Second.\n",
        )
        .unwrap();

        let err = list_glossary_terms(vault.path(), None).unwrap_err();
        assert!(matches!(
            err,
            GlossaryOpError::Storage(glossary::GlossaryError::Duplicate { .. })
        ));
    }

    #[test]
    fn update_glossary_term_edits_in_place_and_renames() {
        let vault = tempdir().unwrap();
        for name in ["Alpha", "Beta", "Gamma"] {
            upsert_glossary_term(
                vault.path(),
                None,
                glossary_term(name, "Original.", &[]),
                glossary::OnConflict::Error,
            )
            .unwrap();
        }

        let write = update_glossary_term(
            vault.path(),
            None,
            "Beta",
            glossary_term("Bravo", "Renamed.", &["b"]),
        )
        .unwrap();

        assert!(!write.created);
        assert_eq!(write.term.term, "Bravo");
        // Renaming keeps the entry where it was rather than moving it to the end.
        let listing = list_glossary_terms(vault.path(), None).unwrap();
        let order: Vec<&str> = listing.terms.iter().map(|t| t.term.as_str()).collect();
        assert_eq!(order, vec!["Alpha", "Bravo", "Gamma"]);
    }

    #[test]
    fn update_glossary_term_rename_onto_another_term_conflicts() {
        let vault = tempdir().unwrap();
        fs::create_dir(vault.path().join("Growth")).unwrap();
        for name in ["MERIDIAN", "TeeTrack"] {
            upsert_glossary_term(
                vault.path(),
                Some("Growth"),
                glossary_term(name, "Original.", &[]),
                glossary::OnConflict::Error,
            )
            .unwrap();
        }

        let err = update_glossary_term(
            vault.path(),
            Some("Growth"),
            "TeeTrack",
            glossary_term("meridian", "Merged?", &[]),
        )
        .unwrap_err();

        assert!(matches!(err, GlossaryOpError::Conflict { .. }));
        // The rejected rename left the file untouched.
        let listing = list_glossary_terms(vault.path(), Some("Growth")).unwrap();
        assert_eq!(listing.terms.len(), 2);
    }

    #[test]
    fn update_glossary_term_missing_term_is_not_found() {
        let vault = tempdir().unwrap();
        let err = update_glossary_term(
            vault.path(),
            None,
            "nonexistent",
            glossary_term("Whatever", "Body.", &[]),
        )
        .unwrap_err();
        assert!(matches!(err, GlossaryOpError::NotFound { .. }));
    }

    #[test]
    fn update_glossary_term_validates_fields() {
        let vault = tempdir().unwrap();
        upsert_glossary_term(
            vault.path(),
            None,
            glossary_term("MERIDIAN", "x.", &[]),
            glossary::OnConflict::Error,
        )
        .unwrap();

        let err = update_glossary_term(
            vault.path(),
            None,
            "MERIDIAN",
            glossary_term("MERIDIAN", "   ", &[]),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            GlossaryOpError::InvalidInput {
                field: "definition",
                ..
            }
        ));
    }

    #[test]
    fn remove_glossary_term_removes_and_persists() {
        let vault = tempdir().unwrap();
        for name in ["MERIDIAN", "TeeTrack"] {
            upsert_glossary_term(
                vault.path(),
                None,
                glossary_term(name, "Original.", &[]),
                glossary::OnConflict::Error,
            )
            .unwrap();
        }

        let write = remove_glossary_term(vault.path(), None, "meridian").unwrap();

        assert_eq!(write.term.term, "MERIDIAN");
        assert!(!write.created);
        let listing = list_glossary_terms(vault.path(), None).unwrap();
        assert_eq!(listing.terms.len(), 1);
        assert_eq!(listing.terms[0].term, "TeeTrack");
    }

    #[test]
    fn remove_glossary_term_missing_term_is_not_found() {
        let vault = tempdir().unwrap();
        fs::create_dir(vault.path().join("Growth")).unwrap();

        let err = remove_glossary_term(vault.path(), Some("Growth"), "nonexistent").unwrap_err();
        assert!(matches!(err, GlossaryOpError::NotFound { .. }));
    }
}
