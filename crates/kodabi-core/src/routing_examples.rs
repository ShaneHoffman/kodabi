//! Per-project routing-example storage — the correction loop's memory.
//!
//! Every manual re-route (the Inbox one-click correction, or the MCP
//! `file_note_to_project` tool) records a routing example in the *target*
//! project's folder as a single YAML file (`_routing_examples.yml`), so the
//! corrections sync and export with the rest of the knowledge base instead of
//! living in the derived SQLite index — exactly like the per-project
//! [`crate::glossary`] file it is modeled on. The target project is implicit in
//! which folder the file sits in and is never stored inside it.
//!
//! One entry per note id: a re-correction of the same note *overwrites* its
//! entry (the contract's "stored as the note's last-correction note, overwrites
//! any prior"), and a later re-route moves the entry into the new target's file
//! (the mover removes it from the previous one), so a note has at most one
//! correction record vault-wide. A project's log is capped at [`MAX_EXAMPLES`],
//! oldest evicted first, so it stays bounded over a vault's lifetime.
//!
//! The scorer reads these: [`crate::routing::load_project_signals`] loads each
//! candidate's examples alongside its glossary, and scoring credits a project
//! for lexical similarity between the incoming note and its recorded
//! corrections (`routing::EXAMPLE_WEIGHT`, capped below the auto-file threshold
//! on its own so one correction never files a note single-handedly). A
//! correction therefore measurably changes future routing — the loop is closed.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// Per-process counter that, combined with the process id, gives each in-flight
/// `save` a unique temp filename so concurrent saves can't clobber each other's
/// scratch file (mirrors [`crate::glossary`]).
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Filename of a project's routing-examples file, relative to the project
/// folder root. The `_` prefix marks it as infrastructure: note scans read only
/// `.md`, and `validate_folder_segment` rejects `_`-prefixed project segments,
/// so it never appears as a note or a project.
pub const ROUTING_EXAMPLES_FILE: &str = "_routing_examples.yml";

/// Maximum length (in `char`s) of a stored body excerpt. Enough to give the
/// scorer lexical context without copying whole notes into the per-project
/// config file.
pub const EXCERPT_MAX_CHARS: usize = 300;

/// Maximum number of examples kept in one project's log. The scorer aggregates
/// with `max`, not a sum (`routing::EXAMPLE_WEIGHT`), so only the single
/// best-matching example ever affects a routing decision — every entry past it
/// is file bulk and per-capture tokenizing work on the global-hotkey path. The
/// cap keeps both bounded no matter how long the vault is in use; the oldest
/// corrections are the ones dropped, since a project's recent vocabulary is what
/// its next note is likely to resemble.
pub const MAX_EXAMPLES: usize = 200;

/// The on-disk path of a project's routing-examples file.
pub fn routing_examples_path(project_dir: &Path) -> PathBuf {
    project_dir.join(ROUTING_EXAMPLES_FILE)
}

/// Derives a compact, single-line **prose** excerpt of a note body for a routing
/// example: Markdown structure is dropped, whitespace runs collapse to a single
/// space, the result is trimmed, and it is truncated to [`EXCERPT_MAX_CHARS`] on
/// a `char` boundary (so a multibyte character is never split).
///
/// Dropping structure matters because the excerpt is scoring input, not just a
/// human-readable snippet. A distilled body opens with `distill::render_body`'s
/// `# Summary` / `## Decisions` / `## Action items` scaffolding, which *every*
/// distilled note carries — keeping it would hand each recorded correction a
/// handful of tokens that match every future note regardless of topic. Heading
/// lines go entirely (their text is a section label, not content); list and
/// task markers are stripped from the front of their line, keeping the item's
/// prose.
pub fn excerpt(body: &str) -> String {
    let prose = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .map(strip_list_marker)
        .collect::<Vec<_>>()
        .join(" ");
    prose
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(EXCERPT_MAX_CHARS)
        .collect()
}

/// Strips a leading Markdown list or task marker (`- `, `* `, `- [ ] `,
/// `- [x] `) from an already-trimmed line, leaving the item's own text. A line
/// that isn't a list item is returned unchanged.
fn strip_list_marker(line: &str) -> &str {
    let Some(rest) = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
    else {
        return line;
    };
    let rest = rest.trim_start();
    // `render_body` writes action items as `- [ ] …`; the checkbox is structure.
    for checkbox in ["[ ] ", "[x] ", "[X] "] {
        if let Some(after) = rest.strip_prefix(checkbox) {
            return after.trim_start();
        }
    }
    rest
}

/// A single routing correction: which note was re-filed, where it came from,
/// the score the correction recorded, and enough lexical context (title +
/// excerpt) for a future signal blender to learn from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingExample {
    pub note_id: String,
    pub title: String,
    pub excerpt: String,
    /// The project the note was corrected *from*; `null` (`None`) when it was
    /// in the Inbox (the Inbox never holds an examples file of its own).
    #[serde(default)]
    pub previous_project: Option<String>,
    /// The routing score this correction wrote to the note's frontmatter
    /// (`1.0` for a plain human correction).
    pub confidence: f64,
    /// RFC 3339 UTC, seconds precision — when the correction was made.
    pub corrected_at: String,
    /// Optional human-readable reason for the correction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A project's full set of routing examples.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RoutingExamples {
    #[serde(default)]
    examples: Vec<RoutingExample>,
}

#[derive(Debug, thiserror::Error)]
pub enum RoutingExamplesError {
    #[error("routing-examples I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("routing-examples YAML error at {path}: {source}")]
    Yaml {
        path: PathBuf,
        #[source]
        source: serde_yaml_ng::Error,
    },
}

impl RoutingExamples {
    /// Loads a project's routing examples. A project without a file yet is
    /// valid and yields an empty set rather than an error.
    pub fn load(project_dir: &Path) -> Result<RoutingExamples, RoutingExamplesError> {
        let path = routing_examples_path(project_dir);
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(RoutingExamples::default());
            }
            Err(source) => return Err(RoutingExamplesError::Io { path, source }),
        };
        serde_yaml_ng::from_str(&raw).map_err(|source| RoutingExamplesError::Yaml { path, source })
    }

    /// Persists the examples to `project_dir`, writing to a per-process/per-call
    /// unique temp file first and renaming it over the target so a reader never
    /// observes a partially written file and two concurrent saves can't race
    /// into a corrupt result.
    pub fn save(&self, project_dir: &Path) -> Result<(), RoutingExamplesError> {
        let path = routing_examples_path(project_dir);
        let tmp_path = project_dir.join(format!(
            "{ROUTING_EXAMPLES_FILE}.{}.{}.tmp",
            std::process::id(),
            TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));

        let yaml = serde_yaml_ng::to_string(self).map_err(|source| RoutingExamplesError::Yaml {
            path: path.clone(),
            source,
        })?;
        fs::write(&tmp_path, yaml).map_err(|source| RoutingExamplesError::Io {
            path: tmp_path.clone(),
            source,
        })?;
        if let Err(source) = fs::rename(&tmp_path, &path) {
            // Don't leave a stray temp file behind in a folder that syncs and
            // exports with the rest of the knowledge base.
            let _ = fs::remove_file(&tmp_path);
            return Err(RoutingExamplesError::Io { path, source });
        }
        Ok(())
    }

    /// Records a correction, replacing any prior entry for the same `note_id`
    /// (the contract's overwrite semantics) or appending a new one, then evicts
    /// the oldest entries beyond [`MAX_EXAMPLES`].
    ///
    /// Eviction only ever runs on the append path — an overwrite can't grow the
    /// log — and never drops the entry just recorded, so the correction a user
    /// just made always takes effect.
    pub fn upsert(&mut self, example: RoutingExample) {
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
    /// corrections recorded in the same second. A hand-authored file with a
    /// malformed timestamp sorts as very old and is evicted first, which is the
    /// right bias for an unparseable entry.
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
    /// removed — the mover uses this to avoid rewriting a previous project's
    /// file that had no entry to drop.
    pub fn remove(&mut self, note_id: &str) -> bool {
        let before = self.examples.len();
        self.examples.retain(|e| e.note_id != note_id);
        self.examples.len() != before
    }

    pub fn is_empty(&self) -> bool {
        self.examples.is_empty()
    }

    pub fn examples(&self) -> &[RoutingExample] {
        &self.examples
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn example(note_id: &str, previous: Option<&str>, confidence: f64) -> RoutingExample {
        RoutingExample {
            note_id: note_id.to_string(),
            title: "weekly sync".to_string(),
            excerpt: "First line of the body.".to_string(),
            previous_project: previous.map(str::to_string),
            confidence,
            corrected_at: "2026-07-18T20:15:00Z".to_string(),
            reason: None,
        }
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempdir().unwrap();
        let mut log = RoutingExamples::default();
        log.upsert(example("n_a1b2c3", None, 1.0));
        log.upsert(example("n_d4e5f6", Some("Ops"), 0.9));

        log.save(dir.path()).unwrap();
        let loaded = RoutingExamples::load(dir.path()).unwrap();

        assert_eq!(loaded, log);
    }

    #[test]
    fn load_with_no_file_is_empty_not_error() {
        let dir = tempdir().unwrap();
        let loaded = RoutingExamples::load(dir.path()).unwrap();
        assert!(loaded.is_empty());
    }

    #[test]
    fn upsert_replaces_the_entry_for_the_same_note_id() {
        let mut log = RoutingExamples::default();
        log.upsert(example("n_a1b2c3", None, 0.5));
        log.upsert(example("n_a1b2c3", Some("Growth"), 1.0));

        assert_eq!(log.examples().len(), 1);
        let stored = &log.examples()[0];
        assert_eq!(stored.confidence, 1.0);
        assert_eq!(stored.previous_project.as_deref(), Some("Growth"));
    }

    #[test]
    fn remove_reports_whether_an_entry_was_dropped() {
        let mut log = RoutingExamples::default();
        log.upsert(example("n_a1b2c3", None, 1.0));

        assert!(log.remove("n_a1b2c3"));
        assert!(log.is_empty());
        assert!(!log.remove("n_a1b2c3"));
    }

    #[test]
    fn parses_hand_authored_yaml_with_absent_optional_keys() {
        let dir = tempdir().unwrap();
        let yaml = r#"
examples:
  - note_id: n_a1b2c3
    title: weekly sync
    excerpt: First line.
    previous_project: null
    confidence: 1.0
    corrected_at: 2026-07-18T20:15:00Z
  - note_id: n_d4e5f6
    title: budget review
    excerpt: Numbers.
    confidence: 0.8
    corrected_at: 2026-07-18T21:00:00Z
    reason: Clearly a finance note.
"#;
        fs::write(routing_examples_path(dir.path()), yaml).unwrap();

        let loaded = RoutingExamples::load(dir.path()).unwrap();

        assert_eq!(loaded.examples().len(), 2);
        // Absent `previous_project` and `reason` default to None.
        assert_eq!(loaded.examples()[0].previous_project, None);
        assert_eq!(loaded.examples()[0].reason, None);
        assert_eq!(
            loaded.examples()[1].reason.as_deref(),
            Some("Clearly a finance note.")
        );
    }

    #[test]
    fn reason_is_omitted_from_yaml_when_absent() {
        let mut log = RoutingExamples::default();
        log.upsert(example("n_a1b2c3", None, 1.0));
        let yaml = serde_yaml_ng::to_string(&log).unwrap();
        assert!(
            !yaml.contains("reason"),
            "an absent reason should not emit a key: {yaml}"
        );
    }

    #[test]
    fn malformed_yaml_is_a_parse_error() {
        let dir = tempdir().unwrap();
        fs::write(routing_examples_path(dir.path()), "examples: [\n").unwrap();

        let result = RoutingExamples::load(dir.path());

        assert!(matches!(result, Err(RoutingExamplesError::Yaml { .. })));
    }

    #[test]
    fn save_leaves_no_temp_file_behind() {
        let dir = tempdir().unwrap();
        let mut log = RoutingExamples::default();
        log.upsert(example("n_a1b2c3", None, 1.0));
        log.save(dir.path()).unwrap();

        let stray = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().ends_with(".tmp"));
        assert!(!stray, "save should not leave a .tmp scratch file behind");
    }

    #[test]
    fn excerpt_collapses_whitespace_and_trims() {
        let body = "  First line.\n\nSecond   line.\t Third.  ";
        assert_eq!(excerpt(body), "First line. Second line. Third.");
    }

    #[test]
    fn excerpt_drops_markdown_structure() {
        // A distilled body's scaffolding: every distilled note carries these
        // exact headings, so keeping them would give each recorded correction a
        // handful of tokens that match every future note regardless of topic.
        let body = "# Summary\n\nThe clubhouse migration cutover slipped a week.\n\n\
                    ## Decisions\n\n- Hold the vendor to the original date\n\n\
                    ## Action items\n\n- [ ] Draft the irrigation schedule";
        let out = excerpt(body);

        assert!(!out.contains('#'), "heading markers survived: {out}");
        for heading in ["Summary", "Decisions", "Action items"] {
            assert!(!out.contains(heading), "heading text survived: {out}");
        }
        assert!(!out.contains("- "), "list markers survived: {out}");
        assert!(!out.contains("[ ]"), "task checkbox survived: {out}");
        // The prose itself is kept, in order.
        assert_eq!(
            out,
            "The clubhouse migration cutover slipped a week. \
             Hold the vendor to the original date Draft the irrigation schedule"
        );
    }

    #[test]
    fn excerpt_keeps_a_plain_prose_body_intact() {
        // A quick capture has no Markdown structure to strip; stripping must not
        // eat ordinary text that merely starts with a dash-like character.
        assert_eq!(
            excerpt("Call the vendor about the renewal."),
            "Call the vendor about the renewal."
        );
        assert_eq!(excerpt("-5 degrees overnight"), "-5 degrees overnight");
    }

    #[test]
    fn upsert_evicts_the_oldest_beyond_the_cap() {
        let mut log = RoutingExamples::default();
        // Fill past the cap with strictly increasing timestamps.
        for index in 0..MAX_EXAMPLES + 5 {
            let mut entry = example(&format!("n_{index:04}"), None, 1.0);
            entry.corrected_at = format!("2026-07-18T20:{:02}:{:02}Z", index / 60, index % 60);
            log.upsert(entry);
        }

        assert_eq!(log.examples().len(), MAX_EXAMPLES);
        // The five oldest are gone; the newest (recorded last) is kept.
        for index in 0..5 {
            assert!(
                !log.examples()
                    .iter()
                    .any(|e| e.note_id == format!("n_{index:04}")),
                "n_{index:04} should have been evicted"
            );
        }
        assert!(log
            .examples()
            .iter()
            .any(|e| e.note_id == format!("n_{:04}", MAX_EXAMPLES + 4)));
        // Eviction preserves the relative order of the survivors.
        let ids: Vec<&str> = log.examples().iter().map(|e| e.note_id.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted);
    }

    #[test]
    fn upsert_never_evicts_the_correction_just_recorded() {
        // A backdated correction at a full log is still the user's latest
        // action: it must take effect, not be evicted as "oldest" on arrival.
        let mut log = RoutingExamples::default();
        for index in 0..MAX_EXAMPLES {
            let mut entry = example(&format!("n_{index:04}"), None, 1.0);
            entry.corrected_at = format!("2026-07-18T20:{:02}:{:02}Z", index / 60, index % 60);
            log.upsert(entry);
        }
        let mut backdated = example("n_backdated", None, 1.0);
        backdated.corrected_at = "2001-01-01T00:00:00Z".to_string();
        log.upsert(backdated);

        assert_eq!(log.examples().len(), MAX_EXAMPLES);
        assert!(log.examples().iter().any(|e| e.note_id == "n_backdated"));
    }

    #[test]
    fn upsert_of_an_existing_note_never_evicts() {
        // An overwrite can't grow the log, so a full log stays exactly full.
        let mut log = RoutingExamples::default();
        for index in 0..MAX_EXAMPLES {
            log.upsert(example(&format!("n_{index:04}"), None, 1.0));
        }
        log.upsert(example("n_0000", Some("Growth"), 0.5));

        assert_eq!(log.examples().len(), MAX_EXAMPLES);
        let stored = log
            .examples()
            .iter()
            .find(|e| e.note_id == "n_0000")
            .expect("the overwritten entry is still present");
        assert_eq!(stored.previous_project.as_deref(), Some("Growth"));
    }

    #[test]
    fn excerpt_truncates_on_a_char_boundary() {
        // A body of multibyte characters longer than the cap: truncation must
        // land on a char boundary (never panic) and keep exactly the cap count.
        let body = "é".repeat(EXCERPT_MAX_CHARS + 50);
        let out = excerpt(&body);
        assert_eq!(out.chars().count(), EXCERPT_MAX_CHARS);
    }
}
