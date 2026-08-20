//! The single-call project briefing behind the `get_project_context` MCP tool
//! (`docs/MCP_TOOL_SURFACE.md` §6) — description, glossary, recent notes,
//! outstanding items, and counts, for grounding a chat about one project.
//!
//! # Disk and index each answer what they own
//!
//! This is the only place in kodabi-core that joins the vault with the index,
//! and the split is deliberate:
//!
//! - **The disk answers "does this project exist, and what is it"** — the
//!   `project` block comes from the same [`vault::list_projects`] scan
//!   `list_projects` serves, so one project reads identically through either
//!   tool. Its glossary and README live in the project folder too.
//! - **The index answers "what is in it"** — counts, recent notes, and
//!   outstanding items, which need `date_utc` ordering across mixed offsets and
//!   the action-item tables.
//!
//! The two can disagree for a moment: a note written seconds ago is in the disk
//! scan's `project.note_count` before the watcher has indexed it into
//! `counts.notes`. That is the eventual consistency the whole app already lives
//! with (browsing reads the vault and needs no index to be current), not a bug
//! to paper over here.
//!
//! There is also a *definitional* gap the spec builds in: `Project.note_count`
//! is "notes directly in this project (not descendants)" while `counts.notes`
//! honors `include_descendants`. With descendants on, the two differ by design.
//!
//! # No pagination
//!
//! Per the cross-cutting contract's one pagination exception, this tool carries
//! no cursor: every section is hard-capped by its own `*_limit`, and `counts`
//! reports the **true** totals so a caller that hits a cap knows to fall back to
//! `list_outstanding_items` or `search_notes` for that slice.

use std::io;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use crate::glossary::{Glossary, GlossaryError, GlossaryTerm};
use crate::index::{
    ActionItemStatus, NoteIndex, NoteRow, NoteTypeCounts, OutstandingItem, OutstandingParams,
    ProjectScope,
};
use crate::note::{self, INBOX};
use crate::vault::{self, ProjectSummary};

/// Largest README this tool will inline as a project `description`.
///
/// The briefing is deliberately bounded (it has no cursor), so an unbounded
/// README would blow the very budget the `*_limit` params exist to protect. A
/// longer one is truncated on a char boundary with an ellipsis.
const DESCRIPTION_MAX_CHARS: usize = 8_000;

/// The file a project's `description` is read from, when present.
const DESCRIPTION_FILE: &str = "README.md";

fn default_true() -> bool {
    true
}

fn default_recent_notes_limit() -> u32 {
    10
}

fn default_outstanding_limit() -> u32 {
    20
}

fn default_glossary_limit() -> u32 {
    200
}

/// The `get_project_context` inputs — mirrors the tool's `inputSchema`
/// field-for-field, including its defaults.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectContextParams {
    /// The project to summarize.
    pub project: String,
    /// Aggregate over the project's subtree, or just this project.
    ///
    /// Note this defaults to **false**, unlike every other tool's
    /// `include_descendants` — a briefing is about one project unless asked
    /// otherwise.
    #[serde(default)]
    pub include_descendants: bool,
    #[serde(default = "default_true")]
    pub include_glossary: bool,
    #[serde(default = "default_true")]
    pub include_recent_notes: bool,
    #[serde(default = "default_true")]
    pub include_outstanding: bool,
    /// Max recent notes to return, clamped to `0..=50`. `0` omits the section.
    #[serde(default = "default_recent_notes_limit")]
    pub recent_notes_limit: u32,
    /// Max outstanding items to return, clamped to `0..=100`. `0` omits the
    /// section — the counts stay true regardless.
    #[serde(default = "default_outstanding_limit")]
    pub outstanding_limit: u32,
    /// Max glossary terms to return, clamped to `0..=500`. `0` omits the
    /// section; `counts.glossary_terms` still reports the full total.
    #[serde(default = "default_glossary_limit")]
    pub glossary_limit: u32,
}

impl ProjectContextParams {
    /// A briefing for `project` with every section at its schema default.
    pub fn new(project: impl Into<String>) -> Self {
        Self {
            project: project.into(),
            include_descendants: false,
            include_glossary: true,
            include_recent_notes: true,
            include_outstanding: true,
            recent_notes_limit: default_recent_notes_limit(),
            outstanding_limit: default_outstanding_limit(),
            glossary_limit: default_glossary_limit(),
        }
    }
}

/// The `GlossaryTerm` `$def`: the stored term plus the `project` that the
/// folder makes implicit (`_glossary.yml` never stores it).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GlossaryTermRef {
    pub term: String,
    pub definition: String,
    pub aliases: Vec<String>,
    pub project: String,
}

/// A note's metadata as the `NoteSummary` `$def` renders it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NoteSummary {
    pub id: String,
    pub path: String,
    pub title: String,
    #[serde(rename = "type")]
    pub note_type: crate::index::NoteType,
    pub project: Option<String>,
    pub date: String,
    pub tags: Vec<String>,
    pub source: String,
    pub confidence: Option<f64>,
    pub category: Option<crate::note::MeetingCategory>,
    pub category_confidence: Option<f64>,
    pub tracking: Option<String>,
}

impl From<&NoteRow> for NoteSummary {
    fn from(row: &NoteRow) -> Self {
        Self {
            id: row.id.clone(),
            path: row.path.clone(),
            title: row.title.clone(),
            note_type: row.note_type,
            project: row.project.clone(),
            date: row.date.clone(),
            tags: row.tags.clone(),
            source: row.source.clone(),
            confidence: row.confidence,
            category: row.category,
            category_confidence: row.category_confidence,
            tracking: row.tracking.clone(),
        }
    }
}

/// The `counts` block: true totals for the scope, independent of every
/// section's cap.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectCounts {
    pub notes: u32,
    pub meetings: u32,
    pub notes_by_type: NoteTypeCounts,
    pub outstanding_open: u32,
    pub outstanding_overdue: u32,
    pub glossary_terms: u32,
}

/// The assembled briefing — the `get_project_context` output shape.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectContext {
    pub project: ProjectSummary,
    pub description: Option<String>,
    pub glossary: Vec<GlossaryTermRef>,
    pub recent_notes: Vec<NoteSummary>,
    pub outstanding: Vec<OutstandingItem>,
    pub counts: ProjectCounts,
}

/// Why a briefing could not be assembled.
#[derive(Debug, thiserror::Error)]
pub enum ProjectContextError {
    /// The slug is not a legal project handle. A caller error the client should
    /// have rejected against the schema.
    #[error(transparent)]
    InvalidProject(#[from] note::NoteError),
    /// No such project on disk. A business fault: the caller asserted it exists.
    #[error("project not found: {project}")]
    NotFound { project: String },
    #[error(transparent)]
    Index(#[from] crate::index::IndexError),
    #[error(transparent)]
    Glossary(#[from] GlossaryError),
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Assembles a project's briefing.
///
/// `today` is supplied by the caller rather than read here (kodabi-core never
/// reads the clock), and is threaded into the outstanding-item status
/// derivation so this tool and `list_outstanding_items` classify the same item
/// identically.
pub fn get_project_context(
    index: &NoteIndex,
    vault_root: &Path,
    params: &ProjectContextParams,
    today: NaiveDate,
) -> Result<ProjectContext, ProjectContextError> {
    // Validates the slug shape and adopts the on-disk casing, like
    // `add_glossary_term` does, so `growth/q3` resolves to `Growth/Q3`.
    let canonical = vault::canonicalize_project(vault_root, &params.project)?;

    // The exact `Inbox` sentinel is a real folder and the one casing
    // `validate_project` lets through, but `list_projects` deliberately excludes
    // it — it is not a routing target, so there is no `Project` object to return
    // and its id/parent/counts would be a fiction. Reads that *filter* by
    // project still honor it as the unfiled sentinel; only this one, which must
    // return a project, rejects it. (Other casings never arrive: they are
    // reserved-name failures from `validate_project` above.)
    if canonical == INBOX {
        return Err(ProjectContextError::NotFound { project: canonical });
    }

    // One disk scan answers both "does it exist" and "what is it".
    let info = vault::list_projects(vault_root)?
        .into_iter()
        .find(|info| info.slug == canonical)
        .ok_or_else(|| ProjectContextError::NotFound {
            project: canonical.clone(),
        })?;
    let project = vault::project_summary(info);

    let scope = ProjectScope::resolve(Some(&canonical), params.include_descendants);
    let project_dir = note::project_dir(vault_root, &canonical);

    let description = read_description(&project_dir)?;

    // The glossary is the named project's own `_glossary.yml`, never a merge
    // over descendants: `add_glossary_term` writes per folder, and the schema's
    // `glossary_terms` count reads "for the project".
    let all_terms = Glossary::load(&project_dir)?;
    let glossary_terms = all_terms.terms().len() as u32;
    let glossary = if params.include_glossary {
        all_terms
            .terms()
            .iter()
            .take(params.glossary_limit.min(500) as usize)
            .map(|term| glossary_term_ref(term, &canonical))
            .collect()
    } else {
        Vec::new()
    };

    let notes_by_type = index.note_counts_by_type(&scope)?;

    let recent_notes = if params.include_recent_notes {
        index
            .recent_notes(&scope, params.recent_notes_limit.min(50))?
            .iter()
            .map(NoteSummary::from)
            .collect()
    } else {
        Vec::new()
    };

    // One shape drives both the items and the counts, so the promise that a
    // capped section's totals stay true is structural rather than coincidental.
    let outstanding_query = OutstandingParams {
        project: Some(canonical.clone()),
        include_descendants: params.include_descendants,
        status: vec![ActionItemStatus::Open, ActionItemStatus::Overdue],
        limit: params.outstanding_limit.min(100),
        ..OutstandingParams::default()
    };
    let want_items = params.include_outstanding && params.outstanding_limit > 0;
    let (outstanding, summary) = if want_items {
        let results = index.list_outstanding_items(&outstanding_query, today)?;
        (results.items, results.summary)
    } else {
        // `OutstandingParams.limit` clamps up to 1, so a zero cap cannot be
        // pushed down; ask for the totals alone and skip the page query.
        (
            Vec::new(),
            index.outstanding_summary(&outstanding_query, today)?,
        )
    };

    Ok(ProjectContext {
        project,
        description,
        glossary,
        recent_notes,
        outstanding,
        counts: ProjectCounts {
            notes: notes_by_type.total(),
            meetings: notes_by_type.meeting,
            notes_by_type,
            outstanding_open: summary.open,
            outstanding_overdue: summary.overdue,
            glossary_terms,
        },
    })
}

/// Reads a project's `README.md` as its description.
///
/// Absent or blank is `None`, not an error — most projects have no README. The
/// file is otherwise inlined verbatim, truncated to [`DESCRIPTION_MAX_CHARS`].
/// Note that `scan_project_notes` already skips a frontmatter-less `README.md`
/// as a non-note, so adopting it here costs routing nothing.
fn read_description(project_dir: &Path) -> Result<Option<String>, ProjectContextError> {
    let path = project_dir.join(DESCRIPTION_FILE);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(ProjectContextError::Io { path, source }),
    };

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    Ok(Some(truncate_chars(trimmed, DESCRIPTION_MAX_CHARS)))
}

/// Truncates on a char boundary, appending an ellipsis when anything was cut.
fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// Adds the owning project to a stored glossary term.
fn glossary_term_ref(term: &GlossaryTerm, project: &str) -> GlossaryTermRef {
    GlossaryTermRef {
        term: term.term.clone(),
        definition: term.definition.clone(),
        aliases: term.aliases.clone(),
        project: project.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glossary::OnConflict;
    use crate::index::{IndexedNote, NoteType};
    use crate::meeting::{ActionItemFact, MeetingFacts};
    use std::fs;
    use tempfile::{tempdir, TempDir};

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 7, 24).unwrap()
    }

    fn indexed(id: &str, project: &str, note_type: NoteType, date: &str) -> IndexedNote {
        IndexedNote {
            id: id.to_string(),
            path: format!("{project}/{id}.md"),
            title: format!("Title {id}"),
            note_type,
            project: Some(project.to_string()),
            date: date.to_string(),
            tags: vec![],
            source: "transcript".to_string(),
            confidence: Some(0.8),
            category: None,
            category_confidence: None,
            tracking: None,
            body: format!("body of {id}"),
            meeting: None,
        }
    }

    /// A vault with `Growth`, `Growth/Q3`, and a `Growthx` sibling on disk, and
    /// an index holding a meeting + a note in `Growth`, a chat in `Growth/Q3`,
    /// and a note in the sibling.
    fn seeded() -> (TempDir, NoteIndex) {
        let vault = tempdir().unwrap();
        for dir in ["Growth", "Growth/Q3", "Growthx"] {
            fs::create_dir_all(vault.path().join(dir)).unwrap();
        }

        let mut index = NoteIndex::open_in_memory().unwrap();
        let mut meeting = indexed("n_meet01", "Growth", NoteType::Meeting, "2026-07-10");
        meeting.meeting = Some(MeetingFacts {
            duration_seconds: Some(600),
            speaker_count: Some(2),
            decisions: vec![],
            action_items: vec![
                ActionItemFact {
                    id: "a_past01".to_string(),
                    description: "send the recap".to_string(),
                    owner: "You".to_string(),
                    due_date: Some("2026-07-01".to_string()),
                    done: false,
                    extracted_date: Some("2026-07-10".to_string()),
                },
                ActionItemFact {
                    id: "a_soon01".to_string(),
                    description: "book the room".to_string(),
                    owner: "Priya".to_string(),
                    due_date: Some("2026-08-01".to_string()),
                    done: false,
                    extracted_date: Some("2026-07-10".to_string()),
                },
                ActionItemFact {
                    id: "a_done01".to_string(),
                    description: "already handled".to_string(),
                    owner: "You".to_string(),
                    due_date: None,
                    done: true,
                    extracted_date: Some("2026-07-10".to_string()),
                },
            ],
        });
        index.upsert_note(&meeting).unwrap();
        index
            .upsert_note(&indexed("n_note01", "Growth", NoteType::Note, "2026-07-11"))
            .unwrap();

        let mut nested = indexed("n_chat01", "Growth/Q3", NoteType::Chat, "2026-07-12");
        nested.meeting = None;
        index.upsert_note(&nested).unwrap();

        index
            .upsert_note(&indexed(
                "n_sib001",
                "Growthx",
                NoteType::Note,
                "2026-07-13",
            ))
            .unwrap();

        (vault, index)
    }

    fn context(
        vault: &TempDir,
        index: &NoteIndex,
        params: &ProjectContextParams,
    ) -> ProjectContext {
        get_project_context(index, vault.path(), params, today()).unwrap()
    }

    #[test]
    fn a_briefing_reports_the_project_and_its_true_counts() {
        let (vault, index) = seeded();
        let ctx = context(&vault, &index, &ProjectContextParams::new("Growth"));

        assert_eq!(ctx.project.slug, "Growth");
        assert_eq!(ctx.project.display_name, "Growth");
        assert_eq!(ctx.project.parent, None);
        assert!(ctx.project.id.starts_with("p_"));

        // Descendants are off by default, so only the two notes directly in
        // `Growth` are counted — and never the `Growthx` sibling.
        assert_eq!(ctx.counts.notes, 2);
        assert_eq!(ctx.counts.meetings, 1);
        assert_eq!(ctx.counts.notes_by_type.meeting, 1);
        assert_eq!(ctx.counts.notes_by_type.note, 1);
        assert_eq!(ctx.counts.notes_by_type.chat, 0);
        assert_eq!(ctx.counts.notes, ctx.counts.notes_by_type.total());

        assert_eq!(ctx.counts.outstanding_open, 1);
        assert_eq!(ctx.counts.outstanding_overdue, 1);

        let ids: Vec<&str> = ctx.outstanding.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["a_past01", "a_soon01"]);
        // Newest first.
        let notes: Vec<&str> = ctx.recent_notes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(notes, ["n_note01", "n_meet01"]);
    }

    #[test]
    fn include_descendants_widens_every_index_backed_section() {
        let (vault, index) = seeded();
        let ctx = context(
            &vault,
            &index,
            &ProjectContextParams {
                include_descendants: true,
                ..ProjectContextParams::new("Growth")
            },
        );

        // The nested chat joins the counts and the recent notes; the `Growthx`
        // sibling still never does.
        assert_eq!(ctx.counts.notes, 3);
        assert_eq!(ctx.counts.notes_by_type.chat, 1);
        assert_eq!(ctx.recent_notes.len(), 3);

        // `Project.note_count` stays "directly in this project" by definition,
        // so it deliberately differs from `counts.notes` here.
        assert_eq!(ctx.project.note_count, 0); // nothing written to disk
        assert_eq!(ctx.counts.notes, 3);
    }

    #[test]
    fn a_missing_project_is_not_found() {
        let (vault, index) = seeded();
        let result = get_project_context(
            &index,
            vault.path(),
            &ProjectContextParams::new("Nope"),
            today(),
        );
        assert!(matches!(result, Err(ProjectContextError::NotFound { .. })));
    }

    #[test]
    fn the_inbox_sentinel_is_not_a_project_to_summarize() {
        let (vault, index) = seeded();
        fs::create_dir_all(vault.path().join(INBOX)).unwrap();

        // Reads that *filter* by project honor `Inbox` as "unfiled", but this
        // one must return a Project object, and there is none.
        let result = get_project_context(
            &index,
            vault.path(),
            &ProjectContextParams::new(INBOX),
            today(),
        );
        assert!(matches!(result, Err(ProjectContextError::NotFound { .. })));

        // Any other casing never even reaches that check: `validate_project`
        // rejects it as a reserved name, because a real project case-folding to
        // `Inbox` would land in the unfiled tree on Windows.
        for spelling in ["inbox", "INBOX"] {
            let result = get_project_context(
                &index,
                vault.path(),
                &ProjectContextParams::new(spelling),
                today(),
            );
            assert!(
                matches!(result, Err(ProjectContextError::InvalidProject(_))),
                "{spelling}"
            );
        }
    }

    #[test]
    fn a_malformed_slug_is_an_invalid_project_not_a_missing_one() {
        let (vault, index) = seeded();
        let result = get_project_context(
            &index,
            vault.path(),
            &ProjectContextParams::new("Growth/../etc"),
            today(),
        );
        assert!(matches!(
            result,
            Err(ProjectContextError::InvalidProject(_))
        ));
    }

    #[test]
    fn the_slug_adopts_the_on_disk_casing() {
        let (vault, index) = seeded();
        let ctx = context(&vault, &index, &ProjectContextParams::new("growth"));
        assert_eq!(ctx.project.slug, "Growth");
    }

    #[test]
    fn the_description_comes_from_the_project_readme() {
        let (vault, index) = seeded();

        // Absent by default.
        let ctx = context(&vault, &index, &ProjectContextParams::new("Growth"));
        assert_eq!(ctx.description, None);

        fs::write(
            vault.path().join("Growth").join("README.md"),
            "\n# Growth\n\nThe growth workstream.\n\n",
        )
        .unwrap();
        let ctx = context(&vault, &index, &ProjectContextParams::new("Growth"));
        assert_eq!(
            ctx.description.as_deref(),
            Some("# Growth\n\nThe growth workstream.")
        );

        // A whitespace-only README reads as no description.
        fs::write(vault.path().join("Growth").join("README.md"), "   \n\n").unwrap();
        let ctx = context(&vault, &index, &ProjectContextParams::new("Growth"));
        assert_eq!(ctx.description, None);
    }

    #[test]
    fn an_over_long_description_is_truncated_on_a_char_boundary() {
        let (vault, index) = seeded();
        // Multi-byte chars, so a naive byte slice would panic.
        let long = "é".repeat(DESCRIPTION_MAX_CHARS + 500);
        fs::write(vault.path().join("Growth").join("README.md"), &long).unwrap();

        let ctx = context(&vault, &index, &ProjectContextParams::new("Growth"));
        let description = ctx.description.unwrap();

        assert_eq!(description.chars().count(), DESCRIPTION_MAX_CHARS + 1);
        assert!(description.ends_with('…'));
    }

    #[test]
    fn the_glossary_is_read_from_the_project_folder_with_its_project_injected() {
        let (vault, index) = seeded();
        let project_dir = vault.path().join("Growth");
        let mut glossary = Glossary::default();
        for term in ["MERIDIAN", "RFP", "SOW"] {
            glossary
                .upsert(
                    GlossaryTerm {
                        term: term.to_string(),
                        definition: format!("{term} means something"),
                        aliases: vec![term.to_lowercase()],
                    },
                    OnConflict::Update,
                )
                .unwrap();
        }
        glossary.save(&project_dir).unwrap();

        let ctx = context(&vault, &index, &ProjectContextParams::new("Growth"));
        assert_eq!(ctx.glossary.len(), 3);
        assert_eq!(ctx.counts.glossary_terms, 3);
        // The file never stores the project; the `$def` requires it.
        assert!(ctx.glossary.iter().all(|t| t.project == "Growth"));
        assert_eq!(ctx.glossary[0].aliases, ["meridian"]);

        // A cap truncates the array but never the count.
        let ctx = context(
            &vault,
            &index,
            &ProjectContextParams {
                glossary_limit: 2,
                ..ProjectContextParams::new("Growth")
            },
        );
        assert_eq!(ctx.glossary.len(), 2);
        assert_eq!(ctx.counts.glossary_terms, 3);
    }

    #[test]
    fn a_missing_glossary_file_is_an_empty_glossary() {
        let (vault, index) = seeded();
        let ctx = context(&vault, &index, &ProjectContextParams::new("Growth"));
        assert!(ctx.glossary.is_empty());
        assert_eq!(ctx.counts.glossary_terms, 0);
    }

    #[test]
    fn switching_a_section_off_leaves_its_counts_true() {
        let (vault, index) = seeded();
        let ctx = context(
            &vault,
            &index,
            &ProjectContextParams {
                include_glossary: false,
                include_recent_notes: false,
                include_outstanding: false,
                ..ProjectContextParams::new("Growth")
            },
        );

        assert!(ctx.glossary.is_empty());
        assert!(ctx.recent_notes.is_empty());
        assert!(ctx.outstanding.is_empty());
        // The briefing's whole point: a caller that drops a section still learns
        // how much is there, so it knows whether to fetch it separately.
        assert_eq!(ctx.counts.notes, 2);
        assert_eq!(ctx.counts.outstanding_open, 1);
        assert_eq!(ctx.counts.outstanding_overdue, 1);
    }

    #[test]
    fn a_zero_limit_omits_a_section_without_touching_its_counts() {
        let (vault, index) = seeded();
        let ctx = context(
            &vault,
            &index,
            &ProjectContextParams {
                recent_notes_limit: 0,
                outstanding_limit: 0,
                ..ProjectContextParams::new("Growth")
            },
        );

        assert!(ctx.recent_notes.is_empty());
        assert!(ctx.outstanding.is_empty());
        assert_eq!(ctx.counts.notes, 2);
        // Notably not 0: `OutstandingParams` clamps a limit up to 1, so the
        // zero case must take the summary-only path to stay honest.
        assert_eq!(ctx.counts.outstanding_open, 1);
        assert_eq!(ctx.counts.outstanding_overdue, 1);
    }

    #[test]
    fn a_section_cap_truncates_items_but_not_counts() {
        let (vault, index) = seeded();
        let ctx = context(
            &vault,
            &index,
            &ProjectContextParams {
                recent_notes_limit: 1,
                outstanding_limit: 1,
                ..ProjectContextParams::new("Growth")
            },
        );

        assert_eq!(ctx.recent_notes.len(), 1);
        assert_eq!(ctx.outstanding.len(), 1);
        assert_eq!(ctx.counts.notes, 2);
        assert_eq!(
            ctx.counts.outstanding_open + ctx.counts.outstanding_overdue,
            2
        );
    }

    #[test]
    fn an_empty_project_still_returns_a_project_with_zero_counts() {
        let (vault, index) = seeded();
        fs::create_dir_all(vault.path().join("Quiet")).unwrap();

        // The disk is the existence authority: a folder with nothing indexed is
        // a real project, not a NotFound.
        let ctx = context(&vault, &index, &ProjectContextParams::new("Quiet"));
        assert_eq!(ctx.project.slug, "Quiet");
        assert_eq!(ctx.counts.notes, 0);
        assert_eq!(ctx.counts.outstanding_open, 0);
        assert!(ctx.recent_notes.is_empty());
    }

    #[test]
    fn a_nested_project_reports_its_parent() {
        let (vault, index) = seeded();
        let ctx = context(&vault, &index, &ProjectContextParams::new("Growth/Q3"));

        assert_eq!(ctx.project.slug, "Growth/Q3");
        assert_eq!(ctx.project.display_name, "Q3");
        assert_eq!(ctx.project.parent.as_deref(), Some("Growth"));
        assert_eq!(ctx.counts.notes_by_type.chat, 1);
    }

    #[test]
    fn the_briefing_serializes_to_the_documented_shape() {
        let (vault, index) = seeded();
        let ctx = context(&vault, &index, &ProjectContextParams::new("Growth"));
        let value = serde_json::to_value(&ctx).unwrap();

        assert_eq!(value["project"]["slug"], "Growth");
        assert_eq!(value["description"], serde_json::Value::Null);
        assert!(value["counts"]["notes_by_type"]["meeting"].is_number());
        // `NoteSummary` renders its type under the `type` key.
        assert_eq!(value["recent_notes"][0]["type"], "note");
        assert_eq!(value["outstanding"][0]["status"], "overdue");
        assert_eq!(value["outstanding"][0]["source"]["id"], "n_meet01");
        // Deliberately no pagination envelope on this tool.
        assert!(value.get("page").is_none());
    }
}
