//! End-of-meeting distill pass: turns a persisted raw session into a
//! distilled meeting note — summary, decisions, action items, and open
//! questions — written through [`crate::note::write_note`]
//! (FOUNDING_DOC §3.5, step 2 of the pipeline).
//!
//! The model returns structured JSON; the Markdown body is rendered here,
//! deterministically, so it always matches the locked `meeting` example in
//! `docs/FRONTMATTER_SCHEMA.md` and stays machine-extractable. Unlike the
//! glossary cleanup pass this one **fails hard**: a runner error or unusable
//! output writes no note at all (a missing note is retryable — the raw
//! session persists; a fabricated note is not).
//!
//! # Action-item line grammar
//!
//! Every action item renders as exactly one of these lines (this is the
//! contract Phase 3's `ActionItem` extractor parses against — owner,
//! description, and due date must stay mechanically recoverable):
//!
//! ```text
//! - [ ] {Owner} to {description} by {YYYY-MM-DD}.
//! - [ ] {Owner} to {description}.
//! ```
//!
//! `{Owner}` is the item's owner, or the fixed token [`UNASSIGNED_OWNER`]
//! when nobody could be attributed (the MCP `ActionItem.owner` field is a
//! required non-null string). The renderer guarantees: the owner never
//! contains `" to "` (so the owner is the prefix before the *first* `" to "`),
//! the optional due-date tail is `" by "` + a valid `YYYY-MM-DD` immediately
//! before the single terminal `.`, and the line contains no newlines. An
//! unchecked box means `open`, a checked box `done`; `overdue` is derived
//! downstream from `open` + a past due date.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};

use crate::llm::{extract_balanced_spans, HeadlessClaude, LlmRequest, LlmRunError};
use crate::naming;
use crate::note::{self, Note, NoteError, NoteId, NoteType, Routing, Source, Tag, INBOX};
use crate::raw_session::{self, RawSessionError, TranscriptSegment};
use crate::transcription::Channel;

/// The owner token rendered for an action item nobody could be attributed to.
/// Chosen over defaulting to the local user ("You"), which would silently
/// claim commitments the user never took.
pub const UNASSIGNED_OWNER: &str = "Unassigned";

/// Longest title kept from the model (characters); anything longer is cut.
/// The title only seeds the note's filename slug (itself capped at 40) and
/// the `DistilledNote` echo, so this is a sanity bound, not a schema rule.
const MAX_TITLE_LEN: usize = 120;

/// Most tags kept from the model. The schema puts no cap on `tags`, but a
/// distill that "tags" a meeting with a dozen topics has stopped tagging.
const MAX_TAGS: usize = 8;

/// Failure distilling a session. Every variant is fatal to this run — no
/// note is written — and the raw session on disk is untouched, so the caller
/// can always retry.
#[derive(Debug, thiserror::Error)]
pub enum DistillError {
    #[error(transparent)]
    Session(#[from] RawSessionError),
    #[error("transcript is empty; nothing to distill")]
    EmptyTranscript,
    #[error(transparent)]
    Run(#[from] LlmRunError),
    #[error("distill output was not usable: {0}")]
    Parse(String),
    #[error("session path {0} is not inside the vault root")]
    SessionOutsideVault(PathBuf),
    #[error("failed to generate note id: {0}")]
    Id(#[source] std::io::Error),
    #[error(transparent)]
    Note(#[from] NoteError),
}

const SYSTEM_PROMPT: &str = "You are a meeting-notes distiller. You will be given a meeting's \
date and its transcript, one line per utterance, each prefixed with its speaker channel: \"You\" \
is the local user, \"Them\" is the other participant(s), \"Unknown\" is unattributed audio. \
Respond with ONLY a single JSON object of exactly this shape: {\"title\": \"<short meeting \
title, at most 80 characters>\", \"summary\": \"<one to three short paragraphs of plain \
prose>\", \"decisions\": [\"<one complete sentence per decision actually made or agreed during \
the meeting>\"], \"action_items\": [{\"owner\": \"<the responsible person's name, or \\\"You\\\" \
when the local user took it on; null when unclear>\", \"description\": \"<what is to be done, \
as a verb phrase like \\\"send the signed budget memo to finance\\\" - no owner name and no due \
date inside it>\", \"due_date\": \"<YYYY-MM-DD when a date was stated or clearly implied \
relative to the meeting date; otherwise null>\"}], \"open_questions\": [\"<questions raised but \
left unresolved>\"], \"tags\": [\"<zero to five lowercase-kebab-case topic tags>\"]}. Only \
report decisions that were actually made and action items that are concrete commitments; empty \
arrays are fine. Never invent owners, dates, decisions, or facts not present in the transcript. \
No prose, no markdown fences, no explanation - only the JSON object.";

/// The distilled content of one meeting, parsed and normalized from the
/// model's JSON. [`render_body`] turns this into the note's Markdown body;
/// the routing engine (#43) scores it into a [`Routing`].
#[derive(Debug, Clone, PartialEq)]
pub struct DistillOutput {
    /// Short meeting title; seeds the note's filename slug. `None` when the
    /// model produced nothing usable.
    pub title: Option<String>,
    /// Summary prose; never empty ([`parse_output`] rejects an output
    /// without one).
    pub summary: String,
    pub decisions: Vec<String>,
    pub action_items: Vec<ActionItemDraft>,
    pub open_questions: Vec<String>,
    /// Already-validated tags; the model's invalid candidates are dropped,
    /// not surfaced as errors.
    pub tags: Vec<Tag>,
}

/// One extracted action item, pre-rendering. "Draft" because the durable
/// `ActionItem` shape (id, status, source note) is minted downstream — this
/// carries only what the meeting itself said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionItemDraft {
    /// What is to be done, as a verb phrase; never empty, never owner- or
    /// date-suffixed (the renderer adds those).
    pub description: String,
    /// Who owns it; `None` renders as [`UNASSIGNED_OWNER`]. Never contains
    /// `" to "` (grammar guard).
    pub owner: Option<String>,
    /// Validated `YYYY-MM-DD`, or `None`.
    pub due_date: Option<String>,
}

/// [`distill_session`]'s successful result: where the note landed, its
/// minted id, and the title used (if any) for the filename slug.
#[derive(Debug, Clone)]
pub struct DistilledNote {
    pub path: PathBuf,
    pub id: NoteId,
    pub title: Option<String>,
}

/// The wire shape the model is asked for. `summary` is deliberately required
/// (no default) so a decoy `{}` or unrelated JSON object embedded in prose
/// can never masquerade as a distill result; everything else degrades to
/// empty rather than failing the whole parse.
#[derive(serde::Deserialize)]
struct RawDistillOutput {
    #[serde(default)]
    title: Option<String>,
    summary: String,
    #[serde(default)]
    decisions: Vec<String>,
    #[serde(default)]
    action_items: Vec<RawActionItem>,
    #[serde(default)]
    open_questions: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
}

/// One action item as the model returns it. All fields default so a single
/// malformed item degrades to a droppable empty rather than failing the
/// whole output.
#[derive(serde::Deserialize)]
struct RawActionItem {
    #[serde(default)]
    description: String,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    due_date: Option<String>,
}

/// The `{You|Them|Unknown}: ` prefix each transcript line carries in the
/// prompt (mirrors the [`Channel`] semantics the system prompt explains).
fn channel_label(channel: Channel) -> &'static str {
    match channel {
        Channel::You => "You",
        Channel::Them => "Them",
        Channel::Unknown => "Unknown",
    }
}

/// Builds the headless request for `segments`. `meeting_date` (`YYYY-MM-DD`)
/// is included so relative phrasings ("by Friday") can resolve to real
/// dates. Timestamps are deliberately omitted — they cost tokens and carry
/// no extraction value at this granularity.
pub fn build_request(segments: &[TranscriptSegment], meeting_date: &str) -> LlmRequest {
    let mut transcript = String::new();
    for segment in segments {
        let text = collapse_ws(&segment.text);
        if text.is_empty() {
            continue;
        }
        let _ = writeln!(transcript, "{}: {}", channel_label(segment.channel), text);
    }
    LlmRequest {
        system_prompt: SYSTEM_PROMPT.to_owned(),
        prompt: format!("Meeting date: {meeting_date}\n\nTranscript:\n{transcript}"),
    }
}

/// Parses the model's output into a normalized [`DistillOutput`].
///
/// Accepts the bare JSON object it asked for, or — models sometimes wrap it
/// in prose or a markdown fence despite instructions — the first balanced
/// `{...}` span that deserializes to the expected shape. Anything else, or a
/// missing/empty `summary`, is a [`DistillError::Parse`]: unlike the cleanup
/// pass there is no untouched input to fall back to, so unusable output must
/// fail the run.
pub fn parse_output(model_output: &str) -> Result<DistillOutput, DistillError> {
    let trimmed = model_output.trim();
    let raw = serde_json::from_str::<RawDistillOutput>(trimmed)
        .ok()
        .or_else(|| {
            extract_balanced_spans(trimmed, '{', '}')
                .into_iter()
                .find_map(|span| serde_json::from_str::<RawDistillOutput>(span).ok())
        })
        .ok_or_else(|| {
            DistillError::Parse("no JSON object of the expected shape in the model output".into())
        })?;

    let summary = raw.summary.replace("\r\n", "\n").trim().to_string();
    if summary.is_empty() {
        return Err(DistillError::Parse("missing or empty summary".into()));
    }

    Ok(DistillOutput {
        title: normalize_title(raw.title),
        summary,
        decisions: raw
            .decisions
            .iter()
            .filter_map(|d| normalize_sentence(d))
            .collect(),
        action_items: raw
            .action_items
            .into_iter()
            .filter_map(normalize_action_item)
            .collect(),
        open_questions: raw
            .open_questions
            .iter()
            .filter_map(|q| normalize_sentence(q))
            .collect(),
        tags: normalize_tags(raw.tags),
    })
}

/// Collapses every whitespace run (including newlines) to a single space and
/// trims — the one-line normal form every bullet and grammar field needs.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_title(raw: Option<String>) -> Option<String> {
    let title = collapse_ws(raw.as_deref()?);
    if title.is_empty() {
        return None;
    }
    let capped: String = title.chars().take(MAX_TITLE_LEN).collect();
    Some(capped.trim_end().to_string())
}

/// One-line bullet with terminal punctuation, or `None` when there is no
/// content. Appending `.` (never replacing existing `!`/`?`) keeps rendered
/// bullets matching the schema example's complete-sentence style.
fn normalize_sentence(s: &str) -> Option<String> {
    let mut sentence = collapse_ws(s);
    if sentence.is_empty() {
        return None;
    }
    if !sentence.ends_with(['.', '!', '?']) {
        sentence.push('.');
    }
    Some(sentence)
}

/// Normalizes one raw action item into a renderable draft, or `None` when
/// nothing usable remains. Each fix here upholds a renderer guarantee from
/// the module-level grammar: descriptions lose a leading `to `, a trailing
/// `.`, and a duplicated ` by <date>` tail (adopting that date when the
/// `due_date` field was null); an owner that would break the `" to "` split
/// is demoted to unassigned; an invalid due date is dropped, never rendered.
fn normalize_action_item(raw: RawActionItem) -> Option<ActionItemDraft> {
    let mut description = collapse_ws(&raw.description);
    if description.len() > 3 && description[..3].eq_ignore_ascii_case("to ") {
        description = description[3..].trim_start().to_string();
    }
    if let Some(stripped) = description.strip_suffix('.') {
        description = stripped.trim_end().to_string();
    }

    let mut due_date = raw.due_date.as_deref().and_then(valid_iso_date);
    if let Some((head, tail_date)) = split_due_tail(&description) {
        due_date = due_date.or(Some(tail_date));
        description = head;
    }
    if description.is_empty() {
        return None;
    }

    let owner = raw
        .owner
        .as_deref()
        .map(collapse_ws)
        .filter(|owner| !owner.is_empty() && !owner.contains(" to "));

    Some(ActionItemDraft {
        description,
        owner,
        due_date,
    })
}

/// Splits a `"{head} by {YYYY-MM-DD}"` description into its head and date.
/// `None` when the description doesn't end in that exact shape (including
/// when stripping it would leave nothing — a date-only "description" is left
/// alone for the empty-check to judge).
fn split_due_tail(description: &str) -> Option<(String, String)> {
    let idx = description.len().checked_sub(10)?;
    if !description.is_char_boundary(idx) {
        return None;
    }
    let (head, tail) = description.split_at(idx);
    let date = valid_iso_date(tail)?;
    let head = head.strip_suffix(" by ")?.trim_end();
    if head.is_empty() {
        return None;
    }
    Some((head.to_string(), date))
}

/// A strict, real-calendar `YYYY-MM-DD`, trimmed — or `None`. Both the shape
/// check and the `chrono` parse matter: `chrono` alone would accept
/// `2026-7-1`, which the grammar (and the MCP `IsoDate` format) must never
/// emit.
fn valid_iso_date(s: &str) -> Option<String> {
    let s = s.trim();
    let b = s.as_bytes();
    let shaped = s.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[8..10].iter().all(u8::is_ascii_digit);
    if shaped && chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok() {
        Some(s.to_string())
    } else {
        None
    }
}

/// Coerces the model's tag candidates toward the schema's kebab-case rule
/// (lowercase, whitespace runs to `-`, a stray leading `#` dropped), then
/// keeps only what [`Tag::parse`] accepts — deduped, capped at [`MAX_TAGS`].
/// Fail-soft by design: a bad tag is not worth failing a distill over.
fn normalize_tags(raw: Vec<String>) -> Vec<Tag> {
    let mut seen = std::collections::HashSet::new();
    let mut tags = Vec::new();
    for candidate in raw {
        let candidate = candidate.trim().trim_start_matches('#').to_lowercase();
        let candidate = candidate.split_whitespace().collect::<Vec<_>>().join("-");
        if let Ok(tag) = Tag::parse(&candidate) {
            if seen.insert(candidate) {
                tags.push(tag);
                if tags.len() == MAX_TAGS {
                    break;
                }
            }
        }
    }
    tags
}

/// Renders one action item per the module-level grammar (without the leading
/// checkbox, which [`render_body`] adds).
fn render_action_item(item: &ActionItemDraft) -> String {
    let owner = item.owner.as_deref().unwrap_or(UNASSIGNED_OWNER);
    match &item.due_date {
        Some(due) => format!("{owner} to {} by {due}.", item.description),
        None => format!("{owner} to {}.", item.description),
    }
}

/// Renders the note's Markdown body: `# Summary`, then `## Decisions`,
/// `## Action items`, and `## Open questions` — each section omitted
/// entirely when it has no entries, matching the locked schema example's
/// shape (which shows no empty sections).
pub fn render_body(output: &DistillOutput) -> String {
    let bullets = |items: &[String]| {
        items
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let mut sections = vec![format!("# Summary\n\n{}", output.summary)];
    if !output.decisions.is_empty() {
        sections.push(format!("## Decisions\n\n{}", bullets(&output.decisions)));
    }
    if !output.action_items.is_empty() {
        let items = output
            .action_items
            .iter()
            .map(|item| format!("- [ ] {}", render_action_item(item)))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("## Action items\n\n{items}"));
    }
    if !output.open_questions.is_empty() {
        sections.push(format!(
            "## Open questions\n\n{}",
            bullets(&output.open_questions)
        ));
    }
    sections.join("\n\n")
}

/// The pre-routing (#43) placeholder every distilled note files under until
/// a real scorer exists: Inbox, with the schema-mandated recorded score
/// (`0.0` — "no routing signal at all", honestly the lowest confidence).
pub fn inbox_routing() -> Routing {
    Routing::Routed {
        project: INBOX.to_string(),
        confidence: 0.0,
    }
}

/// Distills the raw session at `session_path` into a meeting note under
/// `vault_root`, returning where it landed.
///
/// `route` maps the distilled content to its `project` + `confidence` — the
/// seam the routing engine (#43) plugs into; until then callers pass
/// `&|_| inbox_routing()`. The note's `date` is recovered from the session
/// filename's capture timestamp (falling back to today's date-only form for
/// a hand-imported file that doesn't match the scheme), and its `source` is
/// the session's vault-relative path.
pub fn distill_session(
    runner: &dyn HeadlessClaude,
    vault_root: &Path,
    session_path: &Path,
    route: &dyn Fn(&DistillOutput) -> Routing,
) -> Result<DistilledNote, DistillError> {
    let segments = raw_session::read_raw_session(session_path)?;
    if segments.iter().all(|s| s.text.trim().is_empty()) {
        return Err(DistillError::EmptyTranscript);
    }

    // Everything derivable without the model comes first, so a bad path or
    // filename fails before a token is spent.
    let source_rel = session_path
        .strip_prefix(vault_root)
        .map_err(|_| DistillError::SessionOutsideVault(session_path.to_path_buf()))?;
    let source = Source::parse(&source_rel.to_string_lossy().replace('\\', "/"))?;

    let parsed_name = session_path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(naming::parse_session_filename);
    let captured_at = parsed_name
        .as_ref()
        .and_then(|parsed| naming::parse_session_timestamp(&parsed.timestamp));
    let (date, meeting_date) = match captured_at {
        Some(at) => (
            at.to_rfc3339_opts(SecondsFormat::Millis, true),
            at.format("%Y-%m-%d").to_string(),
        ),
        None => {
            let today = Utc::now().format("%Y-%m-%d").to_string();
            (today.clone(), today)
        }
    };

    let request = build_request(&segments, &meeting_date);
    let output = parse_output(&runner.run(&request)?)?;
    let routing = route(&output);

    let id = NoteId::generate().map_err(DistillError::Id)?;
    let body = render_body(&output);
    let note = Note::new(
        id.clone(),
        NoteType::Meeting,
        routing,
        date,
        output.tags.clone(),
        source,
        body,
    )?;

    let title_seed = output
        .title
        .clone()
        .or_else(|| parsed_name.and_then(|parsed| parsed.slug));
    let path = note::write_note(vault_root, &note, title_seed.as_deref())?;

    Ok(DistilledNote {
        path,
        id,
        title: output.title,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DeviceId;
    use chrono::{DateTime, TimeZone, Timelike, Utc};
    use tempfile::tempdir;

    fn segment(index: u64, channel: Channel, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            index,
            channel,
            speaker: None,
            start_ms: index * 1000,
            end_ms: index * 1000 + 500,
            text: text.to_owned(),
        }
    }

    fn device() -> DeviceId {
        DeviceId::parse("k4m2xp7q").unwrap()
    }

    fn instant() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 12, 14, 3, 35)
            .unwrap()
            .with_nanosecond(123_000_000)
            .unwrap()
    }

    /// Writes a two-line session into `<vault>/sessions/` and returns its path.
    fn write_session(vault: &Path, slug: Option<&str>) -> PathBuf {
        let segments = vec![
            segment(0, Channel::You, "lets sync on the budget"),
            segment(1, Channel::Them, "I'll send the memo by the fifteenth"),
        ];
        raw_session::write_raw_session(
            &vault.join("sessions"),
            instant(),
            &device(),
            slug,
            &segments,
        )
        .unwrap()
    }

    struct MockRunner(Result<String, LlmRunError>);

    impl HeadlessClaude for MockRunner {
        fn run(&self, _request: &LlmRequest) -> Result<String, LlmRunError> {
            self.0.clone()
        }
    }

    struct PanicRunner;

    impl HeadlessClaude for PanicRunner {
        fn run(&self, _request: &LlmRequest) -> Result<String, LlmRunError> {
            panic!("runner should not be called when there is nothing to distill");
        }
    }

    fn full_output_json() -> String {
        r#"{
            "title": "Budget sync",
            "summary": "Talked through the Q3 budget.",
            "decisions": ["Approve the revised budget"],
            "action_items": [
                {"owner": "Shane", "description": "send the memo", "due_date": "2026-07-15"},
                {"owner": null, "description": "book the follow-up", "due_date": null}
            ],
            "open_questions": ["Who owns vendor outreach?"],
            "tags": ["budgeting", "phase-2"]
        }"#
        .to_string()
    }

    fn draft(description: &str, owner: Option<&str>, due_date: Option<&str>) -> ActionItemDraft {
        ActionItemDraft {
            description: description.to_string(),
            owner: owner.map(str::to_string),
            due_date: due_date.map(str::to_string),
        }
    }

    // ------------------------------------------------------------------
    // parse_output
    // ------------------------------------------------------------------

    #[test]
    fn parses_a_bare_json_object() {
        let output = parse_output(&full_output_json()).expect("should parse");

        assert_eq!(output.title.as_deref(), Some("Budget sync"));
        assert_eq!(output.summary, "Talked through the Q3 budget.");
        assert_eq!(output.decisions, vec!["Approve the revised budget."]);
        assert_eq!(
            output.action_items,
            vec![
                draft("send the memo", Some("Shane"), Some("2026-07-15")),
                draft("book the follow-up", None, None),
            ]
        );
        assert_eq!(output.open_questions, vec!["Who owns vendor outreach?"]);
        let tag_strs: Vec<&str> = output.tags.iter().map(Tag::as_str).collect();
        assert_eq!(tag_strs, vec!["budgeting", "phase-2"]);
    }

    #[test]
    fn recovers_an_object_wrapped_in_a_markdown_fence() {
        let wrapped = format!("```json\n{}\n```", full_output_json());

        let output = parse_output(&wrapped).expect("should parse");

        assert_eq!(output.summary, "Talked through the Q3 budget.");
    }

    #[test]
    fn skips_a_decoy_object_before_the_real_one() {
        // The decoy has no `summary`, so it can't deserialize as a distill
        // result; the parser must keep scanning.
        let wrapped = format!(
            "{{\"note\": \"decoy\"}} here you go: {}",
            full_output_json()
        );

        let output = parse_output(&wrapped).expect("should parse");

        assert_eq!(output.summary, "Talked through the Q3 budget.");
    }

    #[test]
    fn output_without_json_is_a_parse_error() {
        let err = parse_output("no json here at all").unwrap_err();

        assert!(matches!(err, DistillError::Parse(_)));
    }

    #[test]
    fn missing_or_blank_summary_is_a_parse_error() {
        for bad in [
            r#"{"decisions": []}"#,
            r#"{"summary": ""}"#,
            r#"{"summary": "   \n  "}"#,
        ] {
            let err = parse_output(bad).unwrap_err();
            assert!(
                matches!(err, DistillError::Parse(_)),
                "input {bad:?} should be a parse error"
            );
        }
    }

    #[test]
    fn minimal_object_with_only_a_summary_parses() {
        let output = parse_output(r#"{"summary": "Short catch-up, nothing actionable."}"#)
            .expect("should parse");

        assert_eq!(output.title, None);
        assert!(output.decisions.is_empty());
        assert!(output.action_items.is_empty());
        assert!(output.open_questions.is_empty());
        assert!(output.tags.is_empty());
    }

    // ------------------------------------------------------------------
    // Normalization
    // ------------------------------------------------------------------

    #[test]
    fn decisions_and_questions_are_collapsed_and_punctuated() {
        let output = parse_output(
            r#"{"summary": "s", "decisions": ["  approve   the\nbudget  ", "", "ship it!"],
                "open_questions": ["who  owns this"]}"#,
        )
        .unwrap();

        assert_eq!(output.decisions, vec!["approve the budget.", "ship it!"]);
        assert_eq!(output.open_questions, vec!["who owns this."]);
    }

    #[test]
    fn action_item_description_loses_leading_to_and_trailing_period() {
        let output = parse_output(
            r#"{"summary": "s", "action_items": [
                {"owner": "Shane", "description": "To send the memo."}]}"#,
        )
        .unwrap();

        assert_eq!(
            output.action_items,
            vec![draft("send the memo", Some("Shane"), None)]
        );
    }

    #[test]
    fn duplicated_due_tail_in_description_is_stripped_and_adopted() {
        let output = parse_output(
            r#"{"summary": "s", "action_items": [
                {"owner": "Shane", "description": "send the memo by 2026-07-15"},
                {"owner": "Priya", "description": "file the report by 2026-07-20", "due_date": "2026-07-18"}]}"#,
        )
        .unwrap();

        // First: the in-description date is adopted. Second: the explicit
        // `due_date` field wins; the tail is stripped either way so the
        // renderer can never emit a doubled date.
        assert_eq!(
            output.action_items,
            vec![
                draft("send the memo", Some("Shane"), Some("2026-07-15")),
                draft("file the report", Some("Priya"), Some("2026-07-18")),
            ]
        );
    }

    #[test]
    fn invalid_due_dates_are_dropped() {
        let output = parse_output(
            r#"{"summary": "s", "action_items": [
                {"owner": "A", "description": "x", "due_date": "next Friday"},
                {"owner": "B", "description": "y", "due_date": "2026-7-1"},
                {"owner": "C", "description": "z", "due_date": "2026-13-40"}]}"#,
        )
        .unwrap();

        assert!(output.action_items.iter().all(|i| i.due_date.is_none()));
    }

    #[test]
    fn owner_that_would_break_the_grammar_is_demoted_to_unassigned() {
        let output = parse_output(
            r#"{"summary": "s", "action_items": [
                {"owner": "Shane to Bob", "description": "hand off the report"},
                {"owner": "  ", "description": "book the room"}]}"#,
        )
        .unwrap();

        assert!(output.action_items.iter().all(|i| i.owner.is_none()));
    }

    #[test]
    fn empty_description_drops_the_item() {
        let output = parse_output(
            r#"{"summary": "s", "action_items": [
                {"owner": "Shane", "description": "  "},
                {"owner": "Shane", "description": "To ."},
                {"owner": "Priya", "description": "real work"}]}"#,
        )
        .unwrap();

        assert_eq!(
            output.action_items,
            vec![draft("real work", Some("Priya"), None)]
        );
    }

    #[test]
    fn invalid_tags_are_dropped_and_coercible_ones_kept() {
        let output = parse_output(
            r##"{"summary": "s",
                "tags": ["Budgeting", "#phase-2", "two words", "bad_tag!", "budgeting"]}"##,
        )
        .unwrap();

        let tag_strs: Vec<&str> = output.tags.iter().map(Tag::as_str).collect();
        assert_eq!(tag_strs, vec!["budgeting", "phase-2", "two-words"]);
    }

    #[test]
    fn title_is_collapsed_and_capped() {
        let long = "word ".repeat(60);
        let output =
            parse_output(&format!(r#"{{"summary": "s", "title": "  {long}  "}}"#)).unwrap();

        let title = output.title.unwrap();
        assert!(title.chars().count() <= MAX_TITLE_LEN);
        assert!(!title.contains("  "));
        assert!(!title.ends_with(' '));
    }

    // ------------------------------------------------------------------
    // Renderer
    // ------------------------------------------------------------------

    /// The body of `docs/FRONTMATTER_SCHEMA.md`'s locked meeting example
    /// (mirrored by `note.rs`'s `MEETING_MD` fixture): rendered from the
    /// example's data, the renderer must reproduce it byte-for-byte.
    #[test]
    fn renders_the_locked_schema_example_body() {
        let output = DistillOutput {
            title: None,
            summary: "Reviewed Q3 budget allocation for the course renovation and agreed on \
                      the irrigation\n\
                      contractor shortlist."
                .to_string(),
            decisions: vec![
                "Approved the revised irrigation budget of $42,000.".to_string(),
                "Selected GreenFlow Systems as the lead contractor for bidding.".to_string(),
            ],
            action_items: vec![
                draft(
                    "send the signed budget memo to finance",
                    Some("Shane"),
                    Some("2026-07-11"),
                ),
                draft(
                    "request formal bids from GreenFlow and two alternates",
                    Some("Priya"),
                    None,
                ),
            ],
            open_questions: Vec::new(),
            tags: Vec::new(),
        };

        assert_eq!(
            render_body(&output),
            "# Summary\n\n\
             Reviewed Q3 budget allocation for the course renovation and agreed on the irrigation\n\
             contractor shortlist.\n\n\
             ## Decisions\n\n\
             - Approved the revised irrigation budget of $42,000.\n\
             - Selected GreenFlow Systems as the lead contractor for bidding.\n\n\
             ## Action items\n\n\
             - [ ] Shane to send the signed budget memo to finance by 2026-07-11.\n\
             - [ ] Priya to request formal bids from GreenFlow and two alternates."
        );
    }

    #[test]
    fn empty_sections_are_omitted() {
        let output = DistillOutput {
            title: None,
            summary: "Nothing was decided.".to_string(),
            decisions: Vec::new(),
            action_items: Vec::new(),
            open_questions: vec!["Should we meet again?".to_string()],
            tags: Vec::new(),
        };

        assert_eq!(
            render_body(&output),
            "# Summary\n\nNothing was decided.\n\n## Open questions\n\n- Should we meet again?"
        );
    }

    /// Test-local implementation of the module-doc grammar — the same parse
    /// Phase 3's `ActionItem` extractor performs. Returns
    /// `(owner, description, due_date)`.
    fn parse_action_line(line: &str) -> Option<(String, String, Option<String>)> {
        let rest = line
            .strip_prefix("- [ ] ")
            .or_else(|| line.strip_prefix("- [x] "))?;
        let rest = rest.strip_suffix('.')?;
        let (owner, rest) = rest.split_once(" to ")?;
        // An optional " by YYYY-MM-DD" tail: 4 marker chars + 10 date chars.
        if let Some(idx) = rest.len().checked_sub(14) {
            if rest.is_char_boundary(idx) && rest[idx..].starts_with(" by ") {
                if let Some(date) = valid_iso_date(&rest[idx + 4..]) {
                    return Some((owner.to_string(), rest[..idx].to_string(), Some(date)));
                }
            }
        }
        Some((owner.to_string(), rest.to_string(), None))
    }

    #[test]
    fn every_grammar_shape_round_trips_through_the_documented_parse() {
        let drafts = [
            draft("send the memo", Some("Shane"), Some("2026-07-15")),
            draft("send the memo", Some("Shane"), None),
            draft("send the memo", None, Some("2026-07-15")),
            draft("send the memo", None, None),
            // A description containing " to " and " by " mid-phrase must not
            // confuse the owner split or the due-date tail.
            draft(
                "talk to finance by phone",
                Some("Priya"),
                Some("2026-08-01"),
            ),
        ];

        for item in &drafts {
            let line = format!("- [ ] {}", render_action_item(item));
            let (owner, description, due) =
                parse_action_line(&line).unwrap_or_else(|| panic!("unparsable line: {line}"));

            assert_eq!(
                owner,
                item.owner.as_deref().unwrap_or(UNASSIGNED_OWNER),
                "owner mismatch for {line}"
            );
            assert_eq!(
                description, item.description,
                "description mismatch for {line}"
            );
            assert_eq!(due, item.due_date, "due date mismatch for {line}");
        }
    }

    // ------------------------------------------------------------------
    // build_request
    // ------------------------------------------------------------------

    #[test]
    fn build_request_labels_channels_and_skips_blank_segments() {
        let segments = vec![
            segment(0, Channel::You, "hello  there"),
            segment(1, Channel::Them, "   "),
            segment(2, Channel::Unknown, "who said this"),
        ];

        let request = build_request(&segments, "2026-07-12");

        assert!(request.prompt.starts_with("Meeting date: 2026-07-12\n"));
        assert!(request.prompt.contains("You: hello there\n"));
        assert!(request.prompt.contains("Unknown: who said this\n"));
        assert!(!request.prompt.contains("Them:"));
        assert!(request.system_prompt.contains("meeting-notes distiller"));
    }

    // ------------------------------------------------------------------
    // distill_session
    // ------------------------------------------------------------------

    #[test]
    fn distills_a_session_into_a_schema_valid_inbox_note() {
        let vault = tempdir().unwrap();
        let session_path = write_session(vault.path(), None);
        let runner = MockRunner(Ok(full_output_json()));

        let distilled = distill_session(&runner, vault.path(), &session_path, &|_| inbox_routing())
            .expect("distill should succeed");

        assert_eq!(
            distilled.path,
            vault.path().join("Inbox").join("budget-sync.md")
        );
        assert_eq!(distilled.title.as_deref(), Some("Budget sync"));

        let written = std::fs::read_to_string(&distilled.path).unwrap();
        let note = Note::from_markdown(&written).expect("note should round-trip");
        assert_eq!(note.id, distilled.id);
        assert_eq!(note.note_type, NoteType::Meeting);
        assert_eq!(note.routing.project(), INBOX);
        assert_eq!(note.routing.confidence(), Some(0.0));
        assert_eq!(note.date, "2026-07-12T14:03:35.123Z");
        assert_eq!(
            note.source,
            Source::parse(&format!(
                "sessions/{}",
                session_path.file_name().unwrap().to_str().unwrap()
            ))
            .unwrap()
        );
        assert!(note.body.starts_with("# Summary"));
        assert!(note.body.contains("## Decisions"));
        assert!(note
            .body
            .contains("- [ ] Shane to send the memo by 2026-07-15."));
        assert!(note
            .body
            .contains("- [ ] Unassigned to book the follow-up."));
        assert!(note.body.contains("## Open questions"));
    }

    #[test]
    fn routing_seam_receives_the_distilled_output() {
        let vault = tempdir().unwrap();
        let session_path = write_session(vault.path(), None);
        let runner = MockRunner(Ok(full_output_json()));

        let distilled = distill_session(&runner, vault.path(), &session_path, &|output| {
            assert_eq!(output.summary, "Talked through the Q3 budget.");
            Routing::Routed {
                project: "Briarwood Golf".to_string(),
                confidence: 0.94,
            }
        })
        .unwrap();

        let note = Note::from_markdown(&std::fs::read_to_string(&distilled.path).unwrap()).unwrap();
        assert_eq!(note.routing.project(), "Briarwood Golf");
        assert_eq!(note.routing.confidence(), Some(0.94));
        assert!(distilled
            .path
            .starts_with(vault.path().join("Briarwood Golf")));
    }

    #[test]
    fn title_falls_back_to_the_session_slug_then_the_id() {
        let vault = tempdir().unwrap();
        let session_path = write_session(vault.path(), Some("briarwood golf sync"));
        let runner = MockRunner(Ok(r#"{"summary": "s"}"#.to_string()));

        let distilled =
            distill_session(&runner, vault.path(), &session_path, &|_| inbox_routing()).unwrap();
        assert_eq!(
            distilled.path,
            vault.path().join("Inbox").join("briarwood-golf-sync.md")
        );

        // No LLM title and no session slug: the writer falls back to the id.
        let bare_session = write_session(vault.path(), None);
        let distilled =
            distill_session(&runner, vault.path(), &bare_session, &|_| inbox_routing()).unwrap();
        assert_eq!(
            distilled.path,
            vault
                .path()
                .join("Inbox")
                .join(format!("{}.md", distilled.id.as_str()))
        );
    }

    #[test]
    fn empty_transcript_short_circuits_without_calling_the_runner() {
        let vault = tempdir().unwrap();
        let sessions = vault.path().join("sessions");
        let empty =
            raw_session::write_raw_session(&sessions, instant(), &device(), None, &[]).unwrap();
        let whitespace = raw_session::write_raw_session(
            &sessions,
            instant(),
            &device(),
            Some("blank"),
            &[segment(0, Channel::You, "   ")],
        )
        .unwrap();

        for session in [empty, whitespace] {
            let err = distill_session(&PanicRunner, vault.path(), &session, &|_| inbox_routing())
                .unwrap_err();
            assert!(matches!(err, DistillError::EmptyTranscript));
        }
    }

    #[test]
    fn runner_error_fails_the_distill_and_writes_nothing() {
        let vault = tempdir().unwrap();
        let session_path = write_session(vault.path(), None);
        let runner = MockRunner(Err(LlmRunError::Spawn("boom".to_owned())));

        let err = distill_session(&runner, vault.path(), &session_path, &|_| inbox_routing())
            .unwrap_err();

        assert!(matches!(err, DistillError::Run(_)));
        assert!(!vault.path().join("Inbox").exists());
    }

    #[test]
    fn malformed_model_output_fails_the_distill_and_writes_nothing() {
        let vault = tempdir().unwrap();
        let session_path = write_session(vault.path(), None);
        let runner = MockRunner(Ok("that meeting was great, no JSON for you".to_owned()));

        let err = distill_session(&runner, vault.path(), &session_path, &|_| inbox_routing())
            .unwrap_err();

        assert!(matches!(err, DistillError::Parse(_)));
        assert!(!vault.path().join("Inbox").exists());
    }

    #[test]
    fn session_outside_the_vault_is_rejected_before_running() {
        let vault = tempdir().unwrap();
        let elsewhere = tempdir().unwrap();
        let session_path = write_session(elsewhere.path(), None);

        let err = distill_session(&PanicRunner, vault.path(), &session_path, &|_| {
            inbox_routing()
        })
        .unwrap_err();

        assert!(matches!(err, DistillError::SessionOutsideVault(_)));
    }

    #[test]
    fn missing_session_file_surfaces_as_a_session_error() {
        let vault = tempdir().unwrap();
        let missing = vault.path().join("sessions").join("nope.jsonl");

        let err = distill_session(&PanicRunner, vault.path(), &missing, &|_| inbox_routing())
            .unwrap_err();

        assert!(matches!(err, DistillError::Session(_)));
    }
}
