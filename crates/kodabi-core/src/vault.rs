//! Vault-level enumeration: discovering projects and listing notes directly
//! from disk. The Markdown files are the source of truth
//! (`docs/FRONTMATTER_SCHEMA.md`); the SQLite index is a derived cache that
//! nothing populates yet, so browsing reads the vault itself — a scan stays
//! O(notes-in-folder) parses, cheap for a per-project folder.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};

use crate::note::{self, Note, NoteEdit, NoteError, NoteId, NoteType, Result, INBOX};

/// A note found on disk: its absolute path, display title, and parsed content.
#[derive(Debug, Clone, PartialEq)]
pub struct ListedNote {
    pub path: PathBuf,
    /// De-slugged filename stem (`weekly-sync` → `weekly sync`). The title is
    /// not frontmatter — the filename is the slug of the title — so the stem
    /// is the only faithful source; an id-fallback filename stays as-is.
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
                title: title_from_path(&path),
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
    Ok(Some(ListedNote {
        note: merged,
        path: listed.path,
        title: listed.title,
    }))
}

/// Discovers project folders under the KB root, sorted by slug (so a parent
/// precedes its children). A directory is a project iff it or a descendant
/// holds ≥ 1 parseable direct `.md` note; ancestors of a qualifying folder are
/// included with their own (possibly zero) direct counts. Excluded outright:
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

/// Depth-first walk of one candidate project folder. Appends the folder (and
/// qualifying descendants) to `out`; returns whether the subtree holds any
/// parseable note, so note-free ancestors of a real project still qualify.
/// An unreadable directory (ACL-denied, a cloud-sync placeholder) skips just
/// that subtree — the directory-level mirror of the "one bad file must not
/// make the whole project unbrowsable" contract, so one locked folder can't
/// blank the entire sidebar.
fn collect_project(dir: &Path, slug: String, out: &mut Vec<ProjectInfo>) -> bool {
    let mut note_count = 0u32;
    let mut meeting_count = 0u32;
    let mut latest: Option<SystemTime> = None;
    let mut child_dirs = Vec::new();

    let Ok(entries) = fs::read_dir(dir) else {
        return false;
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

    let mut child_qualifies = false;
    for (path, child_slug) in child_dirs {
        child_qualifies |= collect_project(&path, child_slug, out);
    }

    let qualifies = note_count > 0 || child_qualifies;
    if qualifies {
        out.push(ProjectInfo {
            slug,
            note_count,
            meeting_count,
            last_activity: latest
                .map(|time| DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)),
        });
    }
    qualifies
}

fn is_md_file(path: &Path) -> bool {
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

/// De-slugged display title from the filename stem: hyphens become spaces
/// (`weekly-sync` → `weekly sync`); an id-fallback stem (`n_a1b2c3`) has no
/// hyphens and passes through unchanged.
fn title_from_path(path: &Path) -> String {
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
                date: "2026-07-12".to_string(),
                tags: vec![],
                body: String::new(),
            },
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
    fn list_projects_excludes_reserved_hidden_and_note_free_dirs() {
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
        for reserved in ["sessions", "raw", "EBWebView", ".obsidian", "_scratch"] {
            let dir = vault.path().join(reserved);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("decoy.md"), &decoy).unwrap();
        }
        fs::create_dir(vault.path().join("Empty")).unwrap();

        let projects = list_projects(vault.path()).unwrap();
        let slugs: Vec<&str> = projects.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(slugs, ["Ops"]);
    }

    #[test]
    fn list_projects_missing_root_yields_empty() {
        let vault = tempdir().unwrap();
        let missing = vault.path().join("never-created");
        assert!(list_projects(&missing).unwrap().is_empty());
    }
}
