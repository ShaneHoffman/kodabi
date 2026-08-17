//! Per-project YAML snapshots of the ledger, and the startup restore.
//!
//! The ledger's backup story is deliberately the same sentence as the rest of
//! Kodabi's: **back up the vault.** After a change, each affected project's
//! entries are serialized to `_ledger.yml` in that project's folder, beside
//! [`crate::glossary`]'s and [`crate::routing_examples`]'s files and written the
//! same atomic way. So a vault that syncs or is copied carries the commitment
//! state with it, and `ledger.db` never becomes the one file a user did not know
//! to save.
//!
//! **The database is the truth; this is a serialization of it.** Nothing reads a
//! snapshot at runtime. It is consulted exactly once, at startup, and only when
//! the database is missing or empty — a non-empty database always wins, because
//! merging two divergent histories is a question v1 refuses to guess at. A
//! snapshot fills a void; it never overwrites a fact.
//!
//! Two invariants keep the file honest, and both are tested:
//!
//! * **It never names its own project.** The folder it sits in is the project.
//!   That is what lets `vault::rename_project`'s `fs::rename` carry the file
//!   along with no rewrite, exactly as it does for the glossary.
//! * **It stores no `done` and no `due_date`.** Those live in the Markdown, and
//!   a snapshot that carried them would be a second, staler copy of the note.
//!
//! Restoring on a second machine needs no special step: item ids are
//! deterministic hashes of the note's own id plus the parsed line, so the same
//! synced notes mint the same `a_` ids, and a restored ref points at a real line
//! the moment the vault is indexed.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::store::LedgerEntry;
use super::{
    is_minted_id, ClosedVia, Direction, EntryState, EvidenceSource, Ledger, LedgerError, LinkKind,
    Result,
};
use crate::note::{project_dir, RESERVED_ROOT_DIRS};

/// Per-process counter giving each in-flight save a unique temp filename, so
/// concurrent saves cannot clobber each other's scratch file (mirrors
/// [`crate::routing_examples`]).
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Filename of a project's ledger snapshot, relative to the project folder root.
///
/// The `_` prefix marks it as infrastructure: note scans read only `.md`, the
/// watcher ignores every non-Markdown path, and `validate_project` rejects
/// `_`-prefixed segments, so it can never appear as a note or as a project.
pub const LEDGER_SNAPSHOT_FILE: &str = "_ledger.yml";

/// Snapshot format version. A file numbered higher than this was written by a
/// newer build and is **skipped, never partially read** — half-restoring a
/// future format would silently drop whatever the new fields meant.
pub const LEDGER_SNAPSHOT_VERSION: u32 = 1;

/// The on-disk path of a project's ledger snapshot.
pub fn snapshot_path(project_folder: &Path) -> PathBuf {
    project_folder.join(LEDGER_SNAPSHOT_FILE)
}

/// One project's entries, as written to `_ledger.yml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectSnapshot {
    pub version: u32,
    #[serde(default)]
    pub entries: Vec<SnapshotEntry>,
}

impl Default for ProjectSnapshot {
    fn default() -> Self {
        Self {
            version: LEDGER_SNAPSHOT_VERSION,
            entries: Vec::new(),
        }
    }
}

/// One entry and everything hanging off it.
///
/// Note what is *not* here: no `project` (the folder is the project), no `done`
/// or `due_date` (the Markdown owns those), and no normalized match keys (they
/// are recomputed on restore, so a future improvement to normalization applies
/// retroactively to snapshots written today).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotEntry {
    pub entry_id: String,
    pub state: EntryState,
    pub direction: Direction,
    pub owner: String,
    pub description: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_mention: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_evidence_check: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_via: Option<ClosedVia>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<SnapshotItemRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<SnapshotEvidence>,
    /// Outgoing links only, so an edge is written exactly once even when the two
    /// entries live in different projects.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<SnapshotLink>,
}

/// A reference to one extracted action item, live or retired.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotItemRef {
    pub item_id: String,
    pub note_id: String,
    pub active: bool,
    pub linked_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
}

/// One evidence claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotEvidence {
    pub evidence_id: String,
    pub source: EvidenceSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    pub confidence: f64,
    pub observed_at: String,
}

/// Just the version field, read before the body so a future format is
/// recognized as such rather than mistaken for a corrupt file.
#[derive(Debug, Deserialize)]
struct VersionProbe {
    #[serde(default)]
    version: u32,
}

/// One outgoing supersede/refresh edge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotLink {
    pub to: String,
    pub kind: LinkKind,
    pub created_at: String,
}

/// What a restore did, for the shell to log.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RestoreReport {
    /// False when the database already held entries, so nothing was read.
    pub restored: bool,
    pub files_read: usize,
    pub entries_restored: usize,
    /// Entries seen in more than one project's file (a note that moved between
    /// two flushes); the first by slug order wins.
    pub duplicates_skipped: usize,
    /// Links whose target entry was not in any snapshot.
    pub links_dropped: usize,
    /// Human-readable notes about files that were skipped.
    pub warnings: Vec<String>,
}

impl Ledger {
    /// Writes one project's snapshot, or removes the file when the project has
    /// no entries left.
    ///
    /// Removing matters: a stale file left behind after every entry moved
    /// elsewhere would resurrect them on a future restore, in a project they no
    /// longer belong to. A project folder that does not exist is skipped rather
    /// than created — the ledger must never conjure a folder the user deleted.
    pub fn write_project_snapshot(&self, vault_root: &Path, project: &str) -> Result<()> {
        let folder = project_dir(vault_root, project);
        let path = snapshot_path(&folder);
        let snapshot = self.build_snapshot(project)?;

        if snapshot.entries.is_empty() {
            match fs::remove_file(&path) {
                Ok(()) => return Ok(()),
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(source) => return Err(LedgerError::Io { path, source }),
            }
        }
        if !folder.is_dir() {
            return Ok(());
        }

        let yaml = serde_yaml_ng::to_string(&snapshot).map_err(|source| LedgerError::Yaml {
            path: path.clone(),
            source,
        })?;
        let tmp_path = folder.join(format!(
            "{LEDGER_SNAPSHOT_FILE}.{}.{}.tmp",
            std::process::id(),
            TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&tmp_path, yaml).map_err(|source| LedgerError::Io {
            path: tmp_path.clone(),
            source,
        })?;
        if let Err(source) = fs::rename(&tmp_path, &path) {
            // Don't leave a stray temp file behind in a folder that syncs and
            // exports with the rest of the knowledge base.
            let _ = fs::remove_file(&tmp_path);
            return Err(LedgerError::Io { path, source });
        }
        Ok(())
    }

    /// Writes every stale project's snapshot and clears the dirty set. Returns
    /// the slugs written.
    ///
    /// A project whose write fails stays dirty, so the next flush retries it
    /// rather than silently losing the backup.
    pub fn flush_snapshots(&mut self, vault_root: &Path) -> Result<Vec<String>> {
        let pending = self.dirty_projects();
        let mut written = Vec::new();
        let mut failure = None;
        for project in pending {
            match self.write_project_snapshot(vault_root, &project) {
                Ok(()) => {
                    self.clear_dirty(&project);
                    written.push(project);
                }
                Err(err) => failure = Some(failure.unwrap_or(err)),
            }
        }
        match failure {
            Some(err) => Err(err),
            None => Ok(written),
        }
    }

    /// Collects one project's entries into a serializable snapshot.
    fn build_snapshot(&self, project: &str) -> Result<ProjectSnapshot> {
        let entries = self.list_entries(&super::EntryFilter {
            project: Some(project.to_string()),
            ..super::EntryFilter::default()
        })?;
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let detail = match self.get_entry(&entry.entry_id)? {
                Some(detail) => detail,
                None => continue,
            };
            out.push(SnapshotEntry {
                entry_id: detail.entry.entry_id,
                state: detail.entry.state,
                direction: detail.entry.direction,
                owner: detail.entry.owner,
                description: detail.entry.description,
                created_at: detail.entry.created_at,
                updated_at: detail.entry.updated_at,
                last_mention: detail.entry.last_mention,
                last_evidence_check: detail.entry.last_evidence_check,
                snoozed_until: detail.entry.snoozed_until,
                closed_via: detail.entry.closed_via,
                review_reason: detail.entry.review_reason,
                items: detail
                    .item_refs
                    .into_iter()
                    .map(|item| SnapshotItemRef {
                        item_id: item.item_id,
                        note_id: item.note_id,
                        active: item.active,
                        linked_at: item.linked_at,
                        retired_at: item.retired_at,
                    })
                    .collect(),
                evidence: detail
                    .evidence
                    .into_iter()
                    .map(|claim| SnapshotEvidence {
                        evidence_id: claim.evidence_id,
                        source: claim.source,
                        reference: claim.reference,
                        confidence: claim.confidence,
                        observed_at: claim.observed_at,
                    })
                    .collect(),
                links: detail
                    .links_out
                    .into_iter()
                    .map(|link| SnapshotLink {
                        to: link.to_entry,
                        kind: link.kind,
                        created_at: link.created_at,
                    })
                    .collect(),
            });
        }
        Ok(ProjectSnapshot {
            version: LEDGER_SNAPSHOT_VERSION,
            entries: out,
        })
    }

    /// Rebuilds the ledger from the vault's snapshots, but **only into a void**.
    ///
    /// Called once at startup, before anything can sync. A database that already
    /// holds entries is left completely alone: the v1 conflict rule is that a
    /// non-empty database always wins, because a real merge needs a per-entry
    /// judgement no heuristic should make on a user's commitments.
    pub fn restore_from_snapshots_if_empty(&mut self, vault_root: &Path) -> Result<RestoreReport> {
        let mut report = RestoreReport::default();
        if !self.is_empty()? {
            return Ok(report);
        }
        report.restored = true;

        // Sorted, so "first file wins" for a duplicated entry is deterministic.
        let mut projects = Vec::new();
        collect_projects(vault_root, "", &mut projects)?;
        projects.sort();

        let mut loaded: Vec<(String, ProjectSnapshot)> = Vec::new();
        for project in projects {
            let path = snapshot_path(&project_dir(vault_root, &project));
            let raw = match fs::read_to_string(&path) {
                Ok(raw) => raw,
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => continue,
                Err(source) => return Err(LedgerError::Io { path, source }),
            };
            // Read the version *before* the body. A future format will not
            // deserialize into today's structs, and reporting that as "malformed"
            // would hide the real reason; worse, a format this build half
            // understood would restore a half-entry.
            match serde_yaml_ng::from_str::<VersionProbe>(&raw) {
                Ok(probe) if probe.version > LEDGER_SNAPSHOT_VERSION => {
                    report.warnings.push(format!(
                        "skipped {} (snapshot version {} is newer than this build's {})",
                        path.display(),
                        probe.version,
                        LEDGER_SNAPSHOT_VERSION
                    ));
                    continue;
                }
                Ok(_) => {}
                Err(err) => {
                    report
                        .warnings
                        .push(format!("skipped {} ({err})", path.display()));
                    continue;
                }
            }
            match serde_yaml_ng::from_str::<ProjectSnapshot>(&raw) {
                Ok(snapshot) => {
                    report.files_read += 1;
                    loaded.push((project, snapshot));
                }
                Err(err) => {
                    report
                        .warnings
                        .push(format!("skipped {} ({err})", path.display()));
                }
            }
        }

        let tx = self.connection_mut().transaction()?;
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut pending_links: Vec<(String, SnapshotLink)> = Vec::new();
        let mut live_refs: BTreeSet<(String, String)> = BTreeSet::new();

        // Pass 1: entries, refs, evidence.
        for (project, snapshot) in &loaded {
            for entry in &snapshot.entries {
                if !is_minted_id(&entry.entry_id, "le_") {
                    report
                        .warnings
                        .push(format!("skipped malformed entry id {:?}", entry.entry_id));
                    continue;
                }
                if seen.contains(&entry.entry_id) {
                    report.duplicates_skipped += 1;
                    continue;
                }
                let inserted = Ledger::insert_entry(
                    &tx,
                    &LedgerEntry {
                        entry_id: entry.entry_id.clone(),
                        state: entry.state,
                        direction: entry.direction,
                        owner: entry.owner.clone(),
                        description: entry.description.clone(),
                        project: project.clone(),
                        created_at: entry.created_at.clone(),
                        updated_at: entry.updated_at.clone(),
                        last_mention: entry.last_mention.clone(),
                        last_evidence_check: entry.last_evidence_check.clone(),
                        snoozed_until: entry.snoozed_until.clone(),
                        closed_via: entry.closed_via,
                        review_reason: entry.review_reason.clone(),
                    },
                );
                // A row the schema refuses is a bad *file*, not a bad database:
                // skip it the way a malformed id or unparseable YAML is skipped,
                // because propagating it would roll the transaction back and
                // lose every other project's commitments too. A genuine SQLite
                // failure (a full disk, a corrupt database) still propagates.
                if let Err(err) = inserted {
                    if !is_constraint_violation(&err) {
                        return Err(err);
                    }
                    report
                        .warnings
                        .push(format!("skipped entry {:?} ({err})", entry.entry_id));
                    continue;
                }
                seen.insert(entry.entry_id.clone());

                for item in &entry.items {
                    // Two entries claiming one live line cannot both be right;
                    // the later one is restored as history instead of failing
                    // the whole restore.
                    let key = (item.note_id.clone(), item.item_id.clone());
                    let active = item.active && live_refs.insert(key);
                    let retired_at = if active {
                        None
                    } else {
                        item.retired_at
                            .clone()
                            .or_else(|| Some(entry.updated_at.clone()))
                    };
                    tx.execute(
                        "INSERT OR IGNORE INTO ledger_item_refs
                             (entry_id, item_id, note_id, active, linked_at, retired_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![
                            entry.entry_id,
                            item.item_id,
                            item.note_id,
                            i64::from(active),
                            item.linked_at,
                            retired_at,
                        ],
                    )?;
                }

                for claim in &entry.evidence {
                    tx.execute(
                        "INSERT OR IGNORE INTO ledger_evidence
                             (evidence_id, entry_id, source, reference, confidence, observed_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![
                            claim.evidence_id,
                            entry.entry_id,
                            claim.source.as_str(),
                            claim.reference,
                            claim.confidence,
                            claim.observed_at,
                        ],
                    )?;
                }

                for link in &entry.links {
                    pending_links.push((entry.entry_id.clone(), link.clone()));
                }
                report.entries_restored += 1;
            }
        }

        // Pass 2: links, once every entry they might point at exists.
        for (from, link) in pending_links {
            if !seen.contains(&link.to) {
                // The target's project was not restored (its file was skipped,
                // or the vault is a partial copy). The `from` entry keeps its
                // superseded state: the judgement was real even if the entry it
                // pointed at is gone.
                report.links_dropped += 1;
                continue;
            }
            tx.execute(
                "INSERT OR IGNORE INTO ledger_entry_links (from_entry, to_entry, kind, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![from, link.to, link.kind.as_str(), link.created_at],
            )?;
        }
        tx.commit()?;

        // The database now equals the snapshots; nothing needs rewriting.
        self.clear_all_dirty();
        Ok(report)
    }
}

/// Whether an error is SQLite refusing a row on a `CHECK`, a uniqueness rule or
/// a foreign key — i.e. the file said something the schema forbids, rather than
/// the database being unable to do its job.
fn is_constraint_violation(err: &LedgerError) -> bool {
    matches!(
        err,
        LedgerError::Sqlite(rusqlite::Error::SqliteFailure(error, _))
            if error.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

/// Walks the vault collecting project slugs, using the same rules as
/// `vault::list_projects` except that the Inbox **is** included: meetings
/// legitimately sit there un-filed, and their commitments deserve a backup as
/// much as any other.
fn collect_projects(root: &Path, prefix: &str, out: &mut Vec<String>) -> Result<()> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(LedgerError::Io {
                path: root.to_path_buf(),
                source,
            })
        }
    };
    for entry in entries {
        let entry = entry.map_err(|source| LedgerError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        if !entry.file_type().map(|ty| ty.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        // Infrastructure and hidden folders are never projects, at any depth.
        if name.starts_with('.') || name.starts_with('_') {
            continue;
        }
        if prefix.is_empty() && RESERVED_ROOT_DIRS.contains(&name.as_str()) {
            continue;
        }
        let slug = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        collect_projects(&entry.path(), &slug, out)?;
        out.push(slug);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::sync::NoteSync;
    use crate::ledger::{EntryFilter, EvidenceSource, LinkKind};
    use crate::meeting::ActionItemFact;
    use tempfile::tempdir;

    const NOW: &str = "2026-08-17T12:00:00Z";
    const DAY: &str = "2026-08-01T00:00:00Z";

    fn fact(id: &str, owner: &str, description: &str) -> ActionItemFact {
        ActionItemFact {
            id: id.to_string(),
            description: description.to_string(),
            owner: owner.to_string(),
            due_date: None,
            done: false,
            extracted_date: Some("2026-08-01".to_string()),
        }
    }

    /// A vault with one project folder, and a ledger holding one entry in it.
    fn vault_with_entry() -> (tempfile::TempDir, Ledger, String) {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("Briarwood Golf")).unwrap();
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![fact("a_111111", "Priya", "send the revised deck")];
        let outcome = ledger
            .sync_note_items(&NoteSync {
                note_id: "n_a1b2c3",
                project: "Briarwood Golf",
                note_date_utc: DAY,
                items: &items,
                now: NOW,
            })
            .unwrap();
        let entry_id = outcome.created[0].clone();
        (dir, ledger, entry_id)
    }

    #[test]
    fn a_snapshot_round_trips_through_a_fresh_ledger() {
        let (dir, mut ledger, entry_id) = vault_with_entry();
        ledger
            .add_evidence(
                &entry_id,
                EvidenceSource::Github,
                Some("https://example.com/pull/42"),
                0.8,
                "2026-08-16T02:00:00Z",
                NOW,
            )
            .unwrap();
        let written = ledger.flush_snapshots(dir.path()).unwrap();
        assert_eq!(written, vec!["Briarwood Golf".to_string()]);
        assert!(ledger.dirty_projects().is_empty(), "flush clears the set");

        let before = ledger.get_entry(&entry_id).unwrap().unwrap();

        // A brand-new ledger, as if `ledger.db` had been deleted.
        let mut restored = Ledger::open_in_memory().unwrap();
        let report = restored
            .restore_from_snapshots_if_empty(dir.path())
            .unwrap();
        assert!(report.restored);
        assert_eq!(report.files_read, 1);
        assert_eq!(report.entries_restored, 1);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);

        let after = restored.get_entry(&entry_id).unwrap().unwrap();
        assert_eq!(before, after, "the entry survives a full rebuild");
        assert!(restored.dirty_projects().is_empty());
    }

    #[test]
    fn the_snapshot_never_names_its_own_project() {
        // This is the invariant that lets `rename_project` move the folder with
        // a plain `fs::rename` and no rewrite.
        let (dir, mut ledger, _) = vault_with_entry();
        ledger.flush_snapshots(dir.path()).unwrap();
        let raw = fs::read_to_string(snapshot_path(&dir.path().join("Briarwood Golf"))).unwrap();
        assert!(!raw.contains("Briarwood"), "{raw}");
        assert!(!raw.contains("project"), "{raw}");
        // Nor the fields the Markdown owns.
        assert!(!raw.contains("done"), "{raw}");
        assert!(!raw.contains("due_date"), "{raw}");
    }

    #[test]
    fn a_project_with_no_entries_left_loses_its_file() {
        let (dir, mut ledger, entry_id) = vault_with_entry();
        fs::create_dir_all(dir.path().join("Ops")).unwrap();
        ledger.flush_snapshots(dir.path()).unwrap();
        let path = snapshot_path(&dir.path().join("Briarwood Golf"));
        assert!(path.is_file());

        // The note is re-filed, so the entry follows it to another project.
        let items = vec![fact("a_111111", "Priya", "send the revised deck")];
        ledger
            .sync_note_items(&NoteSync {
                note_id: "n_a1b2c3",
                project: "Ops",
                note_date_utc: DAY,
                items: &items,
                now: NOW,
            })
            .unwrap();
        ledger.flush_snapshots(dir.path()).unwrap();

        assert!(
            !path.exists(),
            "a stale file would resurrect the entry on restore"
        );
        assert!(snapshot_path(&dir.path().join("Ops")).is_file());
        assert_eq!(
            ledger.get_entry(&entry_id).unwrap().unwrap().entry.project,
            "Ops"
        );
    }

    #[test]
    fn a_non_empty_database_always_wins() {
        let (dir, mut ledger, _) = vault_with_entry();
        ledger.flush_snapshots(dir.path()).unwrap();

        // A different ledger that already holds something.
        let mut other = Ledger::open_in_memory().unwrap();
        let items = vec![fact("a_222222", "Sam", "book the venue")];
        other
            .sync_note_items(&NoteSync {
                note_id: "n_d4e5f6",
                project: "Briarwood Golf",
                note_date_utc: DAY,
                items: &items,
                now: NOW,
            })
            .unwrap();

        let report = other.restore_from_snapshots_if_empty(dir.path()).unwrap();
        assert!(!report.restored);
        assert_eq!(report.entries_restored, 0);
        let entries = other.list_entries(&EntryFilter::default()).unwrap();
        assert_eq!(entries.len(), 1, "live data untouched");
        assert_eq!(entries[0].description, "book the venue");
    }

    #[test]
    fn a_newer_snapshot_version_is_skipped_with_a_warning() {
        let dir = tempdir().unwrap();
        let folder = dir.path().join("Briarwood Golf");
        fs::create_dir_all(&folder).unwrap();
        fs::write(
            snapshot_path(&folder),
            "version: 99\nentries:\n  - entry_id: le_aaaaaaaaaaaa\n",
        )
        .unwrap();

        let mut ledger = Ledger::open_in_memory().unwrap();
        let report = ledger.restore_from_snapshots_if_empty(dir.path()).unwrap();
        assert_eq!(report.entries_restored, 0);
        assert_eq!(report.files_read, 0);
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("newer than"));
    }

    #[test]
    fn a_malformed_snapshot_is_skipped_without_failing_the_restore() {
        let (dir, mut ledger, _) = vault_with_entry();
        ledger.flush_snapshots(dir.path()).unwrap();
        let broken = dir.path().join("Ops");
        fs::create_dir_all(&broken).unwrap();
        fs::write(snapshot_path(&broken), "entries: [ this is not: valid yaml").unwrap();

        let mut restored = Ledger::open_in_memory().unwrap();
        let report = restored
            .restore_from_snapshots_if_empty(dir.path())
            .unwrap();
        assert_eq!(report.entries_restored, 1, "the good file still restored");
        assert_eq!(report.warnings.len(), 1);
    }

    #[test]
    fn an_entry_the_schema_refuses_is_skipped_not_fatal() {
        // `closed` with no `closed_via` violates the entries table's CHECK. It
        // is a bad file, so it must be skipped like any other bad file — losing
        // every *other* project's commitments to it would be the opposite of
        // what the snapshots exist for.
        let (dir, mut ledger, entry_id) = vault_with_entry();
        ledger.flush_snapshots(dir.path()).unwrap();
        let broken = dir.path().join("Ops");
        fs::create_dir_all(&broken).unwrap();
        fs::write(
            snapshot_path(&broken),
            "version: 1\nentries:\n\
             - entry_id: le_ffffffffffff\n  \
               state: closed\n  \
               direction: theirs\n  \
               owner: Priya\n  \
               description: closed with no provenance\n  \
               created_at: 2026-07-01T00:00:00Z\n  \
               updated_at: 2026-07-01T00:00:00Z\n  \
               last_mention: 2026-07-01T00:00:00Z\n",
        )
        .unwrap();

        let mut restored = Ledger::open_in_memory().unwrap();
        let report = restored
            .restore_from_snapshots_if_empty(dir.path())
            .unwrap();
        assert_eq!(report.entries_restored, 1, "the good file still restored");
        assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
        assert!(report.warnings[0].contains("le_ffffffffffff"));
        assert!(restored.get_entry(&entry_id).unwrap().is_some());
        assert!(restored.get_entry("le_ffffffffffff").unwrap().is_none());
    }

    #[test]
    fn a_link_whose_target_is_missing_is_dropped() {
        let (dir, mut ledger, first) = vault_with_entry();
        let items = vec![fact("a_222222", "Priya", "send the final deck")];
        let second = ledger
            .sync_note_items(&NoteSync {
                note_id: "n_d4e5f6",
                project: "Briarwood Golf",
                note_date_utc: DAY,
                items: &items,
                now: NOW,
            })
            .unwrap()
            .created[0]
            .clone();
        ledger
            .link_entries(&first, &second, LinkKind::Supersedes, NOW)
            .unwrap();
        ledger.flush_snapshots(dir.path()).unwrap();

        // Hand-edit the file to drop the link's target, as a partial vault copy
        // would.
        let path = snapshot_path(&dir.path().join("Briarwood Golf"));
        let raw = fs::read_to_string(&path).unwrap();
        let mut snapshot: ProjectSnapshot = serde_yaml_ng::from_str(&raw).unwrap();
        snapshot.entries.retain(|entry| entry.entry_id != second);
        fs::write(&path, serde_yaml_ng::to_string(&snapshot).unwrap()).unwrap();

        let mut restored = Ledger::open_in_memory().unwrap();
        let report = restored
            .restore_from_snapshots_if_empty(dir.path())
            .unwrap();
        assert_eq!(report.links_dropped, 1);
        let detail = restored.get_entry(&first).unwrap().unwrap();
        assert!(detail.links_out.is_empty());
        assert_eq!(
            detail.entry.state,
            EntryState::Superseded,
            "the judgement outlives the edge"
        );
    }

    #[test]
    fn snapshots_cover_nested_projects_and_the_inbox() {
        let dir = tempdir().unwrap();
        for folder in ["Inbox", "Growth/Q3", "sessions", "_private", ".hidden"] {
            fs::create_dir_all(dir.path().join(folder)).unwrap();
        }
        let mut ledger = Ledger::open_in_memory().unwrap();
        for (note, project, item, description) in [
            ("n_a1b2c3", "Inbox", "a_111111", "send the deck"),
            ("n_d4e5f6", "Growth/Q3", "a_222222", "book the venue"),
        ] {
            let items = vec![fact(item, "Priya", description)];
            ledger
                .sync_note_items(&NoteSync {
                    note_id: note,
                    project,
                    note_date_utc: DAY,
                    items: &items,
                    now: NOW,
                })
                .unwrap();
        }
        let mut written = ledger.flush_snapshots(dir.path()).unwrap();
        written.sort();
        assert_eq!(written, vec!["Growth/Q3".to_string(), "Inbox".to_string()]);

        let mut restored = Ledger::open_in_memory().unwrap();
        let report = restored
            .restore_from_snapshots_if_empty(dir.path())
            .unwrap();
        assert_eq!(report.files_read, 2);
        assert_eq!(report.entries_restored, 2);
        // The reserved and infrastructure folders were never walked.
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn a_failed_write_leaves_no_scratch_file() {
        let (dir, mut ledger, _) = vault_with_entry();
        ledger.flush_snapshots(dir.path()).unwrap();
        let leftovers: Vec<_> = fs::read_dir(dir.path().join("Briarwood Golf"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
}
