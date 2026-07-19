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
//! earns weight from distinct glossary term/alias matches, mentions of its own
//! name, and lexical similarity to its recorded routing corrections (a manual
//! re-route becomes an example the next similar note learns from); a margin rule
//! subtracts the runner-up's weight so evidence split across projects reads as
//! *low* confidence, and a saturating curve maps the net weight into `[0, 1)`.
//! There is no model call anywhere in this module; the end-of-meeting distill
//! pass can later blend a proposal in as one more additive signal without
//! changing [`route`]'s contract.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

use crate::glossary::{Glossary, GlossaryError};
use crate::note::{project_dir, validate_project, Routing, INBOX, RESERVED_ROOT_DIRS};
use crate::routing_examples::{RoutingExample, RoutingExamples, RoutingExamplesError};

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

/// Weight of the routing-examples similarity signal at full similarity (the
/// per-project contribution is `EXAMPLE_WEIGHT * best_similarity`). Deliberately
/// equal to [`NAME_WEIGHT`]: one recorded human correction is as strong a cue as
/// one literal name mention, and like a lone name mention it saturates to `0.5`
/// alone — below the default threshold — so a correction never auto-files a note
/// single-handedly, and a *single* example can never override strong opposing
/// glossary evidence (its contribution is bounded by this constant regardless of
/// how many examples a project has, since aggregation takes the max, not a sum).
const EXAMPLE_WEIGHT: f64 = 2.0;
/// Minimum length (in `char`s) of a token that may count toward example
/// similarity — a cheap first pass that drops the shortest function words
/// ("the", "and", "for") before the [`EXAMPLE_STOPWORDS`] lookup. It is *not* a
/// stopword list on its own: every four-letter function word ("with", "that",
/// "from", "will") clears it, which is exactly why the explicit list exists. A
/// cost: short acronyms ("OKR", "Q3") don't ride the example channel — but those
/// are glossary vocabulary, which scores on its own separate channel.
const EXAMPLE_MIN_TOKEN_CHARS: usize = 4;
/// Absolute floor on shared content tokens before an example counts at all.
/// Guards short notes: any two English notes share a couple of long words by
/// chance, so a single coincidental overlap must not register — three or more
/// distinct content words in common is a topic, not noise.
const EXAMPLE_MIN_SHARED_TOKENS: usize = 3;
/// Relative floor on the overlap coefficient before an example counts: the note
/// must reuse this fraction of the *smaller* vocabulary, so a note that touches
/// only a corner of a recorded correction contributes nothing.
///
/// Note what this floor does **not** do: because the coefficient normalizes by
/// `min(|note|, |example|)`, a long note is judged by how much of the (capped,
/// short) excerpt it covers, and that fraction *rises* with note length rather
/// than falling. That asymmetry is deliberate — [`crate::routing_examples::EXCERPT_MAX_CHARS`]
/// caps an example at a few dozen tokens, so normalizing by the larger side
/// would put the signal permanently out of reach of any full-length meeting
/// note. The defense against a long note matching by chance is therefore token
/// *quality* ([`EXAMPLE_STOPWORDS`] and the prose-only excerpt), not this ratio.
const EXAMPLE_MIN_SIMILARITY: f64 = 0.3;

/// Function words and meeting-transcript filler excluded from example
/// similarity, sorted for [`slice::binary_search`]. Everything here survives the
/// [`EXAMPLE_MIN_TOKEN_CHARS`] length filter (shorter stopwords need no entry),
/// and everything here is a word two unrelated business notes are *expected* to
/// share — counting them would let a project earn similarity from grammar alone.
///
/// The tail of the list is structural rather than grammatical: `render_body`'s
/// section headings ("summary", "decisions", "action items", "open questions")
/// and generic meeting vocabulary appear in essentially every distilled note, so
/// they carry no topical information even though they are content words in the
/// ordinary sense.
///
/// `rustfmt::skip` keeps this as a packed word list: one entry per line is ~230
/// lines of noise, and the packed form is what makes the sorting scannable by
/// eye (`example_stopwords_is_sorted_and_deduped` enforces it either way).
#[rustfmt::skip]
const EXAMPLE_STOPWORDS: &[&str] = &[
    "about", "action", "after", "again", "against", "agenda", "almost", "along", "already", "also",
    "although", "always", "among", "another", "anyone", "anything", "around", "attendees", "back",
    "because", "been", "before", "being", "below", "best", "better", "between", "both", "bring",
    "call", "came", "cannot", "come", "comes", "coming", "could", "currently", "decision",
    "decisions", "discuss", "discussed", "discussion", "does", "doing", "done", "down", "during",
    "each", "either", "else", "enough", "even", "ever", "every", "everyone", "everything",
    "exactly", "from", "full", "gets", "give", "given", "gives", "goes", "going", "gone", "good",
    "great", "have", "having", "here", "however", "into", "issue", "issues", "item", "items",
    "just", "keep", "kind", "know", "last", "later", "least", "less", "like", "likely", "little",
    "long", "look", "looking", "made", "make", "makes", "making", "many", "maybe", "mean", "means",
    "meeting", "might", "more", "most", "much", "must", "need", "needs", "next", "note", "notes",
    "nothing", "only", "onto", "open", "other", "others", "over", "overall", "part", "people",
    "perhaps", "place", "point", "points", "probably", "question", "questions", "quite", "rather",
    "really", "right", "said", "same", "says", "seem", "seems", "seen", "several", "should",
    "side", "since", "some", "something", "sometimes", "soon", "sort", "start", "started", "steps",
    "still", "such", "summary", "sure", "take", "taken", "takes", "talk", "talked", "than", "that",
    "their", "them", "themselves", "then", "there", "therefore", "these", "they", "thing",
    "things", "think", "this", "those", "though", "thought", "three", "through", "thus", "time",
    "times", "today", "together", "told", "took", "topic", "topics", "toward", "towards",
    "transcript", "tried", "trying", "turn", "under", "until", "upon", "used", "uses", "using",
    "very", "want", "wanted", "wants", "ways", "week", "well", "went", "were", "what", "whatever",
    "when", "where", "whether", "which", "while", "whole", "whom", "whose", "will", "with",
    "within", "without", "word", "words", "work", "working", "works", "would", "yeah", "your",
    "yours",
];

/// Whether `token` is excluded from example similarity: too short to be
/// meaningful, or a known function word / structural heading.
fn is_example_stopword(token: &str) -> bool {
    token.chars().count() < EXAMPLE_MIN_TOKEN_CHARS
        || EXAMPLE_STOPWORDS.binary_search(&token).is_ok()
}

/// Saturation constant `K` in `evidence = w / (w + K)` — diminishing returns.
const SATURATION_WEIGHT: f64 = 2.0;

/// Compiled-in default threshold: `saturate(3.0)` by construction, i.e.
/// auto-filing requires net evidence worth three unopposed body-level signals
/// (`3/(3+K)`, `0.6` at the current `K = 2`). Written in terms of
/// [`SATURATION_WEIGHT`] so retuning the curve keeps that meaning instead of
/// silently changing what the threshold demands. The
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
/// weight, not a redesign — the [`examples`](Self::examples) field is that shape
/// realized once for the corrections signal.
#[derive(Debug, Clone)]
pub struct ProjectSignals {
    /// Frontmatter project slug, e.g. `"Briarwood Golf"` or `"Growth/Q3"`.
    pub project: String,
    /// The project's glossary (empty when it has no `_glossary.yml`).
    pub glossary: Glossary,
    /// The project's recorded routing corrections (empty when it has no
    /// `_routing_examples.yml`). Scored as lexical similarity, see
    /// [`EXAMPLE_WEIGHT`].
    pub examples: RoutingExamples,
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
    /// Best gated similarity to any of this project's routing examples that was
    /// actually credited (`0.0` when no example spoke). The weight it
    /// contributed is `EXAMPLE_WEIGHT * example_sim`.
    pub example_sim: f64,
    /// Raw summed signal weight (the margin rule operates on this).
    pub weight: f64,
    /// `saturate(weight)` in `[0, 1)` — this candidate's standalone evidence.
    /// Not the final confidence: [`route`] subtracts the runner-up first.
    pub evidence: f64,
}

/// A fatal error discovering the vault's projects — the candidate set could not
/// even be enumerated. A single project's *glossary* failing to load is not one
/// of these: that is contained and reported as a [`GlossaryLoadFailure`], never
/// aborting the whole load (see [`load_project_signals`]).
#[derive(Debug, thiserror::Error)]
pub enum RoutingError {
    #[error("project discovery I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// A project whose `_glossary.yml` could not be loaded. Routing proceeds with
/// this project treated as having no vocabulary — it still matches its own name
/// and still receives notes — so a broken glossary is *contained* to its own
/// project instead of disabling routing vault-wide. The error rides out for the
/// caller to surface: a malformed or duplicate-term glossary is a user-fixable
/// mistake worth reporting, just not worth losing the rest of the vault's
/// routing over.
#[derive(Debug)]
pub struct GlossaryLoadFailure {
    /// The project slug whose glossary failed to load.
    pub project: String,
    /// Why it failed (malformed YAML, duplicate term).
    pub error: GlossaryError,
}

/// A project whose `_routing_examples.yml` could not be loaded — the exact
/// glossary-failure treatment applied to the corrections signal. Routing
/// proceeds with this project contributing no example evidence (it still matches
/// its name and glossary and still receives notes), so a broken corrections log
/// is *contained* to its own project rather than disabling routing vault-wide.
/// A *missing* file is not a failure (a project without corrections is the
/// normal case); only an unreadable or malformed one lands here for the caller
/// to surface as a user-fixable mistake.
#[derive(Debug)]
pub struct ExamplesLoadFailure {
    /// The project slug whose routing-examples log failed to load.
    pub project: String,
    /// Why it failed (malformed YAML, I/O error).
    pub error: RoutingExamplesError,
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

/// The distinct "content tokens" of some already-tokenized text: every token
/// that survives [`is_example_stopword`], deduplicated. Borrows from the input
/// slices so example scoring allocates only the (small) example-side sets. This
/// is the vocabulary the corrections signal compares on, and dropping function
/// words here is what keeps two unrelated notes from sharing "similarity" they
/// owe entirely to English grammar.
fn content_token_set<'a>(token_slices: &[&'a [String]]) -> HashSet<&'a str> {
    token_slices
        .iter()
        .flat_map(|slice| slice.iter())
        .filter(|token| !is_example_stopword(token))
        .map(String::as_str)
        .collect()
}

/// Lexical similarity between the note's content tokens and one routing
/// example, in `[0, 1]`. The example's `title` and `excerpt` are tokenized with
/// the same [`tokens`] fold used everywhere else, filtered to content tokens,
/// and compared by *overlap coefficient* — `shared / min(|note|, |example|)` —
/// so a short quick-capture that reuses the example's vocabulary scores near
/// `1.0` even against a longer note, and a long note is judged by how much of
/// the (capped, short) excerpt it reuses. Two gates return exactly `0.0`
/// otherwise: fewer than [`EXAMPLE_MIN_SHARED_TOKENS`] shared tokens, or a
/// coefficient below [`EXAMPLE_MIN_SIMILARITY`]. Deterministic: integer counts
/// and one division, no model call.
///
/// The `min` denominator makes the coefficient *easier* to clear as the note
/// grows, so neither gate is what stops a long unrelated note from matching —
/// [`content_token_set`] is, by refusing to count the function words and
/// section headings that any two notes share (see [`EXAMPLE_STOPWORDS`]). What
/// reaches the arithmetic below is topical vocabulary only.
fn example_similarity(note_content: &HashSet<&str>, example: &RoutingExample) -> f64 {
    if note_content.is_empty() {
        return 0.0;
    }
    let title_tokens = tokens(&example.title);
    let excerpt_tokens = tokens(&example.excerpt);
    let example_content = content_token_set(&[&title_tokens, &excerpt_tokens]);
    if example_content.is_empty() {
        return 0.0;
    }
    let shared = note_content.intersection(&example_content).count();
    if shared < EXAMPLE_MIN_SHARED_TOKENS {
        return 0.0;
    }
    let denom = note_content.len().min(example_content.len());
    let similarity = shared as f64 / denom as f64;
    if similarity >= EXAMPLE_MIN_SIMILARITY {
        similarity
    } else {
        0.0
    }
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
    // The note's content vocabulary, built once: the corrections signal compares
    // every candidate's examples against this same set.
    let note_content = content_token_set(&[&title, &body]);

    let mut scores: Vec<ProjectScore> = candidates
        .iter()
        .map(|candidate| score_candidate(&title, &body, &note_content, candidate))
        .collect();
    scores.sort_by(|a, b| {
        b.weight
            .total_cmp(&a.weight)
            .then_with(|| a.project.cmp(&b.project))
    });
    scores
}

fn score_candidate(
    title: &[String],
    body: &[String],
    note_content: &HashSet<&str>,
    candidate: &ProjectSignals,
) -> ProjectScore {
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

    // The corrections signal: the single best-matching recorded example, scored
    // as lexical similarity. Max, not sum — near-duplicate corrections of the
    // same meeting series must not stack and outvote a glossary, so a project's
    // example contribution is bounded by `EXAMPLE_WEIGHT` no matter how many
    // examples it has. An example's tokens are compared against `note_content`
    // without deduping the note's name/glossary spellings first: resembling a
    // corrected note legitimately includes sharing its project's name, and the
    // double count is bounded by that same cap. A project with no examples folds
    // to exactly `0.0` and so adds nothing.
    let example_sim = candidate
        .examples
        .examples()
        .iter()
        .map(|example| example_similarity(note_content, example))
        .fold(0.0_f64, f64::max);
    weight += EXAMPLE_WEIGHT * example_sim;

    ProjectScore {
        project: candidate.project.clone(),
        glossary_hits,
        name_hits,
        example_sim,
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

/// What [`load_project_signals`] yields: the scored candidates, plus one
/// independent per-project failure report per signal that can fail to load.
/// Every failure list is empty on the happy path.
///
/// A struct rather than a tuple on purpose. [`ProjectSignals`] is shaped so the
/// next signal (the distill pass's model proposal) is one more field; this is
/// the loader's half of that promise, so adding a fourth report names itself at
/// every call site instead of silently shifting a positional binding.
#[derive(Debug, Default)]
pub struct LoadedProjectSignals {
    /// The candidates to score, in [`discover_projects`] order.
    pub signals: Vec<ProjectSignals>,
    /// Projects whose `_glossary.yml` could not be loaded.
    pub glossary_failures: Vec<GlossaryLoadFailure>,
    /// Projects whose `_routing_examples.yml` could not be loaded.
    pub example_failures: Vec<ExamplesLoadFailure>,
}

/// [`discover_projects`] plus each project's glossary and recorded routing
/// examples — the one-call signal loader for [`route`], and the *only* place the
/// scorer touches the disk (scoring itself is pure). A project without a
/// `_glossary.yml` contributes an empty glossary; one without a
/// `_routing_examples.yml` contributes no examples.
///
/// Discovery I/O failure is fatal (the candidate set can't be enumerated), but a
/// single project's malformed glossary *or* corrections log is *contained*: that
/// signal loads empty — the project still matches its own name and still
/// receives notes — and the error is returned for the caller to surface.
/// Glossary failures and examples failures are independent: a project can appear
/// in both lists. One project's broken file never disables routing for the rest
/// of the vault; the mistake is reported, not allowed to take every other
/// project's routing down with it. (The margin rule already guards against the
/// miscategorization a missing signal might otherwise cause: routing into a
/// project still demands net evidence over the runner-up.)
pub fn load_project_signals(vault_root: &Path) -> Result<LoadedProjectSignals, RoutingError> {
    let projects = discover_projects(vault_root)?;
    let mut signals = Vec::with_capacity(projects.len());
    let mut glossary_failures = Vec::new();
    let mut example_failures = Vec::new();
    for project in projects {
        // `note::project_dir` is the writer's slug→folder mapping; using it here
        // keeps both signals loading from exactly the folder notes land in.
        let dir = project_dir(vault_root, &project);

        // Contain a glossary failure: the project still routes on its name.
        let glossary = match Glossary::load(&dir) {
            Ok(glossary) => glossary,
            Err(error) => {
                glossary_failures.push(GlossaryLoadFailure {
                    project: project.clone(),
                    error,
                });
                Glossary::default()
            }
        };

        // Contain an examples failure independently: the project still routes on
        // its name and glossary. A missing file is the normal case, not a
        // failure — `RoutingExamples::load` already maps it to an empty set.
        let examples = match RoutingExamples::load(&dir) {
            Ok(examples) => examples,
            Err(error) => {
                example_failures.push(ExamplesLoadFailure {
                    project: project.clone(),
                    error,
                });
                RoutingExamples::default()
            }
        };

        signals.push(ProjectSignals {
            project,
            glossary,
            examples,
        });
    }
    Ok(LoadedProjectSignals {
        signals,
        glossary_failures,
        example_failures,
    })
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
            examples: RoutingExamples::default(),
        }
    }

    /// A routing example carrying only the fields scoring reads (title +
    /// excerpt); the rest are fixed dummies. Title/excerpt are the lexical
    /// context the corrections signal compares against.
    fn example(title: &str, excerpt: &str) -> RoutingExample {
        RoutingExample {
            note_id: "n_example".to_string(),
            title: title.to_string(),
            excerpt: excerpt.to_string(),
            previous_project: None,
            confidence: 1.0,
            corrected_at: "2026-07-18T20:15:00Z".to_string(),
            reason: None,
        }
    }

    /// [`signals`] plus a set of recorded routing examples for the same project.
    fn signals_with_examples(
        project: &str,
        entries: &[(&str, &[&str])],
        examples: &[RoutingExample],
    ) -> ProjectSignals {
        let mut log = RoutingExamples::default();
        for example in examples {
            log.upsert(example.clone());
        }
        ProjectSignals {
            project: project.to_string(),
            glossary: glossary(entries),
            examples: log,
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

        let loaded = load_project_signals(vault.path()).unwrap();
        assert!(loaded.glossary_failures.is_empty());
        assert!(loaded.example_failures.is_empty());
        let signals = &loaded.signals;
        assert_eq!(signals.len(), 2);
        assert_eq!(signals[0].project, "Growth");
        assert!(signals[0].glossary.is_empty());
        assert_eq!(signals[1].project, "Briarwood Golf");
        assert!(signals[1].glossary.get("meridian").is_some());

        // A malformed glossary is contained, not fatal: the project loads with
        // an empty glossary and the error rides out in the failures list,
        // leaving every other project's signals intact.
        fs::write(
            vault.path().join("Growth").join("_glossary.yml"),
            "terms: [\n",
        )
        .unwrap();
        let loaded = load_project_signals(vault.path()).unwrap();
        let signals = &loaded.signals;
        assert_eq!(signals.len(), 2);
        assert_eq!(loaded.glossary_failures.len(), 1);
        assert_eq!(loaded.glossary_failures[0].project, "Growth");
        assert_eq!(signals[0].project, "Growth");
        assert!(signals[0].glossary.is_empty());
        assert!(signals[1].glossary.get("meridian").is_some());
    }

    // -- examples signal ---------------------------------------------------

    #[test]
    fn strong_example_match_lifts_a_borderline_note_over_the_threshold() {
        // A body that only name-drops the project: 2.0, saturate(2) = 0.5, below
        // threshold — it lands in Inbox with nothing else to go on.
        let borderline = body("Briarwood Golf clubhouse migration cutover plan");
        let without = vec![signals("Briarwood Golf", &[])];
        assert_eq!(
            route(borderline, &without, &RoutingConfig::default()),
            Routing::Routed {
                project: INBOX.to_string(),
                confidence: 0.5,
            }
        );

        // Record a correction about exactly this topic and the same note crosses:
        // name (2.0) + a perfect example match (EXAMPLE_WEIGHT * 1.0 = 2.0) = 4.0,
        // unopposed, so saturate(4 - 0) = 4/6, above the 0.6 threshold.
        let with = vec![signals_with_examples(
            "Briarwood Golf",
            &[],
            &[example(
                "Clubhouse pos migration",
                "clubhouse migration cutover plan",
            )],
        )];
        assert_eq!(
            route(borderline, &with, &RoutingConfig::default()),
            Routing::Routed {
                project: "Briarwood Golf".to_string(),
                confidence: 4.0 / 6.0,
            }
        );
        let scores = score_projects(borderline, &with);
        assert_eq!(score_of(&scores, "Briarwood Golf").example_sim, 1.0);
    }

    #[test]
    fn an_example_match_alone_never_auto_files() {
        // A perfect example match with no name or glossary evidence: 2.0,
        // saturate(2) = 0.5 < threshold. One correction can't file a note by
        // itself, exactly like one bare name mention.
        let candidates = vec![signals_with_examples(
            "Briarwood Golf",
            &[],
            &[example(
                "clubhouse migration",
                "clubhouse migration cutover plan",
            )],
        )];
        let text = body("clubhouse migration cutover plan");
        assert_eq!(
            route(text, &candidates, &RoutingConfig::default()),
            Routing::Routed {
                project: INBOX.to_string(),
                confidence: 0.5,
            }
        );
    }

    #[test]
    fn a_single_example_cannot_override_strong_glossary_evidence() {
        // P earns four glossary hits (4.0); Q earns a perfect example (2.0). The
        // example is real evidence but the margin (4 - 2 = 2, saturate = 0.5)
        // reads the conflict as low confidence: the note lands in Inbox with P on
        // top, never flipped to Q by one correction.
        let text = body("MERIDIAN TeeTrack GreenFlow irrigation clubhouse migration cutover plan");
        let four_terms = vec![
            signals(
                "Briarwood Golf",
                &[
                    ("MERIDIAN", &[]),
                    ("TeeTrack", &[]),
                    ("GreenFlow", &[]),
                    ("irrigation", &[]),
                ],
            ),
            signals_with_examples(
                "Growth/Q3",
                &[],
                &[example("clubhouse", "clubhouse migration cutover plan")],
            ),
        ];
        let scores = score_projects(text, &four_terms);
        assert_eq!(scores[0].project, "Briarwood Golf");
        assert_eq!(
            route(text, &four_terms, &RoutingConfig::default()),
            Routing::Routed {
                project: INBOX.to_string(),
                confidence: 0.5,
            }
        );

        // A fifth glossary hit (5.0 vs 2.0, margin 3) is enough: strong evidence
        // still wins despite a perfect opposing example.
        let five_terms = vec![
            signals(
                "Briarwood Golf",
                &[
                    ("MERIDIAN", &[]),
                    ("TeeTrack", &[]),
                    ("GreenFlow", &[]),
                    ("irrigation", &[]),
                    ("tee sheet", &[]),
                ],
            ),
            signals_with_examples(
                "Growth/Q3",
                &[],
                &[example("clubhouse", "clubhouse migration cutover plan")],
            ),
        ];
        let text =
            body("MERIDIAN TeeTrack GreenFlow irrigation tee sheet clubhouse migration cutover plan");
        assert_eq!(
            route(text, &five_terms, &RoutingConfig::default()),
            Routing::Routed {
                project: "Briarwood Golf".to_string(),
                confidence: DEFAULT_THRESHOLD,
            }
        );
    }

    #[test]
    fn example_evidence_split_across_projects_reads_as_low_confidence() {
        // Two projects each hold a perfect example for the same topic and nothing
        // else distinguishes them: 2.0 vs 2.0, margin 0, saturate(0) = 0.0. Split
        // precedent is a dead tie, so the note lands in Inbox at 0.0.
        let excerpt = "clubhouse migration cutover plan";
        let candidates = vec![
            signals_with_examples("Ops", &[], &[example("clubhouse", excerpt)]),
            signals_with_examples("Sales", &[], &[example("clubhouse", excerpt)]),
        ];
        let text = body("clubhouse migration cutover plan");
        assert_eq!(
            route(text, &candidates, &RoutingConfig::default()),
            Routing::Routed {
                project: INBOX.to_string(),
                confidence: 0.0,
            }
        );
    }

    #[test]
    fn examples_do_not_stack_beyond_the_best_match() {
        // Three near-duplicate corrections of the same meeting series must not
        // sum: the contribution is EXAMPLE_WEIGHT * best_similarity, capped at
        // EXAMPLE_WEIGHT, not 3x it.
        let excerpt = "clubhouse migration cutover plan";
        let candidates = vec![signals_with_examples(
            "Ops",
            &[],
            &[
                example("clubhouse migration one", excerpt),
                example("clubhouse migration two", excerpt),
                example("clubhouse migration three", excerpt),
            ],
        )];
        let scores = score_projects(body("clubhouse migration cutover plan"), &candidates);
        let ops = score_of(&scores, "Ops");
        assert_eq!(ops.example_sim, 1.0);
        assert_eq!(ops.weight, EXAMPLE_WEIGHT);
    }

    #[test]
    fn unrelated_text_earns_zero_example_similarity() {
        // (i) Below the shared-token floor: two content words in common is
        // coincidence, not a topic — under EXAMPLE_MIN_SHARED_TOKENS, so 0.0.
        let too_few = vec![signals_with_examples(
            "Ops",
            &[],
            &[example("", "budget review quarterly finance numbers")],
        )];
        let scores = score_projects(body("budget review dentist"), &too_few);
        assert_eq!(score_of(&scores, "Ops").example_sim, 0.0);
        assert_eq!(score_of(&scores, "Ops").weight, 0.0);

        // (ii) Above the shared-token floor but below the similarity floor: a
        // long note sharing three common words with a long excerpt (3/11 ≈ 0.27
        // < EXAMPLE_MIN_SIMILARITY) is not about the same thing, so 0.0.
        let too_thin = vec![signals_with_examples(
            "Ops",
            &[],
            &[example(
                "",
                "budget review finance alpha bravo charlie delta echo foxtrot gamma hotel",
            )],
        )];
        let scores = score_projects(
            body("budget review finance india juliet kilo lima mike november oscar papa"),
            &too_thin,
        );
        assert_eq!(score_of(&scores, "Ops").example_sim, 0.0);
        assert_eq!(score_of(&scores, "Ops").weight, 0.0);

        // (iii) Overlap only on sub-EXAMPLE_MIN_TOKEN_CHARS tokens: the texts
        // share "okr", "abc", "the", but none survive the length filter, so the
        // content overlap is empty and the example contributes 0.0.
        let short_only = vec![signals_with_examples(
            "Ops",
            &[],
            &[example("", "okr abc the budget review finance")],
        )];
        let scores = score_projects(body("okr abc the meeting agenda notes"), &short_only);
        assert_eq!(score_of(&scores, "Ops").example_sim, 0.0);
        assert_eq!(score_of(&scores, "Ops").weight, 0.0);
    }

    #[test]
    fn shared_function_words_alone_earn_no_example_similarity() {
        // The regression this list exists for. Both texts are ordinary English,
        // so they share "with", "that", "before", "these", "would" — every one
        // of them four-plus chars, so a length filter alone would count all five
        // as topical overlap. Against a short excerpt the coefficient normalizes
        // by the *example* side, so five function words out of ten tokens reads
        // as similarity 0.5 and hands Ops a full EXAMPLE_WEIGHT/2 of evidence for
        // a note about golf-course maintenance. Only the stopword filter stops
        // it, so the credited similarity must be exactly 0.0.
        let candidates = vec![signals_with_examples(
            "Ops",
            &[],
            &[example(
                "",
                "vendor contract signed with legal before that renewal would take these steps",
            )],
        )];
        let unrelated = body(
            "clubhouse irrigation schedule rebuilt greens fairway drainage survey \
             with that before these would replaced entirely",
        );
        let scores = score_projects(unrelated, &candidates);
        assert_eq!(score_of(&scores, "Ops").example_sim, 0.0);
        assert_eq!(score_of(&scores, "Ops").weight, 0.0);

        // The flip side, pinning that the `min` denominator is deliberate and
        // still reachable: the same long note against an example that shares its
        // *topical* vocabulary matches at full strength. A capped excerpt can
        // never be more than a few dozen tokens, so normalizing by the larger
        // side instead would put this case permanently out of reach.
        let topical = vec![signals_with_examples(
            "Ops",
            &[],
            &[example("", "clubhouse irrigation drainage survey")],
        )];
        let scores = score_projects(unrelated, &topical);
        assert_eq!(score_of(&scores, "Ops").example_sim, 1.0);
    }

    #[test]
    fn example_stopwords_is_sorted_and_deduped() {
        // `is_example_stopword` binary-searches the list, which silently misses
        // entries if it is ever left unsorted — and a duplicate is a sign of a
        // hand-merge gone wrong.
        let mut sorted = EXAMPLE_STOPWORDS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.as_slice(), EXAMPLE_STOPWORDS);

        // Every entry must survive the length pre-filter, or it is dead weight:
        // shorter tokens are already dropped before the lookup runs.
        for word in EXAMPLE_STOPWORDS {
            assert!(
                word.chars().count() >= EXAMPLE_MIN_TOKEN_CHARS,
                "{word} is already dropped by the length filter"
            );
        }

        // The words the old doc comment wrongly claimed the length filter
        // dropped, spot-checked through the real predicate.
        for word in ["with", "that", "this", "from", "will", "have", "been"] {
            assert!(is_example_stopword(word), "{word} should be a stopword");
        }
        for word in ["clubhouse", "irrigation", "vendor"] {
            assert!(!is_example_stopword(word), "{word} is topical vocabulary");
        }
    }

    #[test]
    fn load_project_signals_loads_examples_and_contains_failures() {
        let vault = tempdir().unwrap();
        let alpha_dir = vault.path().join("Alpha");
        let beta_dir = vault.path().join("Beta");
        fs::create_dir_all(&alpha_dir).unwrap();
        fs::create_dir_all(&beta_dir).unwrap();

        // Alpha has a valid corrections log; Beta's is malformed.
        let mut alpha_log = RoutingExamples::default();
        alpha_log.upsert(example(
            "clubhouse migration",
            "clubhouse migration cutover plan",
        ));
        alpha_log.save(&alpha_dir).unwrap();
        fs::write(
            crate::routing_examples::routing_examples_path(&beta_dir),
            "examples: [\n",
        )
        .unwrap();

        let loaded = load_project_signals(vault.path()).unwrap();

        let signals = &loaded.signals;
        assert_eq!(signals.len(), 2);
        assert!(loaded.glossary_failures.is_empty());
        // Alpha (sorted first) carries its example; Beta is contained to empty.
        assert_eq!(signals[0].project, "Alpha");
        assert_eq!(signals[0].examples.examples().len(), 1);
        assert_eq!(signals[1].project, "Beta");
        assert!(signals[1].examples.is_empty());
        assert_eq!(loaded.example_failures.len(), 1);
        assert_eq!(loaded.example_failures[0].project, "Beta");
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
        let candidates = load_project_signals(vault).unwrap().signals;
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

    #[test]
    fn recorded_correction_makes_a_similar_note_route_to_its_project() {
        // The acceptance case, through the real recording path: a manual
        // correction teaches routing, and the next similar note files itself.
        let vault = fixture_vault();

        // (i) A note that only name-drops Briarwood Golf and talks about a topic
        // absent from every glossary. Name alone (2.0) is saturate(2) = 0.5,
        // below threshold — it lands in Inbox today.
        let seed_text =
            body("Briarwood Golf clubhouse migration cutover plan and vendor timeline discussion");
        let (seed_routing, seed_path) = write_routed(
            vault.path(),
            seed_text,
            Some("Briarwood Golf clubhouse migration"),
        );
        assert_eq!(
            seed_routing,
            Routing::Routed {
                project: INBOX.to_string(),
                confidence: 0.5,
            }
        );

        // (ii) A human re-files it into Briarwood Golf, which records the
        // correction as a routing example in that project's folder.
        let seed = Note::from_markdown(&fs::read_to_string(&seed_path).unwrap()).unwrap();
        let routed = crate::vault::file_note_to_project(
            vault.path(),
            &seed.id,
            "Briarwood Golf",
            &crate::vault::FileNoteOptions::default(),
        )
        .unwrap()
        .expect("the seed note exists");
        assert!(routed.moved);

        // (iii) A NEW note about the same topic now crosses on its own: name
        // (2.0) + a perfect similarity to the recorded example (2.0) = 4.0,
        // unopposed, saturate(4) = 4/6.
        let (followup_routing, followup_path) = write_routed(
            vault.path(),
            body("Briarwood Golf clubhouse migration cutover plan"),
            Some("Follow up sync"),
        );
        assert_eq!(
            followup_routing,
            Routing::Routed {
                project: "Briarwood Golf".to_string(),
                confidence: 4.0 / 6.0,
            }
        );
        assert_eq!(
            followup_path,
            vault.path().join("Briarwood Golf").join("follow-up-sync.md")
        );

        // (iv) An unrelated note shares nothing with the example and is
        // unaffected: no name, no glossary, no example evidence — Inbox at 0.0.
        let (unrelated_routing, unrelated_path) = write_routed(
            vault.path(),
            body("Dentist appointment tomorrow morning reminder"),
            Some("Dentist"),
        );
        assert_eq!(
            unrelated_routing,
            Routing::Routed {
                project: INBOX.to_string(),
                confidence: 0.0,
            }
        );
        assert_eq!(
            unrelated_path,
            vault.path().join("Inbox").join("dentist.md")
        );
    }
}
