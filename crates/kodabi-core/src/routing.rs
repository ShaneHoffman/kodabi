//! Confidence-split routing: score a distilled note against the vault's
//! projects and decide where it files (`docs/FOUNDING_DOC.md` §3.5).
//!
//! Miscategorized notes are worse than uncategorized ones, so the split is
//! biased toward Inbox: a note files directly into a project only when its
//! confidence clears a threshold, and everything else lands in
//! [`INBOX`](crate::note::INBOX) with the score still recorded — the score is
//! *why* it landed there (`docs/FRONTMATTER_SCHEMA.md`).
//!
//! The scorer is deterministic and purely lexical: each candidate project
//! earns weight from distinct glossary term/alias matches and mentions of its
//! own name, a margin rule subtracts the runner-up's weight so evidence split
//! across projects reads as *low* confidence, and a saturating curve maps the
//! net weight into `[0, 1)`. There is no model call anywhere in this module;
//! the end-of-meeting distill pass can later blend a proposal in as one more
//! additive signal without changing [`route`]'s contract.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

use crate::glossary::{Glossary, GlossaryError};
use crate::note::{project_dir, validate_project, Routing, INBOX, RESERVED_ROOT_DIRS};

/// Weight of one distinct glossary entry (term or any alias) matched in the
/// body.
const TERM_WEIGHT: f64 = 1.0;
/// Weight of one matched project-name path segment in the body. A literal name
/// mention is a stronger cue than one vocabulary hit — but deliberately weak
/// enough that a lone body mention stays below the default threshold: projects
/// get name-dropped in other projects' meetings.
const NAME_WEIGHT: f64 = 2.0;
/// Multiplier applied when a signal matches in the title instead of the body.
/// Titles are short and routing-dense ("Briarwood Golf weekly sync"); bodies
/// are long and mention other projects in passing.
const TITLE_MULTIPLIER: f64 = 2.0;
/// Saturation constant `K` in `evidence = w / (w + K)` — diminishing returns.
const SATURATION_WEIGHT: f64 = 2.0;

/// Compiled-in default threshold: `saturate(3.0)` by construction, i.e.
/// auto-filing requires net evidence worth three unopposed body-level signals
/// (`3/(3+K)`, `0.6` at the current `K = 2`). Written in terms of
/// [`SATURATION_WEIGHT`] so retuning the curve keeps that meaning instead of
/// silently changing what the threshold demands. The future
/// `KODABI_ROUTING_THRESHOLD` env override is applied at the src-tauri
/// boundary, never read here — core takes config as a parameter.
pub const DEFAULT_THRESHOLD: f64 = 3.0 / (3.0 + SATURATION_WEIGHT);

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Tuning for the confidence split. Constructed by the caller; the src-tauri
/// boundary owns any env-var override (mirroring how other pipeline knobs are
/// resolved outside core, `docs/RESOURCE_BUDGET.md`).
#[derive(Debug, Clone)]
pub struct RoutingConfig {
    /// Minimum confidence to file directly into a project; anything below it
    /// lands in Inbox with the score recorded. Sane range `(0.0, 1.0]`.
    pub threshold: f64,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            threshold: DEFAULT_THRESHOLD,
        }
    }
}

impl RoutingConfig {
    /// `threshold` when finite and in `(0.0, 1.0]`, else [`DEFAULT_THRESHOLD`]
    /// — a bad value falls back rather than breaking the pipeline. Excluding
    /// `0.0` keeps a dead tie (confidence `0.0`) from ever auto-filing.
    fn effective_threshold(&self) -> f64 {
        if self.threshold.is_finite() && self.threshold > 0.0 && self.threshold <= 1.0 {
            self.threshold
        } else {
            DEFAULT_THRESHOLD
        }
    }
}

// ---------------------------------------------------------------------------
// Inputs & outputs
// ---------------------------------------------------------------------------

/// The distilled note text being routed. Title and body are separate because
/// they carry different signal density (see [`TITLE_MULTIPLIER`]); a caller
/// without a title passes `None` and degrades cleanly.
#[derive(Debug, Clone, Copy)]
pub struct NoteText<'a> {
    pub title: Option<&'a str>,
    pub body: &'a str,
}

/// One candidate project and the routing signals it contributes. Shaped so a
/// future signal (the distill pass's model proposal) is one more field and
/// weight, not a redesign.
#[derive(Debug, Clone)]
pub struct ProjectSignals {
    /// Frontmatter project slug, e.g. `"Briarwood Golf"` or `"Growth/Q3"`.
    pub project: String,
    /// The project's glossary (empty when it has no `_glossary.yml`).
    pub glossary: Glossary,
}

/// One candidate's evidence tally — diagnostics for tests, the Inbox UI's
/// "why did this land here", and the surface a future signal blender reads.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectScore {
    pub project: String,
    /// Distinct glossary entries matched (term or alias; each entry counts
    /// once, at its best location).
    pub glossary_hits: usize,
    /// Distinct project-name path segments matched.
    pub name_hits: usize,
    /// Raw summed signal weight (the margin rule operates on this).
    pub weight: f64,
    /// `saturate(weight)` in `[0, 1)` — this candidate's standalone evidence.
    /// Not the final confidence: [`route`] subtracts the runner-up first.
    pub evidence: f64,
}

/// Errors produced while discovering projects or loading their signals.
#[derive(Debug, thiserror::Error)]
pub enum RoutingError {
    #[error("project discovery I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Glossary(#[from] GlossaryError),
}

// ---------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------

/// Lowercased word tokens: NFC-composed first (a decomposed `Cafe´` and a
/// precomposed `Café` are the same text on screen and must tokenize alike —
/// combining marks are non-alphanumeric and would otherwise become split
/// points), then split on any non-alphanumeric char (Unicode-aware) and
/// case-folded with `char::to_lowercase` — the same fold `glossary`'s lookups
/// use, so what matches here agrees with what the glossary considers equal.
/// Marks with no precomposed form still split, but identically on both the
/// needle and haystack side.
fn tokens(text: &str) -> Vec<String> {
    let composed: String = text.nfc().collect();
    composed
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.chars().flat_map(char::to_lowercase).collect())
        .collect()
}

/// Whole-word contiguous phrase match: `"tee sheet"` matches "Tee-Sheet" but
/// `"golf"` never matches "golfing". A needle that tokenized to nothing never
/// matches anything, and neither does one that tokenized down to a single
/// one-character word: punctuation-heavy vocabulary ("C#", "A+") degrades to a
/// bare letter under [`tokens`], and matching every stray "c" would award a
/// full signal to text that never mentioned the term.
fn contains_phrase(haystack: &[String], needle: &[String]) -> bool {
    let degenerate = needle.is_empty() || (needle.len() == 1 && needle[0].chars().count() == 1);
    !degenerate
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// Where a signal matched. A signal counts once, at its best location — a
/// needle present in both title and body earns only the title weight.
#[derive(Debug, Clone, Copy)]
enum HitLocation {
    Title,
    Body,
}

impl HitLocation {
    fn multiplier(self) -> f64 {
        match self {
            HitLocation::Title => TITLE_MULTIPLIER,
            HitLocation::Body => 1.0,
        }
    }
}

/// Best location where any of `needles` (alternate spellings of one signal)
/// matches: title beats body, absent beats neither.
fn best_location(
    title: &[String],
    body: &[String],
    needles: &[Vec<String>],
) -> Option<HitLocation> {
    if needles.iter().any(|n| contains_phrase(title, n)) {
        return Some(HitLocation::Title);
    }
    if needles.iter().any(|n| contains_phrase(body, n)) {
        return Some(HitLocation::Body);
    }
    None
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

/// Saturating evidence curve: monotone in `weight`, diminishing returns, and
/// asymptotically below `1.0` — a lexical scorer never claims the certainty a
/// human correction records (`confidence: 1.0`).
fn saturate(weight: f64) -> f64 {
    weight / (weight + SATURATION_WEIGHT)
}

/// Scores every candidate against the note text, sorted by
/// `(weight desc, project asc)` — a total, deterministic order even between
/// equal weights.
pub fn score_projects(text: NoteText<'_>, candidates: &[ProjectSignals]) -> Vec<ProjectScore> {
    let title = text.title.map(tokens).unwrap_or_default();
    let body = tokens(text.body);

    let mut scores: Vec<ProjectScore> = candidates
        .iter()
        .map(|candidate| score_candidate(&title, &body, candidate))
        .collect();
    scores.sort_by(|a, b| {
        b.weight
            .total_cmp(&a.weight)
            .then_with(|| a.project.cmp(&b.project))
    });
    scores
}

fn score_candidate(title: &[String], body: &[String], candidate: &ProjectSignals) -> ProjectScore {
    let mut glossary_hits = 0;
    let mut name_hits = 0;
    let mut weight = 0.0;

    // One signal per distinct path segment: "Briarwood Golf" is a single
    // multi-token phrase; "Growth/Q3" contributes "growth" and "q3"
    // separately, so a title like "Growth Q3 planning" hits both. A hit weighs
    // only toward this exact candidate — no cascade from `Growth/Q3` up to
    // `Growth`, which would inflate parents and worsen exactly the ambiguity
    // the margin rule exists to catch.
    let mut segment_needles: Vec<Vec<String>> = Vec::new();
    for segment in candidate.project.split('/') {
        let needle = tokens(segment);
        if !needle.is_empty() && !segment_needles.contains(&needle) {
            segment_needles.push(needle);
        }
    }

    // The reverse cascade needs breaking too: the leaf segment is this
    // project's own name, while ancestor segments appear in every descendant's
    // slug. Crediting an ancestor hit without the leaf would hand each child a
    // copy of its parent's name evidence ("Growth planning" scoring
    // `Growth/Q3` as high as `Growth` — a guaranteed tie that parks every
    // parent-project note in Inbox). So ancestor segments count only when the
    // leaf matched: without the leaf, the mention is the ancestor's evidence,
    // not this candidate's.
    let leaf = candidate
        .project
        .rsplit('/')
        .next()
        .map(tokens)
        .unwrap_or_default();
    if best_location(title, body, std::slice::from_ref(&leaf)).is_some() {
        for needle in &segment_needles {
            if let Some(location) = best_location(title, body, std::slice::from_ref(needle)) {
                name_hits += 1;
                weight += NAME_WEIGHT * location.multiplier();
            }
        }
    }

    // One signal per glossary *entry*: its term plus every alias are alternate
    // spellings of the same concept, so together they count once. Distinct
    // entries (not occurrences) make the score reward breadth of project
    // vocabulary, not verbosity. And one signal per *spelling* overall: a
    // needle already claimed — by the project name (an entry that is just the
    // project's own name must not turn one mention into TERM_WEIGHT +
    // NAME_WEIGHT and clear the threshold on a lone name-drop) or by an
    // earlier entry (two entries sharing an alias) — never counts again.
    let mut claimed = segment_needles;
    for entry in candidate.glossary.terms() {
        let needles: Vec<Vec<String>> = std::iter::once(entry.term.as_str())
            .chain(entry.aliases.iter().map(String::as_str))
            .map(tokens)
            .filter(|needle| !claimed.contains(needle))
            .collect();
        if let Some(location) = best_location(title, body, &needles) {
            glossary_hits += 1;
            weight += TERM_WEIGHT * location.multiplier();
        }
        claimed.extend(needles);
    }

    ProjectScore {
        project: candidate.project.clone(),
        glossary_hits,
        name_hits,
        weight,
        evidence: saturate(weight),
    }
}

// ---------------------------------------------------------------------------
// The split
// ---------------------------------------------------------------------------

/// The confidence split. Confidence is the *margin* of the top candidate over
/// the runner-up, saturated: `saturate(w1 - w2)`. Subtracting before
/// saturating penalizes competition at full strength — evidence split across
/// projects reads as low confidence, and a dead tie is `0.0`.
///
/// Always returns [`Routing::Routed`] (never `Manual`), with the confidence
/// recorded even for an Inbox landing — the score is why it landed there — so
/// the writer's Inbox-requires-confidence invariant holds by construction.
pub fn route(text: NoteText<'_>, candidates: &[ProjectSignals], config: &RoutingConfig) -> Routing {
    let scores = score_projects(text, candidates);
    let Some(top) = scores.first().filter(|top| top.weight > 0.0) else {
        // No candidates at all, or nothing matched anything: uncategorized
        // beats miscategorized, with the zero score on record.
        return inbox_landing(0.0);
    };
    // The margin is against the best *other* project: candidates are
    // caller-supplied and nothing forbids duplicates, and a duplicate of the
    // winner as its own runner-up would zero out the confidence of an
    // unopposed match.
    let runner_up = scores
        .iter()
        .skip(1)
        .find(|s| s.project != top.project)
        .map_or(0.0, |s| s.weight);
    let confidence = saturate(top.weight - runner_up);
    // A slug the writer would reject (candidates need not come from
    // discovery, which is the only validated source) must not surface as a
    // `Routed` target — `Note::new` would fail the whole write, losing the
    // note. Uncategorized beats lost: it lands in Inbox with the score on
    // record, like every other low-quality outcome.
    if confidence >= config.effective_threshold() && validate_project(&top.project).is_ok() {
        Routing::Routed {
            project: top.project.clone(),
            confidence,
        }
    } else {
        inbox_landing(confidence)
    }
}

fn inbox_landing(confidence: f64) -> Routing {
    Routing::Routed {
        project: INBOX.to_string(),
        confidence,
    }
}

// ---------------------------------------------------------------------------
// Project discovery
// ---------------------------------------------------------------------------

/// Whether a directory name is excluded from project discovery outright.
/// Dot- and underscore-prefixed folders are infra at any depth; the `Inbox`
/// sentinel folder and the reserved root dirs (`note::RESERVED_ROOT_DIRS`)
/// are reserved at the vault root only (any casing — the filesystem this
/// ships on is case-insensitive, matching `validate_project`'s reservations)
/// — a nested `Data/raw` is a legitimate project.
fn is_excluded_dir_name(name: &str, top_level: bool) -> bool {
    if name.starts_with('.') || name.starts_with('_') {
        return true;
    }
    top_level
        && (name.eq_ignore_ascii_case(INBOX)
            || RESERVED_ROOT_DIRS
                .iter()
                .any(|reserved| name.eq_ignore_ascii_case(reserved)))
}

/// Walks `vault_root` for project folders, returned as `/`-joined slugs sorted
/// lexicographically.
///
/// Every surviving directory at every depth is a candidate — both `Growth`
/// and `Growth/Q3` — because on disk there is no way to tell a namespace from
/// a project, and a wrong guess would silently remove a candidate; a junk
/// subfolder simply scores `0.0` and is harmless. Candidates whose slug fails
/// the project rules (`note::validate_project`) are skipped, subtree included:
/// `Note::new` could never file into them anyway. I/O failures are errors, not
/// skips — a silently missing project turns "route to X" into "route to Y or
/// Inbox", the worst failure mode this module has.
pub fn discover_projects(vault_root: &Path) -> Result<Vec<String>, RoutingError> {
    let mut projects = Vec::new();
    walk_projects(vault_root, None, &mut projects)?;
    projects.sort();
    Ok(projects)
}

fn walk_projects(
    dir: &Path,
    slug_prefix: Option<&str>,
    projects: &mut Vec<String>,
) -> Result<(), RoutingError> {
    let io_err = |source| RoutingError::Io {
        path: dir.to_path_buf(),
        source,
    };

    let mut names: Vec<String> = Vec::new();
    for entry in fs::read_dir(dir).map_err(io_err)? {
        let entry = entry.map_err(io_err)?;
        // `file_type` does not follow symlinks, so a link to a directory is
        // skipped here — which also rules out walk cycles.
        if !entry.file_type().map_err(io_err)?.is_dir() {
            continue;
        }
        // A name that isn't valid UTF-8 can never round-trip through a
        // frontmatter slug, so it cannot be a project.
        if let Ok(name) = entry.file_name().into_string() {
            names.push(name);
        }
    }
    // `read_dir` order is OS-dependent; sort so it never leaks into results.
    names.sort();

    for name in names {
        if is_excluded_dir_name(&name, slug_prefix.is_none()) {
            continue;
        }
        let slug = match slug_prefix {
            Some(prefix) => format!("{prefix}/{name}"),
            None => name.clone(),
        };
        // An illegal segment poisons every deeper slug too (`validate_project`
        // checks all segments), so don't recurse under one.
        if validate_project(&slug).is_err() {
            continue;
        }
        projects.push(slug.clone());
        walk_projects(&dir.join(&name), Some(&slug), projects)?;
    }
    Ok(())
}

/// [`discover_projects`] plus each project's glossary — the one-call signal
/// loader for [`route`]. A project without a `_glossary.yml` contributes an
/// empty glossary; a malformed or duplicate-term file is surfaced as an error
/// (a user-fixable mistake worth reporting, not routing around).
pub fn load_project_signals(vault_root: &Path) -> Result<Vec<ProjectSignals>, RoutingError> {
    let projects = discover_projects(vault_root)?;
    let mut signals = Vec::with_capacity(projects.len());
    for project in projects {
        // `note::project_dir` is the writer's slug→folder mapping; using it
        // here keeps glossaries loading from exactly the folder notes land in.
        let glossary = Glossary::load(&project_dir(vault_root, &project))?;
        signals.push(ProjectSignals { project, glossary });
    }
    Ok(signals)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glossary::{GlossaryTerm, OnConflict};
    use crate::note::{Note, NoteId, NoteType, Source};
    use tempfile::tempdir;

    fn glossary(entries: &[(&str, &[&str])]) -> Glossary {
        let mut glossary = Glossary::default();
        for (term, aliases) in entries {
            glossary
                .upsert(
                    GlossaryTerm {
                        term: term.to_string(),
                        definition: String::new(),
                        aliases: aliases.iter().map(|a| a.to_string()).collect(),
                    },
                    OnConflict::Error,
                )
                .unwrap();
        }
        glossary
    }

    fn signals(project: &str, entries: &[(&str, &[&str])]) -> ProjectSignals {
        ProjectSignals {
            project: project.to_string(),
            glossary: glossary(entries),
        }
    }

    /// The two-project fixture the worked examples in the module design use:
    /// a term-rich `Briarwood Golf` and a smaller `Growth/Q3`.
    fn fixture() -> Vec<ProjectSignals> {
        vec![
            signals(
                "Briarwood Golf",
                &[
                    ("MERIDIAN", &[]),
                    ("TeeTrack", &["t-track"]),
                    ("GreenFlow", &[]),
                    ("irrigation", &[]),
                    ("tee sheet", &[]),
                ],
            ),
            signals("Growth/Q3", &[("OKR", &[]), ("activation", &[])]),
        ]
    }

    fn body(text: &str) -> NoteText<'_> {
        NoteText {
            title: None,
            body: text,
        }
    }

    fn titled<'a>(title: &'a str, body: &'a str) -> NoteText<'a> {
        NoteText {
            title: Some(title),
            body,
        }
    }

    fn score_of<'a>(scores: &'a [ProjectScore], project: &str) -> &'a ProjectScore {
        scores
            .iter()
            .find(|s| s.project == project)
            .unwrap_or_else(|| panic!("no score for {project:?}"))
    }

    // -- matching ----------------------------------------------------------

    #[test]
    fn phrase_match_is_case_insensitive_and_whole_word() {
        let candidates = [signals(
            "Briarwood Golf",
            &[("GreenFlow", &[]), ("golf", &[])],
        )];

        let scores = score_projects(body("the greenflow systems demo"), &candidates);
        assert_eq!(score_of(&scores, "Briarwood Golf").glossary_hits, 1);

        // Substrings never match: "golf" ≠ "golfing", "GreenFlow" ≠ "Flow".
        let scores = score_projects(body("golfing with flow"), &candidates);
        assert_eq!(score_of(&scores, "Briarwood Golf").weight, 0.0);
    }

    #[test]
    fn multi_word_and_hyphenated_phrases_match() {
        let candidates = [signals(
            "Briarwood Golf",
            &[("tee sheet", &[]), ("TeeTrack", &["t-track"])],
        )];

        let scores = score_projects(body("the Tee-Sheet import from T-Track"), &candidates);
        let score = score_of(&scores, "Briarwood Golf");
        assert_eq!(score.glossary_hits, 2);
        assert_eq!(score.weight, 2.0 * TERM_WEIGHT);

        // The slug's own tokens match when written as a path in prose.
        let candidates = [signals("Growth/Q3", &[])];
        let scores = score_projects(body("re: growth/q3 review"), &candidates);
        assert_eq!(score_of(&scores, "Growth/Q3").name_hits, 2);
    }

    #[test]
    fn unicode_case_folds_like_glossary() {
        let candidates = [signals("Cafe", &[("Café Renovation", &[])])];
        let scores = score_projects(body("the CAFÉ renovation budget"), &candidates);
        assert_eq!(score_of(&scores, "Cafe").glossary_hits, 1);
    }

    // -- scoring -----------------------------------------------------------

    #[test]
    fn no_matches_scores_zero_for_every_candidate() {
        let scores = score_projects(body("nothing relevant here"), &fixture());
        for score in &scores {
            assert_eq!(score.weight, 0.0);
            assert_eq!(score.evidence, 0.0);
            assert_eq!(score.glossary_hits, 0);
            assert_eq!(score.name_hits, 0);
        }
    }

    #[test]
    fn repeat_occurrences_and_aliases_count_one_entry_once() {
        let candidates = [signals("Briarwood Golf", &[("TeeTrack", &["t-track"])])];
        let scores = score_projects(body("TeeTrack aka t-track; TeeTrack again"), &candidates);
        let score = score_of(&scores, "Briarwood Golf");
        assert_eq!(score.glossary_hits, 1);
        assert_eq!(score.weight, TERM_WEIGHT);
    }

    #[test]
    fn evidence_is_monotone_with_diminishing_returns() {
        let evidence: Vec<f64> = (1..=5).map(|w| saturate(w as f64)).collect();
        for pair in evidence.windows(2) {
            assert!(pair[1] > pair[0], "evidence must be strictly increasing");
        }
        let deltas: Vec<f64> = evidence.windows(2).map(|p| p[1] - p[0]).collect();
        for pair in deltas.windows(2) {
            assert!(pair[1] < pair[0], "returns must diminish");
        }
        assert!(evidence.iter().all(|e| *e < 1.0));
    }

    #[test]
    fn title_hits_weigh_double_and_do_not_stack_with_body() {
        let candidates = [signals("Briarwood Golf", &[("GreenFlow", &[])])];
        let scores = score_projects(titled("GreenFlow review", "GreenFlow again"), &candidates);
        let score = score_of(&scores, "Briarwood Golf");
        assert_eq!(score.glossary_hits, 1);
        assert_eq!(score.weight, TERM_WEIGHT * TITLE_MULTIPLIER);
    }

    #[test]
    fn name_segment_hits_score_name_weight() {
        // A multi-word project name is a single phrase needle…
        let candidates = [signals("Briarwood Golf", &[])];
        let scores = score_projects(body("mentioned Briarwood Golf in passing"), &candidates);
        let score = score_of(&scores, "Briarwood Golf");
        assert_eq!(score.name_hits, 1);
        assert_eq!(score.weight, NAME_WEIGHT);
        // …so half of it alone is no hit.
        let scores = score_projects(body("briarwood lane"), &candidates);
        assert_eq!(score_of(&scores, "Briarwood Golf").weight, 0.0);

        // A hierarchical slug scores per segment: "q3" alone hits `Growth/Q3`
        // but not `Growth`.
        let candidates = [signals("Growth", &[]), signals("Growth/Q3", &[])];
        let scores = score_projects(body("the q3 numbers"), &candidates);
        assert_eq!(score_of(&scores, "Growth/Q3").weight, NAME_WEIGHT);
        assert_eq!(score_of(&scores, "Growth").weight, 0.0);
    }

    #[test]
    fn degenerate_single_letter_needles_never_match() {
        // "C#" and "A+" tokenize down to bare letters; matching every stray
        // "c"/"a" would score text that never mentioned the term.
        let candidates = [signals("DevTools", &[("C#", &[]), ("A+", &[])])];
        let scores = score_projects(
            titled("A plan C review", "option c and a word"),
            &candidates,
        );
        assert_eq!(score_of(&scores, "DevTools").weight, 0.0);
    }

    #[test]
    fn project_name_in_own_glossary_counts_once_as_name() {
        // A glossary entry that is the project's own name must not turn one
        // mention into TERM_WEIGHT + NAME_WEIGHT — that would auto-file a lone
        // body name-drop at exactly the default threshold.
        let candidates = [signals("Irrigation", &[("irrigation", &[])])];
        let text = body("mentioned irrigation in passing");

        let scores = score_projects(text, &candidates);
        let score = score_of(&scores, "Irrigation");
        assert_eq!(score.name_hits, 1);
        assert_eq!(score.glossary_hits, 0);
        assert_eq!(score.weight, NAME_WEIGHT);

        let routing = route(text, &candidates, &RoutingConfig::default());
        assert_eq!(routing.project(), INBOX);
        assert_eq!(routing.confidence(), Some(0.5));
    }

    #[test]
    fn alias_shared_by_two_entries_counts_once() {
        // Only term text is deduplicated at glossary load; a shared alias must
        // not turn one occurrence into two hits' worth of weight.
        let candidates = [signals(
            "Briarwood Golf",
            &[("TeeTrack", &["FU"]), ("FollowUp", &["FU"])],
        )];
        let scores = score_projects(body("the fu rollout"), &candidates);
        let score = score_of(&scores, "Briarwood Golf");
        assert_eq!(score.glossary_hits, 1);
        assert_eq!(score.weight, TERM_WEIGHT);
    }

    #[test]
    fn parent_only_evidence_routes_to_the_parent_not_a_tie() {
        // The child inherits the parent's segment in its slug; without this
        // being leaf-gated, "Growth planning" would tie Growth with Growth/Q3
        // and park every parent-project note in Inbox.
        let candidates = [signals("Growth", &[]), signals("Growth/Q3", &[])];
        let text = titled("Growth planning", "agenda to follow");

        let scores = score_projects(text, &candidates);
        assert_eq!(
            score_of(&scores, "Growth").weight,
            NAME_WEIGHT * TITLE_MULTIPLIER
        );
        assert_eq!(score_of(&scores, "Growth/Q3").weight, 0.0);

        let routing = route(text, &candidates, &RoutingConfig::default());
        assert_eq!(
            routing,
            Routing::Routed {
                project: "Growth".to_string(),
                confidence: 4.0 / 6.0,
            }
        );
    }

    #[test]
    fn nfc_and_nfd_forms_of_the_same_text_match() {
        // Decomposed é (e + U+0301) and precomposed é are the same text on
        // screen; neither side's Unicode form may decide whether a term hits.
        let nfc_term = [signals("Cafe", &[("Caf\u{e9} Renovation", &[])])];
        let scores = score_projects(body("the cafe\u{301} renovation budget"), &nfc_term);
        assert_eq!(score_of(&scores, "Cafe").glossary_hits, 1);

        let nfd_term = [signals("Cafe", &[("Cafe\u{301} Renovation", &[])])];
        let scores = score_projects(body("the caf\u{e9} renovation budget"), &nfd_term);
        assert_eq!(score_of(&scores, "Cafe").glossary_hits, 1);
    }

    #[test]
    fn glossary_hits_do_not_cascade_to_parent_project() {
        let candidates = [
            signals("Growth", &[]),
            signals("Growth/Q3", &[("activation", &[])]),
        ];
        let scores = score_projects(body("activation metrics"), &candidates);
        assert_eq!(score_of(&scores, "Growth/Q3").weight, TERM_WEIGHT);
        assert_eq!(score_of(&scores, "Growth").weight, 0.0);
    }

    #[test]
    fn worked_examples_produce_exact_confidences() {
        let candidates = fixture();
        let config = RoutingConfig::default();

        // Five distinct terms, unopposed: saturate(5) = 5/7.
        let five_terms =
            "MERIDIAN rollout: TeeTrack sync for the tee sheet, GreenFlow irrigation checks.";
        let routing = route(body(five_terms), &candidates, &config);
        assert_eq!(
            routing,
            Routing::Routed {
                project: "Briarwood Golf".to_string(),
                confidence: 5.0 / 7.0,
            }
        );

        // Three terms against one stray competitor term: saturate(3 - 1) = 0.5.
        let contested = "MERIDIAN kickoff with TeeTrack on the tee sheet; timeline follows OKR.";
        let routing = route(body(contested), &candidates, &config);
        assert_eq!(routing.confidence(), Some(0.5));
        assert_eq!(routing.project(), INBOX);

        // Title name-mention plus one body term: 2·2 + 1 = 5 → 5/7 again.
        let routing = route(
            titled("Briarwood Golf weekly sync", "GreenFlow status is nominal."),
            &candidates,
            &config,
        );
        assert_eq!(
            routing,
            Routing::Routed {
                project: "Briarwood Golf".to_string(),
                confidence: 5.0 / 7.0,
            }
        );
    }

    // -- the split ---------------------------------------------------------

    #[test]
    fn no_evidence_and_no_candidates_route_to_inbox_with_zero_confidence() {
        let config = RoutingConfig::default();
        let expected = Routing::Routed {
            project: INBOX.to_string(),
            confidence: 0.0,
        };

        assert_eq!(route(body("anything at all"), &[], &config), expected);
        assert_eq!(route(body(""), &fixture(), &config), expected);
        assert_eq!(
            route(body("nothing relevant"), &fixture(), &config),
            expected
        );
    }

    #[test]
    fn split_evidence_routes_to_inbox_with_low_recorded_confidence() {
        // Two entries each on two projects: a dead tie, confidence 0.0.
        let tied = "MERIDIAN and TeeTrack progress blocked on OKR activation review.";
        let routing = route(body(tied), &fixture(), &RoutingConfig::default());
        assert_eq!(
            routing,
            Routing::Routed {
                project: INBOX.to_string(),
                confidence: 0.0,
            }
        );
    }

    #[test]
    fn a_bare_body_name_drop_is_not_enough_to_file() {
        // NAME_WEIGHT alone saturates to 0.5, below the default threshold by
        // design: projects get name-dropped in other projects' meetings.
        let routing = route(
            body("mentioned Briarwood Golf in passing"),
            &fixture(),
            &RoutingConfig::default(),
        );
        assert_eq!(routing.project(), INBOX);
        assert_eq!(routing.confidence(), Some(0.5));
    }

    #[test]
    fn confidence_exactly_at_threshold_routes() {
        // Three unopposed body terms: saturate(3) = 3/5 == DEFAULT_THRESHOLD
        // exactly in f64, exercising the `>=` boundary.
        let three_terms = "MERIDIAN update, TeeTrack demo, GreenFlow next";
        let routing = route(body(three_terms), &fixture(), &RoutingConfig::default());
        assert_eq!(
            routing,
            Routing::Routed {
                project: "Briarwood Golf".to_string(),
                confidence: DEFAULT_THRESHOLD,
            }
        );
    }

    #[test]
    fn tie_routes_to_inbox_and_score_order_is_deterministic() {
        let tied = "MERIDIAN and TeeTrack progress blocked on OKR activation review.";
        let scores = score_projects(body(tied), &fixture());
        // Equal weights order by project name; the tie itself lands in Inbox
        // even with the threshold floored, because a tie's confidence is 0.0
        // and the effective threshold can never be 0.0.
        assert_eq!(scores[0].project, "Growth/Q3");
        assert_eq!(scores[1].project, "Briarwood Golf");
        assert_eq!(scores[0].weight, scores[1].weight);

        let routing = route(body(tied), &fixture(), &RoutingConfig { threshold: 0.0 });
        assert_eq!(routing.project(), INBOX);
    }

    #[test]
    fn invalid_candidate_slug_falls_back_to_inbox() {
        // Candidates need not come from validated discovery; a slug the writer
        // would reject ("con" is a reserved Windows device name) must land in
        // Inbox with the score recorded, not surface as a Routed target that
        // fails the note write downstream.
        let candidates = [signals(
            "con",
            &[("MERIDIAN", &[]), ("TeeTrack", &[]), ("GreenFlow", &[])],
        )];
        let routing = route(
            body("MERIDIAN update, TeeTrack demo, GreenFlow next"),
            &candidates,
            &RoutingConfig::default(),
        );
        assert_eq!(routing.project(), INBOX);
        assert_eq!(routing.confidence(), Some(DEFAULT_THRESHOLD));
    }

    #[test]
    fn duplicate_candidates_do_not_zero_the_margin() {
        // A caller merging signal lists can pass the same project twice; the
        // duplicate must not become its own runner-up and drag an unopposed
        // winner's confidence to 0.0.
        let duplicated: Vec<ProjectSignals> = vec![fixture()[0].clone(), fixture()[0].clone()];
        let routing = route(
            body("MERIDIAN update, TeeTrack demo, GreenFlow next"),
            &duplicated,
            &RoutingConfig::default(),
        );
        assert_eq!(
            routing,
            Routing::Routed {
                project: "Briarwood Golf".to_string(),
                confidence: DEFAULT_THRESHOLD,
            }
        );
    }

    #[test]
    fn invalid_threshold_falls_back_to_default() {
        // 5/7 ≈ 0.714 clears the default; 0.5 does not. Every invalid
        // threshold must behave exactly like the default on both sides.
        let strong = "MERIDIAN rollout: TeeTrack sync for the tee sheet, GreenFlow irrigation checks.";
        let weak = "mentioned Briarwood Golf in passing";
        for threshold in [f64::NAN, f64::INFINITY, 0.0, -1.0, 1.5] {
            let config = RoutingConfig { threshold };
            assert_eq!(
                route(body(strong), &fixture(), &config).project(),
                "Briarwood Golf"
            );
            assert_eq!(route(body(weak), &fixture(), &config).project(), INBOX);
        }
        // A maxed-out (but legal) threshold is a kill switch: evidence never
        // reaches 1.0, so everything lands in Inbox.
        let config = RoutingConfig { threshold: 1.0 };
        assert_eq!(route(body(strong), &fixture(), &config).project(), INBOX);
    }

    // -- discovery ---------------------------------------------------------

    #[test]
    fn discover_projects_finds_nested_projects_sorted() {
        let vault = tempdir().unwrap();
        for dir in ["Briarwood Golf", "Growth/Q3", "Data/raw"] {
            fs::create_dir_all(vault.path().join(dir)).unwrap();
        }

        let projects = discover_projects(vault.path()).unwrap();
        assert_eq!(
            projects,
            ["Data", "Data/raw", "Growth", "Growth/Q3", "Briarwood Golf"]
        );
    }

    #[test]
    fn discover_projects_excludes_reserved_infra_dirs_and_files() {
        let vault = tempdir().unwrap();
        for dir in [
            "Inbox",
            "raw",
            ".obsidian",
            "_assets",
            "Growth/_attachments",
        ] {
            fs::create_dir_all(vault.path().join(dir)).unwrap();
        }
        fs::write(vault.path().join("loose.md"), "not a project").unwrap();

        let projects = discover_projects(vault.path()).unwrap();
        assert_eq!(projects, ["Growth"]);
    }

    #[test]
    fn dir_name_exclusion_rules_are_pure() {
        // Reserved only at the vault root, in any casing — on the
        // case-insensitive filesystem this ships on, `Raw` *is* the sessions
        // folder (matching `validate_project`'s reservations)…
        assert!(is_excluded_dir_name("Inbox", true));
        assert!(is_excluded_dir_name("inbox", true));
        assert!(is_excluded_dir_name("INBOX", true));
        assert!(is_excluded_dir_name("raw", true));
        assert!(is_excluded_dir_name("Raw", true));
        assert!(is_excluded_dir_name("RAW", true));
        // …but legitimate deeper down.
        assert!(!is_excluded_dir_name("Inbox", false));
        assert!(!is_excluded_dir_name("raw", false));
        // Infra prefixes are excluded at every depth.
        for top_level in [true, false] {
            assert!(is_excluded_dir_name(".obsidian", top_level));
            assert!(is_excluded_dir_name("_assets", top_level));
            assert!(!is_excluded_dir_name("Growth", top_level));
        }
    }

    #[test]
    fn discover_projects_errors_when_vault_root_is_missing() {
        let vault = tempdir().unwrap();
        let missing = vault.path().join("does-not-exist");
        assert!(matches!(
            discover_projects(&missing),
            Err(RoutingError::Io { .. })
        ));
    }

    #[test]
    fn load_project_signals_pairs_projects_with_glossaries() {
        let vault = tempdir().unwrap();
        let golf_dir = vault.path().join("Briarwood Golf");
        fs::create_dir_all(&golf_dir).unwrap();
        fs::create_dir_all(vault.path().join("Growth")).unwrap();
        glossary(&[("MERIDIAN", &[])]).save(&golf_dir).unwrap();

        let signals = load_project_signals(vault.path()).unwrap();
        assert_eq!(signals.len(), 2);
        assert_eq!(signals[0].project, "Growth");
        assert!(signals[0].glossary.is_empty());
        assert_eq!(signals[1].project, "Briarwood Golf");
        assert!(signals[1].glossary.get("meridian").is_some());

        // A malformed glossary is a surfaced error, not a silent skip.
        fs::write(
            vault.path().join("Growth").join("_glossary.yml"),
            "terms: [\n",
        )
        .unwrap();
        assert!(matches!(
            load_project_signals(vault.path()),
            Err(RoutingError::Glossary(_))
        ));
    }

    // -- integration: score → split → write_note → re-parse ----------------

    /// A vault with the fixture projects' glossaries on disk, plus the
    /// reserved folders discovery must ignore.
    fn fixture_vault() -> tempfile::TempDir {
        let vault = tempdir().unwrap();
        for dir in ["Inbox", "raw", "Growth/Q3"] {
            fs::create_dir_all(vault.path().join(dir)).unwrap();
        }
        for candidate in fixture() {
            let dir = project_dir(vault.path(), &candidate.project);
            fs::create_dir_all(&dir).unwrap();
            candidate.glossary.save(&dir).unwrap();
        }
        vault
    }

    fn write_routed(
        vault: &Path,
        text: NoteText<'_>,
        title: Option<&str>,
    ) -> (Routing, std::path::PathBuf) {
        let candidates = load_project_signals(vault).unwrap();
        let routing = route(text, &candidates, &RoutingConfig::default());
        let note = Note::new(
            NoteId::generate().unwrap(),
            NoteType::Meeting,
            routing.clone(),
            "2026-07-17",
            vec![],
            Source::parse("transcript").unwrap(),
            text.body,
        )
        .unwrap();
        let path = crate::note::write_note(vault, &note, title).unwrap();
        (routing, path)
    }

    #[test]
    fn high_confidence_note_lands_in_its_project_folder() {
        let vault = fixture_vault();
        let text =
            body("MERIDIAN rollout: TeeTrack sync for the tee sheet, GreenFlow irrigation checks.");

        let (routing, path) = write_routed(vault.path(), text, Some("MERIDIAN rollout"));

        assert_eq!(
            path,
            vault.path().join("Briarwood Golf").join("meridian-rollout.md")
        );
        let reparsed = Note::from_markdown(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(reparsed.routing, routing);
        assert_eq!(
            reparsed.routing,
            Routing::Routed {
                project: "Briarwood Golf".to_string(),
                confidence: 5.0 / 7.0,
            }
        );
    }

    #[test]
    fn low_confidence_note_lands_in_inbox_with_recorded_score() {
        let vault = fixture_vault();
        let text = body("MERIDIAN kickoff with TeeTrack on the tee sheet; timeline follows OKR.");

        let (_, path) = write_routed(vault.path(), text, Some("Kickoff notes"));

        assert_eq!(path, vault.path().join("Inbox").join("kickoff-notes.md"));
        // The recorded score is *why* it landed in Inbox (FRONTMATTER_SCHEMA),
        // proven through the real writer and parser.
        let reparsed = Note::from_markdown(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            reparsed.routing,
            Routing::Routed {
                project: INBOX.to_string(),
                confidence: 0.5,
            }
        );
    }

    #[test]
    fn hierarchical_target_lands_in_nested_folder() {
        let vault = fixture_vault();
        let text = titled("Growth Q3 planning", "Agenda to follow.");

        let (routing, path) = write_routed(vault.path(), text, Some("Growth Q3 planning"));

        // Both of Growth/Q3's segments hit the title (8) against Growth's one
        // (4): saturate(8 - 4) = 2/3 — the title disambiguates parent vs child.
        assert_eq!(routing.confidence(), Some(2.0 / 3.0));
        assert_eq!(
            path,
            vault
                .path()
                .join("Growth")
                .join("Q3")
                .join("growth-q3-planning.md")
        );
    }
}
