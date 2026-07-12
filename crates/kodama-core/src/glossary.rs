//! Per-project glossary storage.
//!
//! A project's glossary lives as a single YAML file at the root of its
//! project folder (`_glossary.yml`), so it syncs and exports with the rest
//! of the knowledge base instead of living in the derived SQLite index. The
//! project itself is never stored inside the file — it is implicit in which
//! project folder the file sits in, which is what keeps one project's terms
//! from bleeding into another's.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// Per-process counter that, combined with the process id, gives each in-flight
/// `save` a unique temp filename so concurrent saves can't clobber each other's
/// scratch file.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Filename of a project's glossary file, relative to the project folder root.
pub const GLOSSARY_FILE: &str = "_glossary.yml";

/// The on-disk path of a project's glossary file.
pub fn glossary_path(project_dir: &Path) -> PathBuf {
    project_dir.join(GLOSSARY_FILE)
}

/// A single glossary entry: a term, its definition, and alternate spellings
/// that should resolve to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GlossaryTerm {
    pub term: String,
    pub definition: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// A project's full set of glossary terms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Glossary {
    #[serde(default)]
    terms: Vec<GlossaryTerm>,
}

/// What to do when [`Glossary::upsert`] finds a term that already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnConflict {
    Update,
    Error,
}

#[derive(Debug, thiserror::Error)]
pub enum GlossaryError {
    #[error("glossary I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("glossary YAML error at {path}: {source}")]
    Yaml {
        path: PathBuf,
        #[source]
        source: serde_yaml_ng::Error,
    },
    #[error("glossary term {term:?} already exists")]
    Conflict { term: String },
    #[error("glossary at {path} has duplicate term {term:?}")]
    Duplicate { path: PathBuf, term: String },
}

/// Normalizes a term for case-insensitive matching (used consistently by
/// both `upsert` and `get`, so they always agree on what counts as "the
/// same term").
fn normalize(term: &str) -> String {
    term.trim().to_lowercase()
}

/// Whether `candidate` matches an already-`normalize`d `key`, applying the same
/// trim + case-fold semantics without allocating a `String` per comparison (the
/// hot path when scanning a whole glossary on every `get`/`upsert`).
fn normalized_eq(candidate: &str, key: &str) -> bool {
    candidate
        .trim()
        .chars()
        .flat_map(char::to_lowercase)
        .eq(key.chars())
}

impl GlossaryTerm {
    /// Trims surrounding whitespace from the term and its aliases and drops
    /// blank aliases, so the persisted data matches the normalized text used
    /// for lookups instead of carrying stray whitespace into exports.
    fn normalized(mut self) -> GlossaryTerm {
        self.term = self.term.trim().to_string();
        self.aliases = self
            .aliases
            .into_iter()
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty())
            .collect();
        self
    }

    /// True when `key` (already `normalize`d) matches this entry's term or any
    /// of its aliases — aliases are alternate spellings that resolve to it.
    fn matches(&self, key: &str) -> bool {
        normalized_eq(&self.term, key) || self.aliases.iter().any(|a| normalized_eq(a, key))
    }
}

impl Glossary {
    /// Loads a project's glossary. A project without a glossary file yet is
    /// valid and simply yields an empty glossary rather than an error.
    pub fn load(project_dir: &Path) -> Result<Glossary, GlossaryError> {
        let path = glossary_path(project_dir);
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Glossary::default());
            }
            Err(source) => return Err(GlossaryError::Io { path, source }),
        };
        let glossary: Glossary =
            serde_yaml_ng::from_str(&raw).map_err(|source| GlossaryError::Yaml {
                path: path.clone(),
                source,
            })?;
        glossary.check_no_duplicate_terms(&path)?;
        Ok(glossary)
    }

    /// Rejects a glossary whose entries collide on the normalized term text.
    /// Without this a hand-authored file with two `MERIDIAN` entries would parse
    /// fine, but every lookup and in-place update would silently only ever
    /// touch the first, leaving the second as unreachable shadow data.
    fn check_no_duplicate_terms(&self, path: &Path) -> Result<(), GlossaryError> {
        let mut seen: Vec<String> = Vec::with_capacity(self.terms.len());
        for entry in &self.terms {
            let key = normalize(&entry.term);
            if seen.contains(&key) {
                return Err(GlossaryError::Duplicate {
                    path: path.to_path_buf(),
                    term: entry.term.clone(),
                });
            }
            seen.push(key);
        }
        Ok(())
    }

    /// Persists the glossary to `project_dir`, writing to a temp file first
    /// and renaming it over the target so a reader never observes a
    /// partially written file.
    pub fn save(&self, project_dir: &Path) -> Result<(), GlossaryError> {
        let path = glossary_path(project_dir);
        // A per-process/per-call unique temp name keeps two concurrent saves to
        // the same project from writing over the same scratch file and racing
        // their renames into a corrupt result.
        let tmp_path = project_dir.join(format!(
            "{GLOSSARY_FILE}.{}.{}.tmp",
            std::process::id(),
            TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));

        let yaml = serde_yaml_ng::to_string(self).map_err(|source| GlossaryError::Yaml {
            path: path.clone(),
            source,
        })?;
        fs::write(&tmp_path, yaml).map_err(|source| GlossaryError::Io {
            path: tmp_path.clone(),
            source,
        })?;
        if let Err(source) = fs::rename(&tmp_path, &path) {
            // Don't leave a stray temp file behind in a folder that syncs and
            // exports with the rest of the knowledge base.
            let _ = fs::remove_file(&tmp_path);
            return Err(GlossaryError::Io { path, source });
        }
        Ok(())
    }

    /// Inserts a new term, or updates an existing one matched by
    /// case-insensitive, trimmed term text. Returns `true` if the term was
    /// newly created, `false` if an existing entry was updated in place.
    pub fn upsert(
        &mut self,
        term: GlossaryTerm,
        on_conflict: OnConflict,
    ) -> Result<bool, GlossaryError> {
        let term = term.normalized();
        let key = normalize(&term.term);
        // Identity is the primary term text only; aliases resolve on lookup but
        // don't make one term "the same" as another for upsert purposes.
        if let Some(existing) = self.terms.iter_mut().find(|t| normalized_eq(&t.term, &key)) {
            if on_conflict == OnConflict::Error {
                return Err(GlossaryError::Conflict { term: term.term });
            }
            *existing = term;
            return Ok(false);
        }
        self.terms.push(term);
        Ok(true)
    }

    /// Looks up a term by case-insensitive, trimmed match against its term text
    /// or any of its aliases.
    pub fn get(&self, term: &str) -> Option<&GlossaryTerm> {
        let key = normalize(term);
        self.terms.iter().find(|t| t.matches(&key))
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    pub fn terms(&self) -> &[GlossaryTerm] {
        &self.terms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn term(term: &str, definition: &str, aliases: &[&str]) -> GlossaryTerm {
        GlossaryTerm {
            term: term.to_string(),
            definition: definition.to_string(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempdir().unwrap();
        let mut glossary = Glossary::default();
        glossary
            .upsert(
                term(
                    "MERIDIAN",
                    "A regional systems-migration project.",
                    &["meridian", "mer-idian"],
                ),
                OnConflict::Error,
            )
            .unwrap();
        glossary
            .upsert(
                term("TeeTrack", "Tee-sheet / POS vendor.", &["t-track"]),
                OnConflict::Error,
            )
            .unwrap();

        glossary.save(dir.path()).unwrap();
        let loaded = Glossary::load(dir.path()).unwrap();

        assert_eq!(loaded, glossary);
    }

    #[test]
    fn load_with_no_file_is_empty_not_error() {
        let dir = tempdir().unwrap();
        let loaded = Glossary::load(dir.path()).unwrap();
        assert!(loaded.is_empty());
    }

    #[test]
    fn parses_hand_authored_yaml() {
        let dir = tempdir().unwrap();
        let yaml = r#"
terms:
  - term: MERIDIAN
    definition: A regional systems-migration project.
    aliases: [meridian, mer-idian]
  - term: TeeTrack
    definition: Tee-sheet / POS vendor.
"#;
        fs::write(glossary_path(dir.path()), yaml).unwrap();

        let loaded = Glossary::load(dir.path()).unwrap();

        assert_eq!(loaded.terms().len(), 2);
        assert_eq!(
            loaded.get("meridian").unwrap().definition,
            "A regional systems-migration project."
        );
        // No `aliases` key in the hand-written entry should default to empty.
        assert!(loaded.get("teetrack").unwrap().aliases.is_empty());
    }

    #[test]
    fn upsert_creates_new_term() {
        let mut glossary = Glossary::default();
        let created = glossary
            .upsert(
                term("MERIDIAN", "A regional systems-migration project.", &[]),
                OnConflict::Error,
            )
            .unwrap();
        assert!(created);
        assert_eq!(glossary.terms().len(), 1);
    }

    #[test]
    fn upsert_updates_existing_term_case_insensitively() {
        let mut glossary = Glossary::default();
        glossary
            .upsert(term("MERIDIAN", "First definition.", &[]), OnConflict::Error)
            .unwrap();

        let created = glossary
            .upsert(
                term("meridian", "Updated definition.", &["meridian-alias"]),
                OnConflict::Update,
            )
            .unwrap();

        assert!(!created);
        assert_eq!(glossary.terms().len(), 1);
        assert_eq!(
            glossary.get("MERIDIAN").unwrap().definition,
            "Updated definition."
        );
    }

    #[test]
    fn upsert_on_conflict_error_rejects_existing_term() {
        let mut glossary = Glossary::default();
        glossary
            .upsert(term("MERIDIAN", "First definition.", &[]), OnConflict::Error)
            .unwrap();

        let result = glossary.upsert(term("meridian", "Second definition.", &[]), OnConflict::Error);

        assert!(matches!(result, Err(GlossaryError::Conflict { term }) if term == "meridian"));
        assert_eq!(glossary.terms().len(), 1);
    }

    #[test]
    fn get_is_case_insensitive_and_trims() {
        let mut glossary = Glossary::default();
        glossary
            .upsert(
                term("TeeTrack", "Tee-sheet / POS vendor.", &[]),
                OnConflict::Error,
            )
            .unwrap();

        assert!(glossary.get("  teetrack ").is_some());
        assert!(glossary.get("TEETRACK").is_some());
        assert!(glossary.get("nonexistent").is_none());
    }

    #[test]
    fn separate_project_dirs_stay_isolated() {
        let dir_a = tempdir().unwrap();
        let dir_b = tempdir().unwrap();

        let mut glossary_a = Glossary::default();
        glossary_a
            .upsert(term("MERIDIAN", "Client A's project.", &[]), OnConflict::Error)
            .unwrap();
        glossary_a.save(dir_a.path()).unwrap();

        let mut glossary_b = Glossary::default();
        glossary_b
            .upsert(term("TeeTrack", "Client B's vendor.", &[]), OnConflict::Error)
            .unwrap();
        glossary_b.save(dir_b.path()).unwrap();

        let loaded_a = Glossary::load(dir_a.path()).unwrap();
        let loaded_b = Glossary::load(dir_b.path()).unwrap();

        assert!(loaded_a.get("meridian").is_some());
        assert!(loaded_a.get("teetrack").is_none());
        assert!(loaded_b.get("teetrack").is_some());
        assert!(loaded_b.get("meridian").is_none());
    }

    #[test]
    fn malformed_yaml_is_a_parse_error() {
        let dir = tempdir().unwrap();
        fs::write(glossary_path(dir.path()), "terms: [\n").unwrap();

        let result = Glossary::load(dir.path());

        assert!(matches!(result, Err(GlossaryError::Yaml { .. })));
    }

    #[test]
    fn get_resolves_aliases() {
        let mut glossary = Glossary::default();
        glossary
            .upsert(
                term("TeeTrack", "Tee-sheet / POS vendor.", &["t-track", "tee-track"]),
                OnConflict::Error,
            )
            .unwrap();

        // An alias — including with different case and surrounding whitespace —
        // resolves to its entry.
        assert_eq!(glossary.get("t-track").unwrap().term, "TeeTrack");
        assert_eq!(glossary.get("  Tee-Track ").unwrap().term, "TeeTrack");
        assert!(glossary.get("nope").is_none());
    }

    #[test]
    fn upsert_trims_term_and_aliases_and_drops_blanks() {
        let mut glossary = Glossary::default();
        glossary
            .upsert(
                term(
                    "  MERIDIAN  ",
                    "A regional systems-migration project.",
                    &["  meridian ", "", "  "],
                ),
                OnConflict::Error,
            )
            .unwrap();

        let stored = &glossary.terms()[0];
        assert_eq!(stored.term, "MERIDIAN");
        assert_eq!(stored.aliases, vec!["meridian".to_string()]);
    }

    #[test]
    fn load_rejects_duplicate_terms() {
        let dir = tempdir().unwrap();
        let yaml = r#"
terms:
  - term: MERIDIAN
    definition: First.
  - term: meridian
    definition: Second.
"#;
        fs::write(glossary_path(dir.path()), yaml).unwrap();

        let result = Glossary::load(dir.path());

        assert!(matches!(result, Err(GlossaryError::Duplicate { term, .. }) if term == "meridian"));
    }

    #[test]
    fn save_leaves_no_temp_file_behind() {
        let dir = tempdir().unwrap();
        let mut glossary = Glossary::default();
        glossary
            .upsert(
                term("MERIDIAN", "A regional systems-migration project.", &[]),
                OnConflict::Error,
            )
            .unwrap();
        glossary.save(dir.path()).unwrap();

        let stray = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().ends_with(".tmp"));
        assert!(!stray, "save should not leave a .tmp scratch file behind");
    }
}
