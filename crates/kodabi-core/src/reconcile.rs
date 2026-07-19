//! One-pass convergence of the note index to the vault on disk.
//!
//! The Markdown files are the source of truth; the SQLite index is a derived
//! cache (`docs/FRONTMATTER_SCHEMA.md`, `index` module docs). A file watcher can
//! miss events, and files change while the app is closed, so rather than react
//! to individual create/modify/delete events this module runs a single
//! *reconcile* pass: scan every note, upsert each by its stable frontmatter
//! `id`, then delete index rows whose file is gone. Run on a debounced watcher
//! burst, at startup, and as the body of the rebuild command, it always
//! converges the index to what is on disk.
//!
//! Reconcile is **rows only** — it never embeds. `upsert_note` invalidates a
//! note's chunk vectors when its title or body changes (a pure move keeps
//! them), so after a pass the caller re-embeds whatever is missing via
//! [`NoteIndex::note_ids_missing_embeddings`]. Keeping the (slow, feature-gated)
//! embedding out of this file keeps it pure and lets the caller run the embed
//! off the index lock.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::index::{IndexedNote, NoteIndex, NoteRow};
use crate::note::{NoteError, INBOX};
use crate::vault::{self, ListedNote};

/// What one [`reconcile`] pass did, for logging and tests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Notes inserted or updated (content differed from the index).
    pub upserted: usize,
    /// Notes already current — skipped by the unchanged fast-path.
    pub unchanged: usize,
    /// Rows deleted because their file left the vault.
    pub deleted: usize,
    /// `.md` files that failed to read or parse this pass (likely mid-write).
    /// Their existing index rows are spared so a partial write can't drop a note.
    pub skipped: Vec<PathBuf>,
    /// Files skipped because another file already claimed the same `id` this
    /// pass (a stray copy). The first file, in scan order, wins.
    pub duplicates: Vec<(String, PathBuf)>,
}

/// A failure that aborts a whole reconcile pass. Per-file parse errors are *not*
/// among these — an unreadable or corrupt note is recorded in
/// [`ReconcileReport::skipped`] and the pass continues. Only an unreadable vault
/// root or a database error stops it.
#[derive(Debug, thiserror::Error)]
pub enum ReconcileError {
    /// Enumerating projects or scanning a folder failed at the filesystem level.
    #[error(transparent)]
    Vault(#[from] NoteError),
    /// A database read or write failed.
    #[error(transparent)]
    Index(#[from] crate::index::IndexError),
}

/// Drops the whole index, then reconciles it from files alone — the "files are
/// truth" rebuild. Every note is re-read and re-upserted (and its vectors, being
/// cleared, are recomputed by the caller's follow-up embed sweep).
pub fn rebuild(
    vault_root: &Path,
    index: &mut NoteIndex,
) -> Result<ReconcileReport, ReconcileError> {
    index.clear()?;
    reconcile(vault_root, index)
}

/// Converges `index` to the notes under `vault_root` in a single pass: upsert
/// every parsed note keyed by its stable `id`, then delete rows whose file is
/// gone. Reconciles by `id`, never by path, so a moved or re-routed note updates
/// its existing row rather than churning a delete + insert.
pub fn reconcile(
    vault_root: &Path,
    index: &mut NoteIndex,
) -> Result<ReconcileReport, ReconcileError> {
    let mut report = ReconcileReport::default();

    // Inbox first, then every discovered project, sorted — the same walk as
    // `vault::find_note_anywhere`, so "first file wins" for a duplicate id is
    // deterministic.
    let mut projects = vec![INBOX.to_string()];
    projects.extend(
        vault::list_projects(vault_root)?
            .into_iter()
            .map(|p| p.slug),
    );

    // Ids upserted this pass (the stale-sweep keeps these); rel paths that parsed
    // this pass (a stale row whose path was re-parsed under a *different* id is
    // genuinely stale — see the sweep below).
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut parsed_paths: HashSet<String> = HashSet::new();

    for project in &projects {
        let scan = vault::scan_project_notes(vault_root, project)?;
        for listed in scan.notes {
            let rel = rel_path(vault_root, &listed);
            parsed_paths.insert(rel.clone());

            if !seen_ids.insert(listed.note.id.as_str().to_string()) {
                report
                    .duplicates
                    .push((listed.note.id.as_str().to_string(), listed.path));
                continue;
            }

            let indexed = IndexedNote::from_note(&listed.note, &listed.title, &rel);
            // Fast path: an unchanged note skips the write entirely, so a
            // redundant reconcile touches no rows (no FTS trigger churn, no
            // spurious chunk invalidation).
            match index.get_note(&indexed.id)? {
                Some(row) if row_matches(&row, &indexed) => report.unchanged += 1,
                _ => {
                    index.upsert_note(&indexed)?;
                    report.upserted += 1;
                }
            }
        }
        report.skipped.extend(scan.skipped);
    }

    // Stale sweep: any indexed id not seen this pass whose file is truly gone.
    // A row is *spared* when its stored file still exists on disk but was not
    // parsed this pass (mid-write / momentarily corrupt) — the next clean pass
    // converges it. A file that now parses under a different id counts as
    // re-parsed, so the old row is deleted.
    for (id, path) in index.note_ids_and_paths()? {
        if seen_ids.contains(&id) {
            continue;
        }
        let file_still_present = vault_root.join(&path).is_file() && !parsed_paths.contains(&path);
        if file_still_present {
            continue;
        }
        if index.delete_note(&id)? {
            report.deleted += 1;
        }
    }

    Ok(report)
}

/// The KB-relative, forward-slashed path the index stores for a note — the same
/// form the Tauri write path builds (`note_cmds`), so a watcher-driven reconcile
/// and an in-app write agree on a note's `path`.
fn rel_path(vault_root: &Path, listed: &ListedNote) -> String {
    listed
        .path
        .strip_prefix(vault_root)
        .unwrap_or(&listed.path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Whether the indexed row already equals the freshly scanned note, so the
/// upsert can be skipped. Compares every persisted field except the derived
/// `date_utc` (a function of `date`); tags are compared as sets because the
/// index stores them sorted while frontmatter order is arbitrary.
fn row_matches(row: &NoteRow, note: &IndexedNote) -> bool {
    row.id == note.id
        && row.path == note.path
        && row.title == note.title
        && row.note_type == note.note_type
        && row.project == note.project
        && row.date == note.date
        && row.source == note.source
        && row.confidence == note.confidence
        && row.body == note.body
        && tags_match(&row.tags, &note.tags)
}

/// `row_tags` is already sorted (the index orders tags); compare against a sorted
/// copy of the scanned note's tags so frontmatter order doesn't force a rewrite.
fn tags_match(row_tags: &[String], note_tags: &[String]) -> bool {
    if row_tags.len() != note_tags.len() {
        return false;
    }
    let mut scanned = note_tags.to_vec();
    scanned.sort();
    row_tags == scanned.as_slice()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::EmbeddedChunk;
    use crate::note::{write_note, Note, NoteId, NoteType, Routing, Source, Tag};
    use tempfile::tempdir;

    /// Writes a note into `<vault>/<project>` and returns its absolute path. The
    /// project `Inbox` files into the Inbox; a routed project files there.
    fn write(vault: &Path, id: &str, project: &str, body: &str, tags: &[&str]) -> PathBuf {
        let routing = if project == INBOX {
            Routing::Routed {
                project: INBOX.to_string(),
                confidence: 0.4,
            }
        } else {
            Routing::Routed {
                project: project.to_string(),
                confidence: 0.9,
            }
        };
        let note = Note::new(
            NoteId::parse(id).unwrap(),
            NoteType::Note,
            routing,
            "2026-07-11",
            tags.iter().map(|t| Tag::parse(t).unwrap()).collect(),
            Source::parse("manual").unwrap(),
            body,
        )
        .unwrap();
        write_note(vault, &note, None).unwrap()
    }

    fn ids(index: &NoteIndex) -> Vec<String> {
        index
            .note_ids_and_paths()
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    #[test]
    fn indexes_inbox_and_projects_with_relative_paths() {
        let dir = tempdir().unwrap();
        let vault = dir.path();
        write(vault, "n_inbox1", INBOX, "inbox body", &[]);
        write(vault, "n_proj01", "Acme", "acme body", &["a-tag"]);
        write(vault, "n_deep01", "Growth/Q3", "deep body", &[]);

        let mut index = NoteIndex::open_in_memory().unwrap();
        let report = reconcile(vault, &mut index).unwrap();

        assert_eq!(report.upserted, 3);
        assert_eq!(report.unchanged, 0);
        assert_eq!(report.deleted, 0);
        assert_eq!(ids(&index), vec!["n_deep01", "n_inbox1", "n_proj01"]);

        // Inbox note stores a null project; paths are KB-relative, forward-slashed.
        let inbox = index.get_note("n_inbox1").unwrap().unwrap();
        assert_eq!(inbox.project, None);
        assert!(inbox.path.starts_with("Inbox/"), "{}", inbox.path);
        let deep = index.get_note("n_deep01").unwrap().unwrap();
        assert_eq!(deep.project.as_deref(), Some("Growth/Q3"));
        assert!(deep.path.starts_with("Growth/Q3/"), "{}", deep.path);
        assert!(!deep.path.contains('\\'), "{}", deep.path);
    }

    #[test]
    fn a_second_pass_is_all_unchanged_and_preserves_chunks() {
        let dir = tempdir().unwrap();
        let vault = dir.path();
        write(vault, "n_keep01", "Acme", "stable body", &["t"]);

        let mut index = NoteIndex::open_in_memory().unwrap();
        reconcile(vault, &mut index).unwrap();

        // Stand in for the embed pipeline: give the note a chunk vector.
        index
            .set_note_chunks(
                "n_keep01",
                &[EmbeddedChunk {
                    text: "stable body".to_string(),
                    embedding: vec![0.0; crate::index::EMBEDDING_DIM],
                }],
            )
            .unwrap();

        let report = reconcile(vault, &mut index).unwrap();
        assert_eq!(report.unchanged, 1);
        assert_eq!(report.upserted, 0);
        // The unchanged fast-path never touched the row, so the chunk survives.
        assert!(index.note_has_chunks("n_keep01").unwrap());
    }

    #[test]
    fn an_external_edit_updates_the_row_and_drops_stale_chunks() {
        let dir = tempdir().unwrap();
        let vault = dir.path();
        let path = write(vault, "n_edit01", "Acme", "original body", &[]);

        let mut index = NoteIndex::open_in_memory().unwrap();
        reconcile(vault, &mut index).unwrap();
        index
            .set_note_chunks(
                "n_edit01",
                &[EmbeddedChunk {
                    text: "original body".to_string(),
                    embedding: vec![0.0; crate::index::EMBEDDING_DIM],
                }],
            )
            .unwrap();

        // Rewrite the same note (same id) with a new body, in place.
        let edited = Note::new(
            NoteId::parse("n_edit01").unwrap(),
            NoteType::Note,
            Routing::Routed {
                project: "Acme".to_string(),
                confidence: 0.9,
            },
            "2026-07-11",
            vec![],
            Source::parse("manual").unwrap(),
            "rewritten body",
        )
        .unwrap();
        crate::note::save_note_at(&path, &edited).unwrap();

        let report = reconcile(vault, &mut index).unwrap();
        assert_eq!(report.upserted, 1);
        assert_eq!(
            index.get_note("n_edit01").unwrap().unwrap().body,
            "rewritten body"
        );
        // Body changed → the note needs re-embedding.
        assert_eq!(
            index.note_ids_missing_embeddings().unwrap(),
            vec!["n_edit01".to_string()]
        );
    }

    #[test]
    fn an_external_move_keeps_the_row_and_its_chunks() {
        let dir = tempdir().unwrap();
        let vault = dir.path();
        let original = write(vault, "n_move01", "Acme", "moved body", &[]);

        let mut index = NoteIndex::open_in_memory().unwrap();
        reconcile(vault, &mut index).unwrap();
        index
            .set_note_chunks(
                "n_move01",
                &[EmbeddedChunk {
                    text: "moved body".to_string(),
                    embedding: vec![0.0; crate::index::EMBEDDING_DIM],
                }],
            )
            .unwrap();

        // Simulate an external move: same id + body, new folder, frontmatter
        // still names the old project (frontmatter wins — the row's project is
        // unchanged, only its path moves).
        std::fs::remove_file(&original).unwrap();
        write(vault, "n_move01", "Growth", "moved body", &[]);

        let report = reconcile(vault, &mut index).unwrap();
        // One row throughout; path updated, chunk preserved (title+body same).
        assert_eq!(ids(&index), vec!["n_move01"]);
        assert_eq!(report.deleted, 0);
        let row = index.get_note("n_move01").unwrap().unwrap();
        assert!(row.path.starts_with("Growth/"), "{}", row.path);
        assert!(index.note_has_chunks("n_move01").unwrap());
    }

    #[test]
    fn a_deleted_file_deletes_its_row() {
        let dir = tempdir().unwrap();
        let vault = dir.path();
        let path = write(vault, "n_gone01", "Acme", "doomed body", &[]);
        write(vault, "n_stay01", "Acme", "surviving body", &[]);

        let mut index = NoteIndex::open_in_memory().unwrap();
        reconcile(vault, &mut index).unwrap();

        std::fs::remove_file(&path).unwrap();
        let report = reconcile(vault, &mut index).unwrap();

        assert_eq!(report.deleted, 1);
        assert_eq!(ids(&index), vec!["n_stay01"]);
    }

    #[test]
    fn a_mid_write_unparseable_file_is_skipped_and_spared_then_deletes_when_gone() {
        let dir = tempdir().unwrap();
        let vault = dir.path();
        // A sibling keeps the project folder alive so the corrupt file is
        // actually scanned (and lands in `skipped`).
        let path = write(vault, "n_part01", "Acme", "good body", &[]);
        write(vault, "n_sib001", "Acme", "sibling body", &[]);

        let mut index = NoteIndex::open_in_memory().unwrap();
        reconcile(vault, &mut index).unwrap();
        assert_eq!(ids(&index), vec!["n_part01", "n_sib001"]);

        // Overwrite with unparseable garbage (no frontmatter) — a mid-write the
        // watcher observes. The row must survive and the file be reported skipped.
        std::fs::write(&path, "garbage with no frontmatter").unwrap();
        let report = reconcile(vault, &mut index).unwrap();
        assert_eq!(report.deleted, 0);
        assert_eq!(report.upserted, 0);
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(
            ids(&index),
            vec!["n_part01", "n_sib001"],
            "a partial write must not drop the note"
        );

        // Once the file is truly gone, the row is deleted.
        std::fs::remove_file(&path).unwrap();
        let report = reconcile(vault, &mut index).unwrap();
        assert_eq!(report.deleted, 1);
        assert_eq!(ids(&index), vec!["n_sib001"]);
    }

    #[test]
    fn a_corrupt_note_in_a_single_note_folder_is_still_spared() {
        // The folder drops out of `list_projects` (no parseable note left), so
        // the file is never scanned — the row is spared by the on-disk existence
        // check, not the skipped set. Guards the "files are truth" grace for the
        // common one-note-per-folder case.
        let dir = tempdir().unwrap();
        let vault = dir.path();
        let path = write(vault, "n_solo01", "Acme", "good body", &[]);

        let mut index = NoteIndex::open_in_memory().unwrap();
        reconcile(vault, &mut index).unwrap();
        assert_eq!(ids(&index), vec!["n_solo01"]);

        std::fs::write(&path, "garbage with no frontmatter").unwrap();
        let report = reconcile(vault, &mut index).unwrap();
        assert_eq!(report.deleted, 0);
        assert_eq!(
            ids(&index),
            vec!["n_solo01"],
            "a corrupt sole note must not be dropped"
        );

        std::fs::remove_file(&path).unwrap();
        let report = reconcile(vault, &mut index).unwrap();
        assert_eq!(report.deleted, 1);
        assert!(ids(&index).is_empty());
    }

    #[test]
    fn a_duplicate_id_indexes_once_and_is_reported() {
        let dir = tempdir().unwrap();
        let vault = dir.path();
        // Two files, same id, in different projects (an external copy).
        write(vault, "n_dup001", "Acme", "first body", &[]);
        write(vault, "n_dup001", "Growth", "second body", &[]);

        let mut index = NoteIndex::open_in_memory().unwrap();
        let report = reconcile(vault, &mut index).unwrap();

        assert_eq!(ids(&index), vec!["n_dup001"]);
        assert_eq!(report.duplicates.len(), 1);
        assert_eq!(report.duplicates[0].0, "n_dup001");
    }

    #[test]
    fn rebuild_repopulates_from_scratch_after_tampering() {
        let dir = tempdir().unwrap();
        let vault = dir.path();
        write(vault, "n_a00001", "Acme", "a body", &[]);
        write(vault, "n_b00002", "Growth", "b body", &[]);

        let mut index = NoteIndex::open_in_memory().unwrap();
        reconcile(vault, &mut index).unwrap();

        // Tamper via the public API: drop one row, and overwrite the other's
        // body with a stale value the on-disk file no longer has.
        index.delete_note("n_a00001").unwrap();
        let stale = Note::new(
            NoteId::parse("n_b00002").unwrap(),
            NoteType::Note,
            Routing::Routed {
                project: "Growth".to_string(),
                confidence: 0.9,
            },
            "2026-07-11",
            vec![],
            Source::parse("manual").unwrap(),
            "stale body",
        )
        .unwrap();
        index
            .upsert_note(&IndexedNote::from_note(
                &stale,
                "n_b00002",
                "Growth/n_b00002.md",
            ))
            .unwrap();

        let report = rebuild(vault, &mut index).unwrap();
        assert_eq!(report.upserted, 2);
        assert_eq!(ids(&index), vec!["n_a00001", "n_b00002"]);
        assert_eq!(index.get_note("n_b00002").unwrap().unwrap().body, "b body");
    }
}
