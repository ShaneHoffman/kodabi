//! Per-project meeting-category memory — the classification correction loop.
//!
//! Every manual recategorization records an example in the note's own project
//! folder as a single YAML file (`_category.yml`), so the corrections sync and
//! export with the rest of the knowledge base instead of living in the derived
//! SQLite index — exactly like the per-project [`crate::glossary`] and
//! [`crate::routing_examples`] files it is modeled on.
//!
//! The file carries two things, both feeding the same consumer:
//!
//! - `default`: an optional per-project category hint. A project whose meetings
//!   are nearly always one genre says so once, and every distill of a note
//!   routed there starts from that prior. Hand-edited for now; nothing in the
//!   UI writes it.
//! - `examples`: the corrections themselves. Recurring meetings are the point —
//!   the same project plus a similar title next week should land on the genre
//!   the user picked this week.
//!
//! Both are read at distill time by [`crate::distill`], which renders them into
//! the classification prompt as guidance (never as a rule: the transcript can
//! always overrule a prior). That is deliberately weaker than the routing
//! scorer's use of `_routing_examples.yml`, which is arithmetic — a category is
//! chosen by the model reading a conversation, so its memory is prose the model
//! reads too.
//!
//! One entry per note id: recategorizing the same note *overwrites* its entry,
//! and a project's log is capped at [`MAX_EXAMPLES`], oldest evicted first, so
//! it stays bounded over a vault's lifetime.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::note::MeetingCategory;

/// Per-process counter that, combined with the process id, gives each in-flight
/// `save` a unique temp filename so concurrent saves can't clobber each other's
/// scratch file (mirrors [`crate::routing_examples`]).
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Filename of a project's category file, relative to the project folder root.
/// The `_` prefix marks it as infrastructure: note scans read only `.md`, and
/// `validate_folder_segment` rejects `_`-prefixed project segments, so it never
/// appears as a note or a project.
pub const CATEGORY_FILE: &str = "_category.yml";

/// Maximum number of corrections kept in one project's file. Only the most
/// recent handful are ever rendered into a prompt (`distill`'s block is bounded
/// separately), so everything past this cap is file bulk; the oldest go first,
/// since a project's recent meetings are what its next one is likely to
/// resemble.
pub const MAX_EXAMPLES: usize = 200;

/// The on-disk path of a project's category file.
pub fn category_path(project_dir: &Path) -> PathBuf {
    project_dir.join(CATEGORY_FILE)
}

/// A single classification correction: which note was recategorized, what the
/// user chose, and enough lexical context (title + excerpt) for the classifier
/// to recognize the same meeting again.
///
/// Deliberately carries no confidence (a human correction is not an estimate —
/// the routing loop's `confidence: 1.0` says the same thing more verbosely) and
/// no previous category (nothing reads it; the corrected value is the whole
/// signal).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CategoryExample {
    pub note_id: String,
    pub title: String,
    pub excerpt: String,
    pub category: MeetingCategory,
    /// RFC 3339 UTC, seconds precision — when the correction was made.
    pub corrected_at: String,
}

/// A project's category prior and its recorded corrections.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CategoryFile {
    /// The project's default genre, used as a prior when the transcript is
    /// ambiguous. `None` (the key absent) means no prior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<MeetingCategory>,
    #[serde(default)]
    examples: Vec<CategoryExample>,
}

#[derive(Debug, thiserror::Error)]
pub enum CategoryFileError {
    #[error("category-file I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("category-file YAML error at {path}: {source}")]
    Yaml {
        path: PathBuf,
        #[source]
        source: serde_yaml_ng::Error,
    },
}

impl CategoryFile {
    /// Loads a project's category file. A project without one yet is valid and
    /// yields an empty file rather than an error.
    pub fn load(project_dir: &Path) -> Result<CategoryFile, CategoryFileError> {
        let path = category_path(project_dir);
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(CategoryFile::default());
            }
            Err(source) => return Err(CategoryFileError::Io { path, source }),
        };
        serde_yaml_ng::from_str(&raw).map_err(|source| CategoryFileError::Yaml { path, source })
    }

    /// Persists the file to `project_dir`, writing to a per-process/per-call
    /// unique temp file first and renaming it over the target so a reader never
    /// observes a partially written file and two concurrent saves can't race
    /// into a corrupt result.
    pub fn save(&self, project_dir: &Path) -> Result<(), CategoryFileError> {
        let path = category_path(project_dir);
        let tmp_path = project_dir.join(format!(
            "{CATEGORY_FILE}.{}.{}.tmp",
            std::process::id(),
            TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));

        let yaml = serde_yaml_ng::to_string(self).map_err(|source| CategoryFileError::Yaml {
            path: path.clone(),
            source,
        })?;
        fs::write(&tmp_path, yaml).map_err(|source| CategoryFileError::Io {
            path: tmp_path.clone(),
            source,
        })?;
        if let Err(source) = fs::rename(&tmp_path, &path) {
            // Don't leave a stray temp file behind in a folder that syncs and
            // exports with the rest of the knowledge base.
            let _ = fs::remove_file(&tmp_path);
            return Err(CategoryFileError::Io { path, source });
        }
        Ok(())
    }

    /// Records a correction, replacing any prior entry for the same `note_id`
    /// or appending a new one, then evicts the oldest entries beyond
    /// [`MAX_EXAMPLES`].
    ///
    /// Eviction only ever runs on the append path — an overwrite can't grow the
    /// log — and never drops the entry just recorded, so the correction a user
    /// just made always takes effect.
    pub fn upsert(&mut self, example: CategoryExample) {
        if let Some(existing) = self
            .examples
            .iter_mut()
            .find(|e| e.note_id == example.note_id)
        {
            *existing = example;
            return;
        }
        self.examples.push(example);
        let just_recorded = self.examples.len() - 1;
        self.evict_oldest_beyond_cap(just_recorded);
    }

    /// Drops oldest-first until at most [`MAX_EXAMPLES`] remain, never touching
    /// `protected`. Ordered by `corrected_at` (RFC 3339 UTC, so a lexical sort
    /// *is* a chronological one — see `.claude/rules/utc-timestamps.md`) with
    /// `note_id` breaking ties, so eviction is deterministic even for
    /// corrections recorded in the same second.
    fn evict_oldest_beyond_cap(&mut self, protected: usize) {
        if self.examples.len() <= MAX_EXAMPLES {
            return;
        }
        let mut order: Vec<usize> = (0..self.examples.len())
            .filter(|&i| i != protected)
            .collect();
        order.sort_by(|&a, &b| {
            let (left, right) = (&self.examples[a], &self.examples[b]);
            left.corrected_at
                .cmp(&right.corrected_at)
                .then_with(|| left.note_id.cmp(&right.note_id))
        });
        let doomed: std::collections::HashSet<usize> = order
            .into_iter()
            .take(self.examples.len() - MAX_EXAMPLES)
            .collect();
        let mut index = 0;
        self.examples.retain(|_| {
            let keep = !doomed.contains(&index);
            index += 1;
            keep
        });
    }

    /// Removes the entry for `note_id` if present. Returns whether anything was
    /// removed.
    pub fn remove(&mut self, note_id: &str) -> bool {
        let before = self.examples.len();
        self.examples.retain(|e| e.note_id != note_id);
        self.examples.len() != before
    }

    /// Whether the file carries nothing worth writing — no prior and no
    /// corrections.
    pub fn is_empty(&self) -> bool {
        self.default.is_none() && self.examples.is_empty()
    }

    pub fn examples(&self) -> &[CategoryExample] {
        &self.examples
    }

    /// The `limit` most recently corrected examples, newest first — the shape
    /// the distill prompt block wants. Sorted by `corrected_at` (UTC RFC 3339,
    /// so lexical order is chronological) with `note_id` breaking ties.
    pub fn most_recent(&self, limit: usize) -> Vec<&CategoryExample> {
        let mut ordered: Vec<&CategoryExample> = self.examples.iter().collect();
        ordered.sort_by(|left, right| {
            right
                .corrected_at
                .cmp(&left.corrected_at)
                .then_with(|| left.note_id.cmp(&right.note_id))
        });
        ordered.truncate(limit);
        ordered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example(note_id: &str, corrected_at: &str, category: MeetingCategory) -> CategoryExample {
        CategoryExample {
            note_id: note_id.to_string(),
            title: format!("Meeting {note_id}"),
            excerpt: "Some prose from the body.".to_string(),
            category,
            corrected_at: corrected_at.to_string(),
        }
    }

    #[test]
    fn a_project_without_a_file_loads_as_empty() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = CategoryFile::load(dir.path()).expect("load");
        assert_eq!(file, CategoryFile::default());
        assert!(file.is_empty());
    }

    #[test]
    fn saves_and_reloads_the_prior_and_the_examples() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut file = CategoryFile {
            default: Some(MeetingCategory::Client),
            examples: Vec::new(),
        };
        file.upsert(example(
            "n_a1b2c3",
            "2026-08-19T12:00:00Z",
            MeetingCategory::Review,
        ));
        file.save(dir.path()).expect("save");

        let reloaded = CategoryFile::load(dir.path()).expect("load");
        assert_eq!(reloaded, file);
        assert_eq!(reloaded.default, Some(MeetingCategory::Client));
        assert_eq!(reloaded.examples()[0].category, MeetingCategory::Review);
        // No stray temp file left in a folder the user syncs.
        let strays: Vec<_> = fs::read_dir(dir.path())
            .expect("read dir")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(strays.is_empty(), "left temp files behind: {strays:?}");
    }

    #[test]
    fn a_file_with_no_prior_omits_the_key() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut file = CategoryFile::default();
        file.upsert(example(
            "n_a1b2c3",
            "2026-08-19T12:00:00Z",
            MeetingCategory::Standup,
        ));
        file.save(dir.path()).expect("save");

        let raw = fs::read_to_string(category_path(dir.path())).expect("read");
        assert!(!raw.contains("default"), "unexpected default key in {raw}");
        assert!(
            raw.contains("category: standup"),
            "missing category in {raw}"
        );
    }

    #[test]
    fn recategorizing_the_same_note_overwrites_its_entry() {
        let mut file = CategoryFile::default();
        file.upsert(example(
            "n_a1b2c3",
            "2026-08-19T12:00:00Z",
            MeetingCategory::Standup,
        ));
        file.upsert(example(
            "n_a1b2c3",
            "2026-08-19T13:00:00Z",
            MeetingCategory::Client,
        ));

        assert_eq!(file.examples().len(), 1);
        assert_eq!(file.examples()[0].category, MeetingCategory::Client);
    }

    #[test]
    fn eviction_drops_the_oldest_and_never_the_just_recorded() {
        let mut file = CategoryFile::default();
        for index in 0..MAX_EXAMPLES {
            file.upsert(example(
                &format!("n_{index:06}"),
                &format!("2026-01-01T00:{:02}:00Z", index % 60),
                MeetingCategory::Standup,
            ));
        }
        assert_eq!(file.examples().len(), MAX_EXAMPLES);

        // The newest possible timestamp on the oldest-sorting id would still be
        // kept: it is the entry just recorded.
        file.upsert(example(
            "n_zzzzzz",
            "1999-01-01T00:00:00Z",
            MeetingCategory::Observer,
        ));
        assert_eq!(file.examples().len(), MAX_EXAMPLES);
        assert!(
            file.examples().iter().any(|e| e.note_id == "n_zzzzzz"),
            "the just-recorded correction was evicted"
        );
    }

    #[test]
    fn most_recent_returns_newest_first_and_respects_the_limit() {
        let mut file = CategoryFile::default();
        file.upsert(example(
            "n_aaaaaa",
            "2026-08-01T00:00:00Z",
            MeetingCategory::Standup,
        ));
        file.upsert(example(
            "n_bbbbbb",
            "2026-08-03T00:00:00Z",
            MeetingCategory::Client,
        ));
        file.upsert(example(
            "n_cccccc",
            "2026-08-02T00:00:00Z",
            MeetingCategory::Review,
        ));

        let recent = file.most_recent(2);
        let ids: Vec<&str> = recent.iter().map(|e| e.note_id.as_str()).collect();
        assert_eq!(ids, vec!["n_bbbbbb", "n_cccccc"]);
    }

    #[test]
    fn remove_reports_whether_it_removed_anything() {
        let mut file = CategoryFile::default();
        file.upsert(example(
            "n_a1b2c3",
            "2026-08-19T12:00:00Z",
            MeetingCategory::Standup,
        ));
        assert!(file.remove("n_a1b2c3"));
        assert!(!file.remove("n_a1b2c3"));
        assert!(file.is_empty());
    }

    #[test]
    fn an_unknown_category_in_a_hand_edited_file_is_an_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(category_path(dir.path()), "default: retro\nexamples: []\n").expect("write");
        let error = CategoryFile::load(dir.path()).expect_err("unknown category must not load");
        assert!(
            matches!(error, CategoryFileError::Yaml { .. }),
            "expected a YAML error, got {error:?}"
        );
    }
}
