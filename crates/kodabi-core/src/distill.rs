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
//! # Long transcripts
//!
//! A prompt over [`DISTILL_INPUT_BUDGET_CHARS`] would overflow the model rather
//! than distill, and retrying the same oversized transcript would fail
//! identically — so instead it is split on segment boundaries, each chunk
//! distilled into the same JSON shape, and the parts merged back down
//! ([`distill_chunked`]). Both paths converge on a single [`DistillOutput`], so
//! the rendered body, the frontmatter contract, and the fail-hard rule above
//! are the same either way: a failure part-way through a chunk sequence writes
//! no note at all.
//!
//! Two bounds keep that path from becoming its own failure mode. The fan-out is
//! capped at [`MAX_DISTILL_CHUNKS`], checked before the first call, because
//! every chunk is a full-timeout call the pipeline serializes behind. And the
//! reduce step is budgeted the same way the map step is: the parts are model
//! output, so nothing bounds their combined size, and [`merge_parts`] batches
//! them into as many rounds as it takes rather than sending an over-budget
//! merge prompt.
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
//! A tentative ("soft") item renders the same line with [`SOFT_MARKER`]
//! appended after the terminal `.` — `- [ ] Dana to look into pricing.
//! (tentative)`. Soft items are extracted and rendered like any other, but the
//! ledger's enrollment gate never tracks them. The marker is peeled off before
//! the rest of the grammar is parsed and before the item id is hashed, so a
//! firm line is byte-identical to what this module has always rendered and no
//! id depends on firmness.
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

use chrono::{DateTime, Local, SecondsFormat, TimeZone, Utc};

use crate::category_examples::CategoryFile;
use crate::llm::{extract_balanced_spans, HeadlessClaude, LlmRequest, LlmRunError};
use crate::naming;
use crate::note::{
    self, MeetingCategory, Note, NoteError, NoteId, NoteType, Routing, Source, Tag, INBOX,
};
use crate::raw_session::{self, RawSessionError, TranscriptSegment};
use crate::routing::{
    self, ExamplesLoadFailure, GlossaryLoadFailure, NoteText, RoutingConfig, RoutingError,
};
use crate::transcription::Channel;

/// The owner token rendered for an action item nobody could be attributed to.
/// Chosen over defaulting to the local user ("You"), which would silently
/// claim commitments the user never took.
pub const UNASSIGNED_OWNER: &str = "Unassigned";

/// The owner token a hand-written note's checkbox lines are attributed to.
///
/// A plain note is the user's own scratchpad: a `- [ ]` line in it is something
/// *they* mean to do, so unlike a meeting's unattributed line it is theirs
/// rather than [`UNASSIGNED_OWNER`]. The distill grammar's `" to "` split is
/// deliberately not applied there (see [`crate::meeting`]), so this is the only
/// owner a plain note's item ever carries.
///
/// **It is hashed into the item id** (`crate::meeting::action_item_id`), so it
/// must be a fixed token and must never track the identity setting — a user
/// renaming themselves would otherwise re-mint every hand-written item's id and
/// orphan its ledger entry. `"You"` is safe to fix because
/// [`Direction::resolve`](crate::ledger::Direction::resolve) maps it to
/// `Mine` outright, ahead of any alias lookup: it is the grammar's own spelling
/// for the local user and no identity can redefine it.
pub const SELF_OWNER: &str = "You";

/// Longest title kept from the model (characters); anything longer is cut.
/// The title only seeds the note's filename slug (itself capped at 40) and
/// the `DistilledNote` echo, so this is a sanity bound, not a schema rule.
const MAX_TITLE_LEN: usize = 120;

/// Most tags kept from the model. The schema puts no cap on `tags`, but a
/// distill that "tags" a meeting with a dozen topics has stopped tagging.
const MAX_TAGS: usize = 8;

/// Longest distill prompt sent in one call, in characters (~4 chars per token,
/// the same heuristic [`crate::embed`] documents for its chunk cap — no
/// tokenizer dependency). A transcript whose prompt exceeds this is chunked and
/// map-reduced by [`distill_chunked`] instead of failing.
///
/// The binding constraint is the runner's per-call timeout (kodabi-llm's
/// `DISTILL_DEFAULT_TIMEOUT_SECS`, 180s), not the model's context window: ~25k
/// tokens of transcript plus a schema-bounded JSON response finishes well
/// inside it, while the window is several times larger again. At ~140 words per
/// minute a meeting under about 90 minutes stays on the single-call path (the
/// common case, unchanged), and a 2-3 hour meeting splits into two or three
/// chunks plus one merge.
const DISTILL_INPUT_BUDGET_CHARS: usize = 100_000;

/// Characters reserved out of the budget for a chunk prompt's non-transcript
/// framing (the meeting date and the "part N of M" preamble), so a packed chunk
/// plus its framing still fits. Pinned against the real framing by a unit test,
/// so growing the preamble past the reserve fails there rather than silently
/// pushing a chunk prompt over budget.
const CHUNK_PROMPT_OVERHEAD_CHARS: usize = 512;

/// Most chunks one distill will fan out into, checked before the first call.
///
/// Every chunk is a separate runner call that gets the full per-call timeout
/// (kodabi-llm's `DISTILL_DEFAULT_TIMEOUT_SECS`, 180s), and the src-tauri
/// pipeline holds its distill lock across the whole sequence — so an unbounded
/// fan-out would block every later meeting's distill for hours while the UI sat
/// on "distilling" with nothing to show. At [`DISTILL_INPUT_BUDGET_CHARS`] per
/// chunk this admits roughly a full day of continuous speech; past that the
/// honest answer is to fail immediately, before a token is spent, so the
/// oversized session can be split and retried.
const MAX_DISTILL_CHUNKS: usize = 24;

/// Failure distilling a session. Every variant is fatal to this run — no
/// note is written — and the raw session on disk is untouched, so the caller
/// can always retry.
#[derive(Debug, thiserror::Error)]
pub enum DistillError {
    #[error(transparent)]
    Session(#[from] RawSessionError),
    #[error("transcript is empty; nothing to distill")]
    EmptyTranscript,
    /// The transcript would need more than [`MAX_DISTILL_CHUNKS`] chunks.
    /// Raised before any call is made, so nothing is spent and nothing is
    /// blocked; splitting the session and retrying is the way through.
    #[error(
        "transcript needs {chunks} distill chunks, over the limit of {max}; \
split this session into shorter ones and retry"
    )]
    TranscriptTooLong { chunks: usize, max: usize },
    /// A chat transcript that never got far enough to be worth a note (see
    /// [`crate::chat::has_distillable_substance`]). Raised before any call is
    /// made — this is a skip, not a fault, and the transcript stays on disk.
    #[error("chat has only {chars} characters of conversation, under the {min} needed to distill")]
    ThinChat { chars: usize, min: usize },
    /// The chat transcript could not be read (missing, or unreadable).
    #[error("could not read the chat transcript: {0}")]
    ChatTranscript(#[source] std::io::Error),
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

/// The distill system prompt's opening: the role, and how to read the
/// channel-prefixed transcript lines [`render_segment_line`] produces.
const SYSTEM_PROMPT_ROLE: &str =
    "You are a meeting-notes distiller. You will be given a meeting's \
date and its transcript, one line per utterance, each prefixed with its speaker channel: \"You\" \
is the local user, \"Them\" is the other participant(s), \"Unknown\" is unattributed audio.";

/// The JSON contract, shared **verbatim** by the distill and merge system
/// prompts so the two passes can never disagree about the response shape
/// [`parse_output`] parses.
///
/// Its own const rather than a substring sliced back out of the assembled
/// prompt at runtime: reworded prompt text is then a compile-time fact instead
/// of a pair of `find`s that panic mid-distill when the wording moves.
pub(crate) const RESPONSE_SHAPE_SPEC: &str = "Respond with ONLY a single JSON object of exactly \
this shape: \
{\"title\": \"<short meeting title, at most 80 characters>\", \"summary\": \"<one to three short \
paragraphs of plain prose>\", \"decisions\": [\"<one complete sentence per decision actually made \
or agreed during the meeting>\"], \"action_items\": [{\"owner\": \"<the responsible person's name, \
or \\\"You\\\" when the local user took it on; null when unclear>\", \"description\": \"<what is \
to be done, as a verb phrase like \\\"send the signed budget memo to finance\\\" - no owner name \
and no due date inside it>\", \"due_date\": \"<YYYY-MM-DD when a date was stated or clearly \
implied relative to the meeting date; otherwise null>\", \"firmness\": \"<firm when someone \
actually committed to doing it; soft when it was only tentative or aspirational, like \\\"we \
should probably look into that sometime\\\">\"}], \"open_questions\": [\"<questions \
raised but left unresolved>\"], \"tags\": [\"<zero to five lowercase-kebab-case topic tags>\"], \
\"category\": \"<the kind of meeting this was, exactly one of: standup | one-on-one | client | \
working-session | review | all-hands | observer. standup is a short recurring status round; \
one-on-one is a private conversation between two people; client is a conversation with an \
external customer or partner; working-session is hands-on work done together; review is an \
assessment of finished work; all-hands is a large company-wide or department-wide address; \
observer is a meeting the local user only listened in on. null when none of them clearly \
fits>\", \"category_confidence\": <0.0-1.0, how strongly the conversation supports that \
category; null when category is null>, \
\"ledger_updates\": [{\"entry\": \"<an id copied exactly from the open commitments listed \
above>\", \"kind\": \"<refresh when it was mentioned as still outstanding, supersede when it \
was replaced by a different action item you extracted, completed when the conversation says it was \
already done>\", \"item\": <the 0-based index into action_items of the item that restates or \
replaces it; null when none does>, \"confidence\": <0.0-1.0, how strongly the conversation \
supports this>, \"quote\": \"<the shortest verbatim excerpt showing it, or null>\"}]}. \
Report a ledger update only for a commitment listed above, at most once each, and only when the \
conversation actually referred to it; when no commitments are listed, ledger_updates must be \
empty.";

/// The reporting rules that follow the shared shape spec in the distill (not
/// merge) system prompt: what counts as reportable, and the no-fabrication rule.
const DISTILL_REPORTING_RULES: &str = "Only report decisions that were actually made and action \
items that are concrete commitments; empty arrays are fine. Never invent owners, dates, decisions, \
or facts not present in the transcript. No prose, no markdown fences, no explanation - only the \
JSON object.";

/// The closing instruction every system prompt ends on. Its own const only
/// because the merge prompt has no reporting rules to carry it.
const JSON_ONLY_RULE: &str = "No prose, no markdown fences, no explanation - only the JSON object.";

/// The pass-specific half of a distill prompt set: everything that differs
/// between a meeting transcript and a chat conversation.
///
/// The JSON contract ([`RESPONSE_SHAPE_SPEC`]) is deliberately **not** a field.
/// It is shared verbatim by every pass, which is the whole reason it is its own
/// const — templating it per pass would turn a shared constant into a shared
/// format string, and the two passes could then disagree about the shape
/// [`parse_output`] parses.
///
/// Everything downstream of the prompts (chunk planning, parsing, rendering,
/// merging) is flavor-agnostic, so a second pass costs one of these rather than
/// a fork of the map-reduce machinery.
pub(crate) struct PromptFlavor {
    /// Opening of the distill system prompt: the role, and how to read the
    /// prefixed lines this pass renders.
    pub(crate) role: &'static str,
    /// Reporting rules appended after the shared shape spec.
    pub(crate) rules: &'static str,
    /// Opening of the merge system prompt, before the shared shape spec.
    pub(crate) merge_role: &'static str,
    /// Label on the prompt's date line ("Meeting date" | "Chat date").
    pub(crate) date_label: &'static str,
    /// Heading over the prompt's body block ("Transcript" | "Conversation").
    pub(crate) body_label: &'static str,
    /// What the chunk framing calls the whole thing being split, as it reads
    /// after "one continuous " ("meeting transcript" | "chat conversation").
    pub(crate) split_subject: &'static str,
    /// How the merge prompt names what was distilled in parts, as a sentence
    /// subject ("The meeting's transcript" | "The conversation").
    pub(crate) merge_subject: &'static str,
}

/// The meeting pass's flavor: the prompt text this module has always sent,
/// byte for byte (pinned by `meeting_flavor_reproduces_the_locked_prompts`).
pub(crate) const MEETING_FLAVOR: PromptFlavor = PromptFlavor {
    role: SYSTEM_PROMPT_ROLE,
    rules: DISTILL_REPORTING_RULES,
    merge_role: MERGE_PROMPT_ROLE,
    date_label: "Meeting date",
    body_label: "Transcript",
    split_subject: "meeting transcript",
    merge_subject: "The meeting's transcript",
};

/// The distill system prompt, assembled from its shared and pass-specific
/// parts. Identical for a whole meeting and for one chunk of a long one: a
/// chunk is asked for exactly the same JSON shape.
pub(crate) fn system_prompt(flavor: &PromptFlavor) -> String {
    format!("{} {RESPONSE_SHAPE_SPEC} {}", flavor.role, flavor.rules)
}

/// What a conversation did to a commitment the ledger already holds.
///
/// The three are the whole vocabulary: a commitment can be brought up again,
/// replaced by a different one, or reported done. Anything else the model
/// might say about it is not something the ledger can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerUpdateKind {
    /// Mentioned as still outstanding. Resets the aging clock; no state change.
    Refresh,
    /// Replaced by a different commitment made in this conversation.
    Supersede,
    /// Reported as already done. Evidence, not a verdict: whether it closes
    /// the entry or parks it for review is the confidence split's call.
    Completed,
}

impl LedgerUpdateKind {
    /// Parses the wire spelling, `None` for anything else. Case-insensitive
    /// because the model writes these, and a capitalized "Refresh" is the
    /// right answer spelled differently rather than a different answer.
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "refresh" => Some(LedgerUpdateKind::Refresh),
            "supersede" | "superseded" | "supersedes" => Some(LedgerUpdateKind::Supersede),
            "completed" | "complete" | "done" => Some(LedgerUpdateKind::Completed),
            _ => None,
        }
    }
}

/// One classification of an open commitment, normalized and ready to apply.
///
/// "Draft" for the same reason [`ActionItemDraft`] is: this is what the
/// conversation said, not what the ledger decided to do about it.
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerUpdateDraft {
    /// The entry the update is about. Validated against the entries actually
    /// shown to the model before it leaves this module, and again against the
    /// ledger before anything is written.
    pub entry_id: String,
    pub kind: LedgerUpdateKind,
    /// Index into [`DistillOutput::action_items`] of the item that restates
    /// (refresh) or replaces (supersede) the commitment; `None` when none
    /// does. Already remapped past the items normalization dropped.
    pub item: Option<usize>,
    /// How strongly the conversation supports this, clamped to `0.0..=1.0`.
    pub confidence: f64,
    /// The shortest excerpt showing it, when the model quoted one.
    pub quote: Option<String>,
}

/// The confidence a completion claim gets when the model omitted one.
///
/// Deliberately below any sane auto-close threshold: an unquantified claim
/// parks for a human rather than closing a commitment on its own.
const UNSTATED_CONFIDENCE: f64 = 0.5;

/// The distilled content of one meeting, parsed and normalized from the
/// model's JSON. [`render_body`] turns this into the note's Markdown body;
/// [`route_distilled`] scores it into a [`Routing`].
#[derive(Debug, Clone, PartialEq)]
pub struct DistillOutput {
    /// Short meeting title; seeds the note's filename slug. `None` when the
    /// model produced nothing usable.
    pub title: Option<String>,
    /// Summary prose; never empty in anything that reaches a note
    /// ([`parse_output`] rejects an output without one). The intermediate
    /// per-chunk outputs of a map-reduced transcript are the one exception.
    pub summary: String,
    pub decisions: Vec<String>,
    pub action_items: Vec<ActionItemDraft>,
    pub open_questions: Vec<String>,
    /// Already-validated tags; the model's invalid candidates are dropped,
    /// not surfaced as errors.
    pub tags: Vec<Tag>,
    /// The meeting's genre, as the model classified it. `None` when the model
    /// declined to pick one or named something outside the closed set.
    ///
    /// Set on every flavor's output because the response shape is shared, but
    /// only *written* for a meeting: [`distill_rendered`] drops it on the chat
    /// path, where the facet has no meaning.
    pub category: Option<MeetingCategory>,
    /// How strongly the model backed [`DistillOutput::category`], clamped to
    /// `0.0..=1.0`. [`UNSTATED_CONFIDENCE`] when a category came back without
    /// one; `None` exactly when the category is `None`.
    pub category_confidence: Option<f64>,
    /// What this conversation did to commitments the ledger already held.
    /// Empty unless open entries were shown to the model, and empty on the
    /// map-reduced path (see [`distill_rendered`]).
    pub ledger_updates: Vec<LedgerUpdateDraft>,
}

/// One extracted action item, pre-rendering. "Draft" because the durable
/// `ActionItem` shape (id, status, source note) is minted downstream — this
/// carries only what the meeting itself said.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ActionItemDraft {
    /// What is to be done, as a verb phrase; never empty, never owner- or
    /// date-suffixed (the renderer adds those).
    pub description: String,
    /// Who owns it; `None` renders as [`UNASSIGNED_OWNER`]. Never contains
    /// `" to "` (grammar guard).
    pub owner: Option<String>,
    /// Validated `YYYY-MM-DD`, or `None`.
    pub due_date: Option<String>,
    /// Whether the speaker actually committed to this, rather than floating it
    /// as tentative or aspirational. Soft items are still extracted and still
    /// render into the note (as a [`SOFT_MARKER`]-suffixed line), but the
    /// ledger's enrollment gate never tracks them — extraction is not tracking.
    ///
    /// Defaults to firm everywhere it cannot be read: a model that ignores the
    /// field, an older response, an unparseable value. Enrolling something
    /// tentative is a recoverable annoyance; silently dropping a real
    /// commitment is not.
    pub firm: bool,
}

/// [`distill_session`]'s successful result: where the note landed, its
/// minted id, and the title used (if any) for the filename slug.
#[derive(Debug, Clone)]
pub struct DistilledNote {
    pub path: PathBuf,
    pub id: NoteId,
    pub title: Option<String>,
    /// What this conversation said about commitments the ledger already held.
    /// The caller applies them: this module writes notes, not ledger state.
    pub ledger_updates: Vec<LedgerUpdateDraft>,
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
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    category_confidence: Option<f64>,
    #[serde(default)]
    ledger_updates: Vec<RawLedgerUpdate>,
}

/// One classification of an already-open commitment, as the model returns it.
/// Same all-defaulting posture as [`RawActionItem`]: a malformed update is
/// dropped in normalization rather than costing the whole distill.
#[derive(serde::Deserialize)]
struct RawLedgerUpdate {
    #[serde(default)]
    entry: String,
    #[serde(default)]
    kind: String,
    /// Signed on the wire so a negative index deserializes and is then
    /// rejected, rather than failing the parse of every update beside it.
    #[serde(default)]
    item: Option<i64>,
    #[serde(default)]
    confidence: Option<f64>,
    #[serde(default)]
    quote: Option<String>,
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
    /// `"firm"` / `"soft"`, and `None` when the model omitted it. Anything
    /// other than a case-insensitive `"soft"` normalizes to firm, so a model
    /// that never learned the field keeps today's behaviour exactly.
    #[serde(default)]
    firmness: Option<String>,
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

/// One utterance or turn rendered for the prompt, kept alongside the pieces the
/// chunk planner needs so it is collapsed and shaped exactly once per distill
/// (the single-call path and the chunked path share this).
pub(crate) struct RenderedLine {
    /// The line's speaker prefix (`You`/`Them`/`Unknown` for a meeting,
    /// `You`/`Claude`/`Tool` for a chat), re-carried on every piece when an
    /// oversized line has to be split.
    label: &'static str,
    /// The whitespace-collapsed text, without prefix or newline.
    text: String,
    /// `"{label}: {text}\n"` — what actually goes into the prompt.
    line: String,
}

impl RenderedLine {
    /// Shapes one prompt line, or `None` when `raw_text` collapses to nothing.
    /// The single place a line is built, so every pass packs exactly the text
    /// the single-call path would have sent.
    pub(crate) fn new(label: &'static str, raw_text: &str) -> Option<Self> {
        let text = collapse_ws(raw_text);
        if text.is_empty() {
            return None;
        }
        let mut line = String::new();
        let _ = writeln!(line, "{label}: {text}");
        Some(Self { label, text, line })
    }

    /// `"{label}: {text}\n"` — exactly what goes into the prompt.
    pub(crate) fn line(&self) -> &str {
        &self.line
    }
}

/// One transcript line — `"{channel}: {text}\n"` — or `None` for a segment
/// with nothing but whitespace.
fn render_segment_line(segment: &TranscriptSegment) -> Option<RenderedLine> {
    RenderedLine::new(channel_label(segment.channel), &segment.text)
}

/// Renders every non-empty segment once. Both prompt paths start here, so no
/// transcript is collapsed or formatted twice in a run.
fn render_lines(segments: &[TranscriptSegment]) -> Vec<RenderedLine> {
    segments.iter().filter_map(render_segment_line).collect()
}

/// The transcript block of the prompt: the rendered lines, concatenated.
fn transcript_from_lines(lines: &[RenderedLine]) -> String {
    let mut transcript = String::with_capacity(lines.iter().map(|line| line.line().len()).sum());
    for line in lines {
        transcript.push_str(line.line());
    }
    transcript
}

/// The transcript block of the prompt: one line per non-empty segment.
fn render_transcript(segments: &[TranscriptSegment]) -> String {
    transcript_from_lines(&render_lines(segments))
}

/// One open commitment as the distill prompt shows it.
///
/// Deliberately three fields: an id to refer back to, and the two the model
/// needs to recognize the thing being talked about. Dates, states and history
/// would be tokens spent on distinctions the classification does not draw.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OpenCommitment {
    pub entry_id: String,
    pub owner: String,
    pub description: String,
}

/// How many open commitments the prompt will name, and the character ceiling
/// they share.
///
/// Both bounds are real: a long-running project accumulates entries without
/// limit, and every one of them costs transcript budget. Forty is well past
/// what any single meeting refers back to, and the character cap catches the
/// pathological case of a few very long descriptions.
const LEDGER_CONTEXT_MAX_ENTRIES: usize = 40;
const LEDGER_CONTEXT_MAX_CHARS: usize = 8_000;

/// Renders the open commitments as the prompt block, or `None` when there are
/// none to show.
///
/// JSON rather than prose for the same reason the cleanup pass serializes its
/// glossary that way: the ids have to come back byte-identical, and a bulleted
/// list invites the model to paraphrase them.
fn ledger_context_block(open: &[OpenCommitment]) -> Option<String> {
    let mut shown: Vec<&OpenCommitment> = Vec::new();
    let mut chars = 0usize;
    for commitment in open.iter().take(LEDGER_CONTEXT_MAX_ENTRIES) {
        // The serialized length of this entry, near enough: the exact framing
        // is a few characters either way and the cap is not a cliff.
        let cost = commitment.entry_id.chars().count()
            + commitment.owner.chars().count()
            + commitment.description.chars().count()
            + 48;
        if chars + cost > LEDGER_CONTEXT_MAX_CHARS {
            break;
        }
        chars += cost;
        shown.push(commitment);
    }
    if shown.is_empty() {
        return None;
    }
    let payload = serde_json::to_string(&shown).ok()?;
    Some(format!(
        "Open commitments already recorded for this project:\n{payload}"
    ))
}

/// How many recorded categorizations the prompt will show, and the character
/// ceiling they share.
///
/// Far smaller than the ledger's bounds, and deliberately so: this block exists
/// to teach a *recurring* meeting's genre, so the handful of most recent
/// corrections carry nearly all the signal, and the rest would only spend
/// transcript budget.
const CATEGORY_CONTEXT_MAX_EXAMPLES: usize = 8;
const CATEGORY_CONTEXT_MAX_CHARS: usize = 2_000;

/// The prompt block naming who the local user is, or `None` when they have not
/// said.
///
/// The channel labels are already ground truth: `"You:"` is the mic, `"Them:"`
/// is the loopback, and that split is the whole of v1's speaker attribution
/// ([`crate::transcription::Channel`]). What the model lacks is the *name* that
/// goes with the mic, and without it a first-person commitment spoken on the
/// mic channel can be attributed to whichever name it happened to hear in the
/// room. This block closes that gap.
///
/// **It does not change the owner spelling.** [`RESPONSE_SHAPE_SPEC`] still
/// asks for `"You"` when the local user took something on, and this block
/// reinforces that rather than competing with it - the name is here to identify
/// the person, not to be copied into the owner field. Emitting the name instead
/// would re-mint every action-item id against a spelling the existing ledger
/// has never seen, so `owner_norm` would stop matching the commitments already
/// tracked as `"You"`.
fn identity_context_block(identity: &crate::settings::IdentitySettings) -> Option<String> {
    let identity = identity.normalized();
    if identity.is_unset() {
        return None;
    }

    let mut names = String::new();
    if !identity.display_name.is_empty() {
        let _ = write!(names, "\"{}\"", identity.display_name);
    }
    for alias in &identity.aliases {
        if !names.is_empty() {
            names.push_str(", ");
        }
        let _ = write!(names, "\"{alias}\"");
    }

    let mut block = format!("The local user - the \"You\" channel - is {names}.");
    let _ = write!(
        block,
        " A commitment that person takes on is owned by \"You\", however the \
transcript happens to name them and whoever said it out loud. A first-person \
commitment on a \"Them\" line belongs to that speaker: use their name when the \
conversation gives one, otherwise \"Them\"."
    );
    Some(block)
}

/// Renders a project's category prior and recent corrections as a prompt block,
/// or `None` when it has neither.
///
/// Prose rather than the ledger block's JSON, because the two blocks want
/// opposite things: a ledger update has to quote an id back byte-identically,
/// while this is guidance the model weighs against the transcript. Saying so in
/// sentences is what keeps it guidance — the transcript can always overrule a
/// prior, and a note that reads as data invites the model to obey it instead.
fn category_context_block(file: &CategoryFile) -> Option<String> {
    let mut block = String::new();
    if let Some(default) = file.default {
        let _ = write!(
            block,
            "Meetings in this project are usually \"{}\".",
            default.as_str()
        );
    }

    let mut chars = block.chars().count();
    let mut lines: Vec<String> = Vec::new();
    for example in file.most_recent(CATEGORY_CONTEXT_MAX_EXAMPLES) {
        let line = format!(
            "- \"{}\" -> {}: {}",
            example.title,
            example.category.as_str(),
            example.excerpt
        );
        let cost = line.chars().count() + 1;
        if chars + cost > CATEGORY_CONTEXT_MAX_CHARS {
            break;
        }
        chars += cost;
        lines.push(line);
    }

    if !lines.is_empty() {
        if !block.is_empty() {
            block.push(' ');
        }
        let _ = write!(
            block,
            "Meetings in this project the user has categorized by hand:\n{}",
            lines.join("\n")
        );
    }

    if block.is_empty() {
        return None;
    }
    let _ = write!(
        block,
        "\nTreat this as guidance about the project, not a rule: classify what \
this conversation actually was."
    );
    Some(block)
}

/// The single-call request around an already-rendered transcript block. The
/// one place that prompt is shaped, so [`build_request`] and
/// [`distill_rendered`] (which measures before it commits) can never drift
/// apart.
fn request_from_transcript(
    transcript: &str,
    prompt_date: &str,
    flavor: &PromptFlavor,
    ledger_context: Option<&str>,
) -> LlmRequest {
    // Without a context block this is byte-for-byte the prompt this pass has
    // always sent, which is what `meeting_flavor_reproduces_the_locked_prompts`
    // pins: the ledger block is an addition to the prompt, not a rewrite of it.
    let context = match ledger_context {
        Some(block) => format!("\n\n{block}"),
        None => String::new(),
    };
    LlmRequest {
        system_prompt: system_prompt(flavor),
        prompt: format!(
            "{}: {prompt_date}{context}\n\n{}:\n{transcript}",
            flavor.date_label, flavor.body_label
        ),
    }
}

/// Builds the headless request for `segments`. `meeting_date` (`YYYY-MM-DD`)
/// is included so relative phrasings ("by Friday") can resolve to real
/// dates. Timestamps are deliberately omitted — they cost tokens and carry
/// no extraction value at this granularity.
pub fn build_request(segments: &[TranscriptSegment], meeting_date: &str) -> LlmRequest {
    request_from_transcript(
        &render_transcript(segments),
        meeting_date,
        &MEETING_FLAVOR,
        None,
    )
}

/// Splits the transcript into chunk bodies of at most `budget_chars`
/// characters, cutting **only between rendered lines** so every line keeps its
/// `You:`/`Them:`/`Unknown:` prefix and the channel attribution survives the
/// split. Characters, not bytes, so a multibyte transcript isn't over-counted.
///
/// Concatenating the result reproduces [`transcript_from_lines`] exactly,
/// except where a single utterance longer than the whole budget had to be
/// word-split ([`split_oversized_line`]).
fn plan_chunk_transcripts(lines: &[RenderedLine], budget_chars: usize) -> Vec<String> {
    let budget = budget_chars.max(1);
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_chars = 0usize;

    for line in lines {
        let pieces = if line.line.chars().count() > budget {
            split_oversized_line(line.label, &line.text, budget)
        } else {
            vec![line.line.clone()]
        };
        for piece in pieces {
            let piece_chars = piece.chars().count();
            if current_chars > 0 && current_chars + piece_chars > budget {
                chunks.push(std::mem::take(&mut current));
                current_chars = 0;
            }
            current.push_str(&piece);
            current_chars += piece_chars;
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Splits one utterance that is longer than the whole chunk budget into lines
/// of at most `limit_chars`, **re-carrying `label` on every piece** — that is
/// what keeps you/them attribution intact through a mid-utterance cut.
///
/// The splitting itself is [`crate::embed::hard_split`] (break at the last
/// whitespace inside the window, char boundaries as the last resort): the same
/// budget-bounded split the embedding chunker already needs, so the two can't
/// drift. Only the prefix re-carrying is this pass's business.
fn split_oversized_line(label: &str, collapsed_text: &str, limit_chars: usize) -> Vec<String> {
    // `label` + ": " + the text + "\n".
    let overhead = label.chars().count() + 3;
    let text_budget = limit_chars.saturating_sub(overhead).max(1);

    crate::embed::hard_split(collapsed_text, text_budget)
        .into_iter()
        .map(|piece| format!("{label}: {piece}\n"))
        .collect()
}

/// Builds the request for chunk `part` of `total` (1-based). Reuses
/// [`system_prompt`] unchanged — a chunk is asked for exactly the same JSON
/// shape as a whole meeting, so [`parse_output`] handles both — and frames the
/// partiality in the user prompt so the model doesn't guess at what it can't see.
fn build_chunk_request(
    chunk_transcript: &str,
    prompt_date: &str,
    part: usize,
    total: usize,
    flavor: &PromptFlavor,
) -> LlmRequest {
    LlmRequest {
        system_prompt: system_prompt(flavor),
        prompt: format!(
            "{}: {prompt_date}\n\nThis is part {part} of {total} of one continuous \
{}, split only for length; the other parts are distilled separately and merged \
afterward. Report only what appears in this part.\n\n{} (part {part} of {total}):\n\
{chunk_transcript}",
            flavor.date_label, flavor.split_subject, flavor.body_label
        ),
    }
}

/// Opening of the meeting merge system prompt, before the shared shape spec.
const MERGE_PROMPT_ROLE: &str =
    "You are a meeting-notes distiller. One meeting's transcript was distilled in consecutive \
parts; you will be given the meeting's date and those partial results, in order, as a JSON array. \
Merge them into ONE result for the whole meeting: write a single unified summary of the meeting \
(not a list of per-part recaps), keep every distinct decision and action item exactly once (two \
entries are the same only when they describe the same commitment), keep only the open questions no \
later part resolved, and choose one title and the most representative tags. Never invent owners, \
dates, decisions, or facts not present in the partial results.";

/// System prompt for a merge call: the same JSON contract as a chunk (shared
/// verbatim via [`RESPONSE_SHAPE_SPEC`]), a merging role instead of a
/// distilling one.
fn merge_system_prompt(flavor: &PromptFlavor) -> String {
    format!(
        "{} {RESPONSE_SHAPE_SPEC} {JSON_ONLY_RULE}",
        flavor.merge_role
    )
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
    let output = normalize_output(deserialize_output(model_output)?);
    if output.summary.trim().is_empty() {
        return Err(DistillError::Parse("missing or empty summary".into()));
    }
    Ok(output)
}

/// Parses one **chunk** of a map-reduced transcript.
///
/// Same JSON, one relaxation: a chunk's summary is an intermediate the merge
/// call consumes, never anything written to disk, so an empty one is not fatal
/// here the way it is for a whole note. A chunk that carries no content at all
/// — a stretch of hold music or dead air — is reported as `None` and dropped,
/// rather than failing a three-hour meeting over one silent quarter of it.
/// Output that isn't the expected JSON at all still fails the run.
fn parse_chunk_output(model_output: &str) -> Result<Option<DistillOutput>, DistillError> {
    let output = normalize_output(deserialize_output(model_output)?);
    if output.summary.trim().is_empty()
        && output.decisions.is_empty()
        && output.action_items.is_empty()
        && output.open_questions.is_empty()
    {
        return Ok(None);
    }
    Ok(Some(output))
}

/// Finds the model's JSON object: the bare one it was asked for, or the first
/// balanced `{...}` span that deserializes to the expected shape.
fn deserialize_output(model_output: &str) -> Result<RawDistillOutput, DistillError> {
    let trimmed = model_output.trim();
    serde_json::from_str::<RawDistillOutput>(trimmed)
        .ok()
        .or_else(|| {
            extract_balanced_spans(trimmed, '{', '}')
                .into_iter()
                .find_map(|span| serde_json::from_str::<RawDistillOutput>(span).ok())
        })
        .ok_or_else(|| {
            DistillError::Parse("no JSON object of the expected shape in the model output".into())
        })
}

/// Normalizes a deserialized output field by field. The summary requirement is
/// deliberately **not** enforced here — it differs between a whole note
/// ([`parse_output`]) and one chunk ([`parse_chunk_output`]).
fn normalize_output(raw: RawDistillOutput) -> DistillOutput {
    let summary = defuse_headings(raw.summary.replace("\r\n", "\n").trim());
    // Action items are filtered, which renumbers them. The model's indexes
    // point into the list it produced, so the surviving items' original
    // positions are kept to remap the updates below; without it a dropped item
    // would silently slide every later reference onto the wrong commitment.
    let mut kept_from: Vec<usize> = Vec::new();
    let mut action_items: Vec<ActionItemDraft> = Vec::new();
    for (index, item) in raw.action_items.into_iter().enumerate() {
        if let Some(item) = normalize_action_item(item) {
            kept_from.push(index);
            action_items.push(item);
        }
    }
    let ledger_updates = normalize_ledger_updates(raw.ledger_updates, &kept_from);
    let (category, category_confidence) = normalize_category(raw.category, raw.category_confidence);

    DistillOutput {
        title: normalize_title(raw.title),
        summary,
        decisions: raw
            .decisions
            .iter()
            .filter_map(|d| normalize_sentence(d))
            .collect(),
        action_items,
        open_questions: raw
            .open_questions
            .iter()
            .filter_map(|q| normalize_sentence(q))
            .collect(),
        tags: normalize_tags(raw.tags),
        category,
        category_confidence,
        ledger_updates,
    }
}

/// Normalizes the model's genre classification, dropping the unusable rather
/// than failing the distill (the same posture as an invalid tag).
///
/// A category outside the closed set — a hallucinated genre, or a spelling we
/// do not know — takes its confidence with it: a score for a category we
/// discarded describes nothing. A category that arrives without a usable
/// confidence keeps [`UNSTATED_CONFIDENCE`], exactly as an unquantified ledger
/// update does.
fn normalize_category(
    raw_category: Option<String>,
    raw_confidence: Option<f64>,
) -> (Option<MeetingCategory>, Option<f64>) {
    let Some(category) = raw_category
        .as_deref()
        .and_then(MeetingCategory::parse_model)
    else {
        return (None, None);
    };
    let confidence = match raw_confidence {
        Some(value) if value.is_finite() => value.clamp(0.0, 1.0),
        _ => UNSTATED_CONFIDENCE,
    };
    (Some(category), Some(confidence))
}

/// Normalizes the model's commitment classifications, dropping the unusable.
///
/// `kept_from` maps each surviving action item to the position it held in the
/// model's own list, which is what its `item` indexes refer to.
///
/// An update naming an item that normalization dropped keeps its meaning only
/// for a refresh, where the item is a hint and the entry is the point. A
/// supersede without the commitment that replaced it has nothing to link to,
/// so it goes.
fn normalize_ledger_updates(
    raw: Vec<RawLedgerUpdate>,
    kept_from: &[usize],
) -> Vec<LedgerUpdateDraft> {
    raw.into_iter()
        .filter_map(|update| {
            let entry_id = update.entry.trim().to_string();
            if entry_id.is_empty() {
                return None;
            }
            let kind = LedgerUpdateKind::parse(&update.kind)?;
            let item = update
                .item
                .and_then(|index| usize::try_from(index).ok())
                .and_then(|index| kept_from.iter().position(|kept| *kept == index));
            if item.is_none() && kind == LedgerUpdateKind::Supersede {
                return None;
            }
            let confidence = match update.confidence {
                Some(value) if value.is_finite() => value.clamp(0.0, 1.0),
                _ => UNSTATED_CONFIDENCE,
            };
            Some(LedgerUpdateDraft {
                entry_id,
                kind,
                item,
                confidence,
                // Collapsed, not sentence-normalized: this is a verbatim
                // excerpt, and `normalize_sentence` would append a full stop
                // the speaker never said.
                quote: update
                    .quote
                    .as_deref()
                    .map(collapse_ws)
                    .filter(|quote| !quote.is_empty()),
            })
        })
        .collect()
}

/// Collapses every whitespace run (including newlines) to a single space and
/// trims — the one-line normal form every bullet and grammar field needs.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Demotes Markdown heading lines inside the summary to plain prose by
/// stripping their leading `#` run. The summary is the one field rendered
/// with its newlines intact, so a heading-shaped line inside it could
/// fabricate a `## Action items` (or any other) section and corrupt the
/// locked body grammar Phase 3 parses; with headings defused no fake section
/// can exist, and stray bullet or checkbox lines stay inert prose inside
/// `# Summary`.
fn defuse_headings(summary: &str) -> String {
    summary
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with('#') {
                trimmed.trim_start_matches('#').trim_start()
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    // `get` (never direct slicing): a multibyte char spanning byte 3 makes
    // index 3 a non-char-boundary, and `description[..3]` would panic on it.
    let to_prefixed = description
        .get(..3)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("to "));
    if to_prefixed && description.len() > 3 {
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

    // An owner containing " to " — or ending in the word "to", which would
    // fuse with the renderer's own " to " separator into an earlier split
    // point — breaks the grammar's first-" to " parse; demote either shape
    // to unassigned.
    let owner =
        raw.owner.as_deref().map(collapse_ws).filter(|owner| {
            !owner.is_empty() && !owner.contains(" to ") && !owner.ends_with(" to")
        });

    Some(ActionItemDraft {
        description,
        owner,
        due_date,
        firm: is_firm(raw.firmness.as_deref()),
    })
}

/// Reads the model's `firmness` field. Only an explicit, case-insensitive
/// `"soft"` is soft; absent, empty, and unrecognized values are all firm, so
/// the gate this feeds can never silently stop enrolling because a model
/// answered the new field in a way we did not anticipate.
fn is_firm(firmness: Option<&str>) -> bool {
    !firmness.is_some_and(|value| value.trim().eq_ignore_ascii_case("soft"))
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
pub(crate) fn valid_iso_date(s: &str) -> Option<String> {
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
    let soft = if item.firm { "" } else { SOFT_MARKER };
    match &item.due_date {
        Some(due) => format!("{owner} to {} by {due}.{soft}", item.description),
        None => format!("{owner} to {}.{soft}", item.description),
    }
}

/// The suffix a soft (tentative) action item carries, after the line's
/// terminal `.`. A firm line renders byte-identical to what this module
/// rendered before firmness existed, so no existing note re-renders and no
/// existing item id re-mints.
///
/// Deliberately readable prose rather than a sigil: it is the note's own
/// explanation of why the ledger is not tracking that line, and deleting it
/// by hand is a legitimate way to promote the item. It is stripped before
/// [`crate::meeting::action_item_id`] hashes the line, so editing firmness
/// alone never re-mints the id.
pub(crate) const SOFT_MARKER: &str = " (tentative)";

/// The two action-item checkbox markers, exactly as the grammar accepts them
/// (lowercase `x` only).
pub(crate) const UNCHECKED_MARKER: &str = "- [ ] ";
pub(crate) const CHECKED_MARKER: &str = "- [x] ";

/// Peels a leading checkbox marker off a line, returning the remaining text and
/// whether the box was ticked. `None` when the line is not a checkbox line at
/// all.
///
/// The shared first step of both grammars [`crate::meeting::parse_body`] runs:
/// the distill grammar goes on to split the remainder into owner / description /
/// due date ([`parse_action_line`]), while the plain-checkbox grammar keeps it
/// whole. Because it strips only these two exact markers, a
/// [`crate::vault::ANNOTATION_PREFIX`] line (`- Closed …`) is rejected here, in
/// both grammars, by construction.
pub(crate) fn parse_checkbox_line(line: &str) -> Option<(&str, bool)> {
    if let Some(rest) = line.strip_prefix(UNCHECKED_MARKER) {
        return Some((rest, false));
    }
    line.strip_prefix(CHECKED_MARKER).map(|rest| (rest, true))
}

/// Parses one rendered action-item line back into `(owner, description,
/// due_date, firm)` — the inverse of [`render_action_item`] and the exact parse
/// the meeting-facts extractor ([`crate::meeting`]) runs against a note body.
///
/// Accepts either checkbox state (`- [ ] ` / `- [x] `); the caller inspects the
/// prefix itself when it needs the done/open distinction. Returns `None` for a
/// line that does not fit the grammar. A [`SOFT_MARKER`] suffix is peeled off
/// first — it sits *after* the terminal `.`, so the period check below would
/// otherwise reject every soft line — and its presence is the returned
/// firmness. The owner is the prefix before the *first* `" to "`; a trailing
/// `" by {YYYY-MM-DD}"` immediately before the terminal `.` is peeled off as
/// the due date only when it is a valid calendar date, so a description ending
/// in a date-like phrase is not misread.
///
/// Because both this and [`crate::meeting::action_item_line`] run through this
/// one function, the marker cannot fall out of lockstep between the facts
/// extractor and the line rewriter.
pub(crate) fn parse_action_line(line: &str) -> Option<(String, String, Option<String>, bool)> {
    let (rest, _done) = parse_checkbox_line(line)?;
    let (rest, firm) = match rest.strip_suffix(SOFT_MARKER) {
        Some(rest) => (rest, false),
        None => (rest, true),
    };
    let rest = rest.strip_suffix('.')?;
    let (owner, rest) = rest.split_once(" to ")?;
    // An optional " by YYYY-MM-DD" tail: 4 marker chars + 10 date chars.
    if let Some(idx) = rest.len().checked_sub(14) {
        if rest.is_char_boundary(idx) && rest[idx..].starts_with(" by ") {
            if let Some(date) = valid_iso_date(&rest[idx + 4..]) {
                return Some((owner.to_string(), rest[..idx].to_string(), Some(date), firm));
            }
        }
    }
    Some((owner.to_string(), rest.to_string(), None, firm))
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

/// Inbox with the schema-mandated recorded score (`0.0` — "no routing signal
/// at all", honestly the lowest confidence). Two callers: [`route_distilled`]'s
/// fail-soft fallback when signals can't load, and the routing stub in tests.
pub fn inbox_routing() -> Routing {
    Routing::Routed {
        project: INBOX.to_string(),
        confidence: 0.0,
    }
}

/// Scores a distilled meeting into its [`Routing`] via the confidence-split
/// router, adapting [`DistillOutput`] to the router's [`NoteText`] (the model's
/// title, which weighs double, plus `body`).
///
/// `body` is the [`render_body`] Markdown, passed in rather than re-rendered
/// here so the exact string that decides where the note lands is the one
/// [`distill_session`] writes to disk — rendered once, scored and stored. A
/// note explains its own routing. (One consequence: the body's section headings
/// — "Decisions", "Open questions" — are in the scored haystack, so a glossary
/// term literally named after a heading would match every meeting note.
/// Pathological, `TERM_WEIGHT` is only 1.0, and the margin rule still applies,
/// so it is documented, not sanitized.)
///
/// Fail-soft: the model tokens are already spent by the time this runs, so no
/// signal problem may lose the note. Two degradations, both carried out in
/// [`RoutingDiagnostics`] for the caller to surface, neither fatal:
///
/// - **Discovery failure** (the vault is unreadable): no project can be scored,
///   so the note falls back to [`inbox_routing`]. `discovery_failure` is `Some`,
///   and the routing is Inbox at `0.0`.
/// - **A malformed glossary** in one project: that project is *contained* by
///   [`load_project_signals`] (it routes on its name only), routing proceeds
///   normally over every other project, and the failure is listed in
///   `glossary_failures`. The note lands wherever the surviving signals send it.
/// - **A malformed routing-examples log** in one project: contained the same
///   way — that project contributes no correction evidence but still routes on
///   its name and glossary, and the failure is listed in `example_failures`.
///
/// All three diagnostic fields are empty on the happy path.
///
/// `vault_root` must be the same root passed to [`distill_session`]: signals
/// are discovered where the note will be written.
pub fn route_distilled(
    vault_root: &Path,
    output: &DistillOutput,
    body: &str,
    config: &RoutingConfig,
) -> (Routing, RoutingDiagnostics) {
    match routing::load_project_signals(vault_root) {
        Ok(loaded) => {
            let text = NoteText {
                title: output.title.as_deref(),
                body,
            };
            let routing = routing::route(text, &loaded.signals, config);
            (
                routing,
                RoutingDiagnostics {
                    discovery_failure: None,
                    glossary_failures: loaded.glossary_failures,
                    example_failures: loaded.example_failures,
                },
            )
        }
        Err(err) => (
            inbox_routing(),
            RoutingDiagnostics {
                discovery_failure: Some(err),
                glossary_failures: Vec::new(),
                example_failures: Vec::new(),
            },
        ),
    }
}

/// Non-fatal signal-loading diagnostics from [`route_distilled`]: the note is
/// always routed (never lost), but the signals behind that decision may have
/// degraded. All fields are empty on the happy path; see [`route_distilled`]
/// for what each one means.
#[derive(Debug, Default)]
pub struct RoutingDiagnostics {
    /// Set when project discovery failed outright, forcing the note to
    /// [`inbox_routing`]. `Some` implies the routing is Inbox at `0.0`.
    pub discovery_failure: Option<RoutingError>,
    /// Projects whose `_glossary.yml` could not be parsed. Routing still ran,
    /// treating each as having no vocabulary; this is a "fix your glossary"
    /// report, not a routing failure.
    pub glossary_failures: Vec<GlossaryLoadFailure>,
    /// Projects whose `_routing_examples.yml` could not be parsed. Routing still
    /// ran, treating each as having no recorded corrections; this is a "fix your
    /// corrections log" report, not a routing failure.
    pub example_failures: Vec<ExamplesLoadFailure>,
}

/// Drops exact cross-chunk repeats — the same decision, action item (same
/// owner, description and due date), or open question surfacing in more than
/// one part — keeping the first occurrence, in part order.
///
/// Deliberately mechanical: near-duplicates ("send the memo" vs "send the
/// budget memo") are the merge model's judgment call, but an item repeated
/// verbatim because two chunks overlapped on the same commitment is dropped
/// here, deterministically, before it can bias the merge or survive it twice.
///
/// Firmness is deliberately *not* part of that identity. The same commitment
/// heard twice reads as firm in the chunk that caught the actual promise and
/// soft in the one that caught someone musing about it; keying on firmness
/// would keep both copies, and keeping whichever landed first would let chunk
/// order decide whether a real commitment enrolls. So the surviving copy is
/// firm when *any* occurrence was firm — the same fail-toward-enrolling
/// posture the rest of the firmness path takes, and the reason a soft
/// classification can never be an artefact of the chunked path alone.
fn dedup_across_chunks(parts: Vec<DistillOutput>) -> Vec<DistillOutput> {
    type ItemKey = (Option<String>, String, Option<String>);
    fn key(item: &ActionItemDraft) -> ItemKey {
        (
            item.owner.clone(),
            item.description.clone(),
            item.due_date.clone(),
        )
    }

    let firm_anywhere: std::collections::HashSet<ItemKey> = parts
        .iter()
        .flat_map(|part| part.action_items.iter())
        .filter(|item| item.firm)
        .map(key)
        .collect();

    let mut seen_decisions = std::collections::HashSet::new();
    let mut seen_items = std::collections::HashSet::new();
    let mut seen_questions = std::collections::HashSet::new();

    parts
        .into_iter()
        .map(|mut part| {
            part.decisions
                .retain(|decision| seen_decisions.insert(decision.clone()));
            part.action_items.retain_mut(|item| {
                let key = key(item);
                if !seen_items.insert(key.clone()) {
                    return false;
                }
                item.firm = firm_anywhere.contains(&key);
                true
            });
            part.open_questions
                .retain(|question| seen_questions.insert(question.clone()));
            part
        })
        .collect()
}

/// One chunk's result as the merge call sees it. Field order is fixed by the
/// struct, so the same parts always serialize to the same prompt.
#[derive(serde::Serialize)]
struct MergePart<'a> {
    part: usize,
    title: Option<&'a str>,
    summary: &'a str,
    decisions: &'a [String],
    action_items: Vec<MergeActionItem<'a>>,
    open_questions: &'a [String],
    tags: Vec<&'a str>,
    /// Each part's own genre call, carried into the merge so the merging model
    /// weighs them and returns one answer for the whole meeting.
    ///
    /// Deliberately *unlike* `ledger_updates`, which the chunked path clears:
    /// those name entry ids the chunks were never shown, so they cannot be
    /// meaningful. A category is read off the conversation itself, and a long
    /// meeting is precisely the case where the classification matters most, so
    /// dropping it there would be the wrong trade.
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    category_confidence: Option<f64>,
}

#[derive(serde::Serialize)]
struct MergeActionItem<'a> {
    owner: Option<&'a str>,
    description: &'a str,
    due_date: Option<&'a str>,
    /// Carried in the same spelling the response spec asks for, so the merging
    /// model can hand it straight back. Without it the merge would re-answer
    /// firmness from the part summaries alone and every long meeting would
    /// classify differently from a short one.
    firmness: &'static str,
}

/// Builds a merge request from the (already deduped) chunk outputs.
///
/// Fails rather than falling back to an empty payload: a merge prompt that
/// announces N parts and supplies none invites the model to invent a whole
/// meeting, and a fabricated note is the one outcome this module will not
/// produce (a missing one is retryable).
fn build_merge_request(
    parts: &[DistillOutput],
    prompt_date: &str,
    flavor: &PromptFlavor,
) -> Result<LlmRequest, DistillError> {
    let wire: Vec<MergePart<'_>> = parts
        .iter()
        .enumerate()
        .map(|(index, part)| MergePart {
            part: index + 1,
            title: part.title.as_deref(),
            summary: &part.summary,
            decisions: &part.decisions,
            action_items: part
                .action_items
                .iter()
                .map(|item| MergeActionItem {
                    owner: item.owner.as_deref(),
                    description: &item.description,
                    due_date: item.due_date.as_deref(),
                    firmness: if item.firm { "firm" } else { "soft" },
                })
                .collect(),
            open_questions: &part.open_questions,
            tags: part.tags.iter().map(|tag| tag.as_str()).collect(),
            category: part.category.map(|category| category.as_str()),
            category_confidence: part.category_confidence,
        })
        .collect();

    let payload = serde_json::to_string(&wire).map_err(|err| {
        DistillError::Parse(format!(
            "could not encode the chunk results for merging: {err}"
        ))
    })?;
    let total = parts.len();

    Ok(LlmRequest {
        system_prompt: merge_system_prompt(flavor),
        prompt: format!(
            "{}: {prompt_date}\n\n{} was distilled in {total} \
consecutive parts. The partial results, in order:\n{payload}",
            flavor.date_label, flavor.merge_subject
        ),
    })
}

/// Reduces `parts` to one output, in as many merge rounds as the input budget
/// requires.
///
/// A single merge call is the common case and the whole point — but the parts
/// are model output, so their combined size is not bounded by anything the
/// chunker controls. When one merge prompt would exceed `budget_chars` it would
/// overflow exactly the way the un-chunked transcript did, so the parts are
/// first merged in consecutive batches that each fit, and those results merged
/// in turn. Batching consecutively keeps the meeting in chronological order
/// through every round, which is what lets the merge prompt keep saying
/// "consecutive parts, in order" truthfully.
///
/// Terminates because every round that recurses collapses at least one batch of
/// two or more parts into one, strictly shrinking the count.
fn merge_parts(
    runner: &dyn HeadlessClaude,
    mut parts: Vec<DistillOutput>,
    prompt_date: &str,
    budget_chars: usize,
    flavor: &PromptFlavor,
) -> Result<DistillOutput, DistillError> {
    // A lone part is already the merge of its batch; nothing to spend a call
    // on. (`pop` rather than `remove(0)` so an empty vec can't panic here —
    // every caller passes at least one, and this keeps it that way.)
    if parts.len() <= 1 {
        return parts
            .pop()
            .ok_or_else(|| DistillError::Parse("no chunk results left to merge".into()));
    }

    let request = build_merge_request(&parts, prompt_date, flavor)?;
    if request.prompt.chars().count() <= budget_chars {
        return parse_output(&runner.run(&request)?);
    }

    let batch_sizes = plan_merge_batches(&parts, prompt_date, budget_chars, flavor)?;
    if batch_sizes.iter().all(|&size| size == 1) {
        // Even two parts together are over budget, so no amount of batching
        // helps. Send the full request anyway: an honest overflow the runner
        // reports beats silently dropping parts of the meeting.
        return parse_output(&runner.run(&request)?);
    }

    let mut remaining = parts.into_iter();
    let mut reduced = Vec::with_capacity(batch_sizes.len());
    for size in batch_sizes {
        let batch: Vec<DistillOutput> = remaining.by_ref().take(size).collect();
        reduced.push(merge_parts(
            runner,
            batch,
            prompt_date,
            budget_chars,
            flavor,
        )?);
    }
    merge_parts(runner, reduced, prompt_date, budget_chars, flavor)
}

/// Groups `parts` into the longest consecutive runs whose merge request still
/// fits `budget_chars`, returning each run's length. A part that alone fills
/// the budget forms a run of one — there is nothing smaller to try.
fn plan_merge_batches(
    parts: &[DistillOutput],
    prompt_date: &str,
    budget_chars: usize,
    flavor: &PromptFlavor,
) -> Result<Vec<usize>, DistillError> {
    let mut sizes = Vec::new();
    let mut start = 0usize;
    while start < parts.len() {
        let mut size = 1usize;
        while start + size < parts.len() {
            let candidate = build_merge_request(&parts[start..=start + size], prompt_date, flavor)?;
            if candidate.prompt.chars().count() > budget_chars {
                break;
            }
            size += 1;
        }
        sizes.push(size);
        start += size;
    }
    Ok(sizes)
}

/// Map-reduce path for a transcript whose single-call prompt exceeds
/// `budget_chars`: distill each chunk, drop exact repeats, then merge into the
/// same [`DistillOutput`] the single-call path produces.
///
/// Each call gets the runner's full per-call timeout, so wall-clock scales with
/// chunk count rather than one call having to fit a whole long meeting — which
/// is why the fan-out is capped at [`MAX_DISTILL_CHUNKS`] and the cap is checked
/// before the first call, so an absurdly long session fails in milliseconds
/// instead of holding the pipeline for hours.
///
/// A runner error, or output that isn't the expected JSON, aborts the whole
/// distill: no further calls are made and no note is written, keeping the
/// module's fail-hard contract intact for long transcripts too. A chunk that
/// merely has *nothing in it* is the one thing tolerated (see
/// [`parse_chunk_output`]) — dead air in one stretch must not cost the meeting.
fn distill_chunked(
    runner: &dyn HeadlessClaude,
    lines: &[RenderedLine],
    prompt_date: &str,
    budget_chars: usize,
    flavor: &PromptFlavor,
) -> Result<DistillOutput, DistillError> {
    let line_budget = budget_chars
        .saturating_sub(CHUNK_PROMPT_OVERHEAD_CHARS)
        .max(1);
    let transcripts = plan_chunk_transcripts(lines, line_budget);
    let total = transcripts.len();
    if total > MAX_DISTILL_CHUNKS {
        return Err(DistillError::TranscriptTooLong {
            chunks: total,
            max: MAX_DISTILL_CHUNKS,
        });
    }

    let mut parts = Vec::with_capacity(total);
    for (index, transcript) in transcripts.iter().enumerate() {
        let request = build_chunk_request(transcript, prompt_date, index + 1, total, flavor);
        if let Some(part) = parse_chunk_output(&runner.run(&request)?)? {
            parts.push(part);
        }
    }
    if parts.is_empty() {
        return Err(DistillError::Parse(
            "no transcript chunk produced usable content".into(),
        ));
    }

    // One surviving part is merged with nothing, so it goes straight through —
    // but it is now the whole note, and has to clear the whole note's bar.
    if parts.len() == 1 {
        let part = parts.remove(0);
        if part.summary.trim().is_empty() {
            return Err(DistillError::Parse("missing or empty summary".into()));
        }
        return Ok(part);
    }

    merge_parts(
        runner,
        dedup_across_chunks(parts),
        prompt_date,
        budget_chars,
        flavor,
    )
}

/// Render a capture instant as the frontmatter `date` plus its calendar day,
/// both in `tz`'s wall clock.
///
/// Per `docs/FRONTMATTER_SCHEMA.md`, the canonical `date` carries the device's
/// **local** offset at capture time (`2026-07-09T20:00:00-04:00`), not the
/// `…Z` UTC form: the offset preserves the exact instant, but the digits are
/// the user's own day, so an evening meeting near the UTC day boundary files
/// under today rather than tomorrow. Generic over the zone so production passes
/// `&Local` — [`DateTime::with_timezone`] resolves the offset in effect *at
/// that instant*, so a DST-era capture keeps its era's offset — while tests
/// pass a `FixedOffset` and pin exact strings on any host.
pub(crate) fn frontmatter_date_parts<Tz: TimeZone>(at: DateTime<Utc>, tz: &Tz) -> (String, String)
where
    Tz::Offset: std::fmt::Display,
{
    let local = at.with_timezone(tz);
    (
        // `use_z = false`: a device actually on UTC emits `+00:00`, which is
        // its local offset, rather than the `Z` this pass used to force.
        local.to_rfc3339_opts(SecondsFormat::Millis, false),
        local.format("%Y-%m-%d").to_string(),
    )
}

/// Distills the raw session at `session_path` into a meeting note under
/// `vault_root`, returning where it landed.
///
/// `route` maps the distilled content — plus its rendered Markdown body, so the
/// scored text is exactly the text written to disk (rendered once here, shared
/// with the write) — to its `project` + `confidence`: production wraps
/// [`route_distilled`] (resolving config at the src-tauri boundary); tests stub
/// it. The note's `date` is recovered from the session
/// filename's capture timestamp, rendered with the device's local offset at
/// that instant (see [`frontmatter_date_parts`]); a hand-imported file that
/// doesn't match the scheme falls back to the file's modification time (then
/// today) as a local calendar date — the closest honest stand-in, since
/// stamping "now" would fabricate chronology for a weeks-old import. The note's
/// `source` is the session's vault-relative path.
///
/// One model call for a transcript inside [`DISTILL_INPUT_BUDGET_CHARS`];
/// a longer one is chunked and merged by [`distill_chunked`], which costs one
/// call per chunk plus at least one merge and fails the whole distill if any of
/// them fails. Past [`MAX_DISTILL_CHUNKS`] chunks it fails immediately with
/// [`DistillError::TranscriptTooLong`], before any call is spent.
pub fn distill_session(
    runner: &dyn HeadlessClaude,
    vault_root: &Path,
    session_path: &Path,
    identity: Option<&crate::settings::IdentitySettings>,
    route: &dyn Fn(&DistillOutput, &str) -> Routing,
    open_entries: &dyn Fn(&routing::RouteGuess) -> Vec<OpenCommitment>,
) -> Result<DistilledNote, DistillError> {
    let segments = raw_session::read_raw_session(session_path)?;
    if raw_session::is_silent(&segments) {
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
        // Local wall clock for both: the prompt's meeting date is what the
        // model resolves relative due dates against, so it has to agree with
        // the day the note files under.
        Some(at) => frontmatter_date_parts(at, &Local),
        None => {
            // No capture timestamp to recover: prefer the file's mtime (an
            // import usually preserves it) over "today", which would stamp a
            // weeks-old meeting as happening now. Date-only form: whichever
            // fallback wins, the clock time is a guess the schema lets us
            // omit.
            let fallback = std::fs::metadata(session_path)
                .and_then(|meta| meta.modified())
                .map(DateTime::<Utc>::from)
                .unwrap_or_else(|_| Utc::now());
            let date = frontmatter_date_parts(fallback, &Local).1;
            (date.clone(), date)
        }
    };

    // Rendered once, then shared by whichever path runs: a long meeting's
    // transcript is megabytes, and collapsing and formatting it twice to make
    // the same decision is pure waste.
    let lines = render_lines(&segments);

    distill_rendered(
        runner,
        vault_root,
        RenderedDistill {
            flavor: &MEETING_FLAVOR,
            lines: &lines,
            note_type: NoteType::Meeting,
            date: &date,
            prompt_date: &meeting_date,
            source,
            title_seed_fallback: parsed_name.and_then(|parsed| parsed.slug),
            identity,
        },
        route,
        open_entries,
    )
}

/// Everything the shared half of a distill needs that the two passes differ on.
pub(crate) struct RenderedDistill<'a> {
    pub(crate) flavor: &'a PromptFlavor,
    pub(crate) lines: &'a [RenderedLine],
    pub(crate) note_type: NoteType,
    /// The note's frontmatter `date`, already in schema form.
    pub(crate) date: &'a str,
    /// The calendar day the prompt's date line carries, which is what the
    /// model resolves relative due dates against.
    pub(crate) prompt_date: &'a str,
    /// The note's `source`, vault-relative.
    pub(crate) source: Source,
    /// Filename-slug seed used only when the model returns no title.
    pub(crate) title_seed_fallback: Option<String>,
    /// Who the local user is, for the identity block; `None` on passes where
    /// the question does not arise.
    pub(crate) identity: Option<&'a crate::settings::IdentitySettings>,
}

/// The model-and-write half both distill passes share: budget-check the
/// pre-rendered lines, run one call or the map-reduce, render the body once,
/// route it, and write the note.
///
/// Everything upstream — reading the artifact, recovering the date, rendering
/// the lines — stays with the caller, because that is exactly what differs
/// between a meeting transcript and a chat conversation. Everything from here
/// down is identical, which is what keeps the two passes from drifting on the
/// frontmatter contract or the fail-hard rule.
pub(crate) fn distill_rendered(
    runner: &dyn HeadlessClaude,
    vault_root: &Path,
    input: RenderedDistill<'_>,
    route: &dyn Fn(&DistillOutput, &str) -> Routing,
    open_entries: &dyn Fn(&routing::RouteGuess) -> Vec<OpenCommitment>,
) -> Result<DistilledNote, DistillError> {
    // One call while the prompt fits the budget — the overwhelmingly common
    // case, byte-for-byte what this pass has always sent. Only a transcript
    // that would otherwise overflow takes the chunked map-reduce path.
    let transcript = transcript_from_lines(input.lines);
    let bare = request_from_transcript(&transcript, input.prompt_date, input.flavor, None);
    let mut output = if bare.prompt.chars().count() <= DISTILL_INPUT_BUDGET_CHARS {
        // Which project this is has to be guessed here, before the call: the
        // authoritative routing runs on the rendered body, which does not
        // exist yet. A guess is the right instrument anyway — it only decides
        // which commitments are worth showing, and the model ignores the ones
        // the conversation never mentions.
        let guess = guess_project(vault_root, &transcript);
        let open = guess.as_ref().map(open_entries).unwrap_or_default();
        // The same guess picks whose category prior and corrections to show.
        // A project whose file will not load simply contributes no block, the
        // way an unloadable signal set simply shows no commitments.
        let category_block = guess
            .as_ref()
            .filter(|_| input.note_type == NoteType::Meeting)
            .and_then(|guess| {
                let project_dir = note::project_dir(vault_root, &guess.project);
                CategoryFile::load(&project_dir).ok()
            })
            .as_ref()
            .and_then(category_context_block);
        // The note always wins over the context: a transcript already near the
        // budget takes the plain prompt rather than being chunked for the sake
        // of blocks that only ever add precision. Blocks are dropped from the
        // cheapest loss upward - all three, then identity and ledger, then
        // identity alone, then neither.
        //
        // The ordering is by what each miss costs. A missed prior costs one
        // classification the user can correct in a click. A missed commitment
        // update silently loses a tracked promise. A missed identity is the
        // worst of the three and much the smallest to carry: in a context-only
        // meeting the direction *is* the enrolment gate, so an owner the model
        // named a person for produces no ledger row at all, and one sentence
        // naming the user costs a fraction of what either list does.
        let ledger_block = ledger_context_block(&open);
        let identity_block = input.identity.and_then(identity_context_block);
        let fits =
            |request: &LlmRequest| request.prompt.chars().count() <= DISTILL_INPUT_BUDGET_CHARS;
        let build = |blocks: &[&str]| {
            request_from_transcript(
                &transcript,
                input.prompt_date,
                input.flavor,
                Some(blocks.join("\n\n").as_str()),
            )
        };
        // Most-preferred rung first; the first that fits wins, and the bare
        // prompt is the floor that always does.
        let ladder: Vec<Vec<&str>> = [
            vec![
                identity_block.as_deref(),
                ledger_block.as_deref(),
                category_block.as_deref(),
            ],
            vec![identity_block.as_deref(), ledger_block.as_deref()],
            vec![identity_block.as_deref()],
        ]
        .into_iter()
        .map(|rung| rung.into_iter().flatten().collect::<Vec<_>>())
        .filter(|rung| !rung.is_empty())
        .collect();
        // Dropping a block that was never there leaves the rung it was on
        // identical to the one below, and building the same prompt twice to
        // measure it twice is pure waste.
        let mut ladder = ladder;
        ladder.dedup();
        let request = ladder
            .iter()
            .map(|rung| build(rung))
            .find(fits)
            .unwrap_or(bare);
        let mut output = parse_output(&runner.run(&request)?)?;
        // The model can only report on what it was shown. An id it invented,
        // or one from a list it was never given, is dropped here rather than
        // being carried to something that would look it up.
        retain_known_entries(&mut output, &open);
        output
    } else {
        // The whole-transcript prompt is dead weight from here on; the chunked
        // path re-packs the same lines under its own budget.
        drop(bare);
        let mut output = distill_chunked(
            runner,
            input.lines,
            input.prompt_date,
            DISTILL_INPUT_BUDGET_CHARS,
            input.flavor,
        )?;
        // No chunk is ever shown the open commitments, so anything here was
        // invented; and an `item` index cannot survive a merge that rewrites
        // the action-item list anyway. Classification is a single-call
        // capability by construction.
        output.ledger_updates.clear();
        output
    };
    let ledger_updates = std::mem::take(&mut output.ledger_updates);
    // Render the body once: routing scores it and the note stores it, so the
    // text that decides where the note lands is exactly the text on disk.
    let body = render_body(&output);
    let routing = route(&output, &body);

    let id = NoteId::generate().map_err(DistillError::Id)?;
    // The model's title (already ≤120 chars) is stored whole in frontmatter, so
    // it survives past the 40-char filename slug it also seeds. When the model
    // gave none, the title stays unset (the display layer de-slugs the filename)
    // — the caller's fallback below is a *slug* seed, not a display title.
    // The genre is a meeting facet. The response shape is shared with the chat
    // pass, so a chat distill may well return one; it is dropped here rather
    // than in the parser, so the chat path's own tests can still see what the
    // model said.
    let category = (input.note_type == NoteType::Meeting)
        .then_some(output.category)
        .flatten();
    let category_confidence = category.and(output.category_confidence);
    let note = Note::new(
        id.clone(),
        input.note_type,
        routing,
        input.date,
        output.tags.clone(),
        input.source,
        body,
    )?
    .with_title(output.title.clone())
    .with_category(category, category_confidence)?;

    let title_seed = output.title.clone().or(input.title_seed_fallback);
    let path = note::write_note(vault_root, &note, title_seed.as_deref())?;

    Ok(DistilledNote {
        path,
        id,
        title: output.title,
        ledger_updates,
    })
}

/// The project this transcript most likely belongs to, for choosing which open
/// commitments to show the model.
///
/// Advisory by design, and separate from [`route_distilled`]: this runs on the
/// raw transcript before the model has said anything, while routing runs on
/// the rendered body afterwards and is what actually files the note. A vault
/// whose signals will not load simply shows no commitments.
fn guess_project(vault_root: &Path, transcript: &str) -> Option<routing::RouteGuess> {
    let loaded = routing::load_project_signals(vault_root).ok()?;
    routing::best_candidate(
        NoteText {
            title: None,
            body: transcript,
        },
        &loaded.signals,
    )
}

/// Drops classifications naming a commitment that was never shown to the model.
fn retain_known_entries(output: &mut DistillOutput, open: &[OpenCommitment]) {
    output
        .ledger_updates
        .retain(|update| open.iter().any(|entry| entry.entry_id == update.entry_id));
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The fetcher for a distill with no ledger behind it: every call site
    /// that is not exercising the commitment context uses this, so the prompt
    /// it sends is the plain one.
    fn no_open_entries(_: &routing::RouteGuess) -> Vec<OpenCommitment> {
        Vec::new()
    }

    use crate::device::DeviceId;
    use crate::glossary::{Glossary, GlossaryTerm, OnConflict};
    use crate::note::project_dir;
    use crate::routing::DEFAULT_THRESHOLD;
    use chrono::{DateTime, FixedOffset, TimeZone, Timelike, Utc};
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

    /// The calendar day [`instant`] falls on in *this host's* zone — what the
    /// prompt's meeting date and a date-only frontmatter value resolve to.
    fn local_day() -> String {
        instant()
            .with_timezone(&Local)
            .format("%Y-%m-%d")
            .to_string()
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
                {"owner": "Jane", "description": "send the memo", "due_date": "2026-07-15"},
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
            firm: true,
        }
    }

    /// The same draft, classified soft — the shape that renders a
    /// [`SOFT_MARKER`] tail and never reaches the ledger.
    fn soft_draft(
        description: &str,
        owner: Option<&str>,
        due_date: Option<&str>,
    ) -> ActionItemDraft {
        ActionItemDraft {
            firm: false,
            ..draft(description, owner, due_date)
        }
    }

    /// One chunk's result carrying just the action items under test.
    fn part_with(action_items: Vec<ActionItemDraft>) -> DistillOutput {
        DistillOutput {
            action_items,
            ..distill_output(None, "A part.", &[])
        }
    }

    /// Writes a project glossary to disk so `route_distilled` has real signals
    /// to discover and load. Mirrors routing.rs's own test-glossary builder.
    fn save_glossary(vault: &Path, slug: &str, entries: &[(&str, &[&str])]) {
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
        let dir = project_dir(vault, slug);
        std::fs::create_dir_all(&dir).unwrap();
        glossary.save(&dir).unwrap();
    }

    /// The two-project routing fixture (mirroring routing.rs's): a term-rich
    /// `Briarwood Golf` and a smaller `Growth/Q3`. `write_session` separately
    /// creates the `sessions/` folder, which discovery ignores as reserved.
    fn routing_fixture(vault: &Path) {
        save_glossary(
            vault,
            "Briarwood Golf",
            &[
                ("MERIDIAN", &[]),
                ("TeeTrack", &["t-track"]),
                ("GreenFlow", &[]),
                ("irrigation", &[]),
                ("tee sheet", &[]),
            ],
        );
        save_glossary(vault, "Growth/Q3", &[("OKR", &[]), ("activation", &[])]);
    }

    fn distill_output(title: Option<&str>, summary: &str, decisions: &[&str]) -> DistillOutput {
        DistillOutput {
            title: title.map(str::to_string),
            summary: summary.to_string(),
            decisions: decisions.iter().map(|d| d.to_string()).collect(),
            action_items: vec![],
            open_questions: vec![],
            tags: vec![],
            category: None,
            category_confidence: None,
            ledger_updates: vec![],
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
                draft("send the memo", Some("Jane"), Some("2026-07-15")),
                draft("book the follow-up", None, None),
            ]
        );
        assert_eq!(output.open_questions, vec!["Who owns vendor outreach?"]);
        let tag_strs: Vec<&str> = output.tags.iter().map(Tag::as_str).collect();
        assert_eq!(tag_strs, vec!["budgeting", "phase-2"]);
    }

    // ------------------------------------------------------------------
    // category classification
    // ------------------------------------------------------------------

    /// The model's JSON with a classification attached.
    fn categorized_json(category: &str, confidence: &str) -> String {
        format!(
            r#"{{
                "summary": "Talked through the Q3 budget.",
                "category": {category},
                "category_confidence": {confidence}
            }}"#
        )
    }

    #[test]
    fn a_category_parses_with_its_confidence() {
        let output = parse_output(&categorized_json("\"one-on-one\"", "0.82")).expect("parses");

        assert_eq!(output.category, Some(MeetingCategory::OneOnOne));
        assert_eq!(output.category_confidence, Some(0.82));
    }

    #[test]
    fn a_category_without_a_confidence_parks_at_the_unstated_score() {
        let output = parse_output(&categorized_json("\"standup\"", "null")).expect("parses");

        assert_eq!(output.category, Some(MeetingCategory::Standup));
        assert_eq!(output.category_confidence, Some(UNSTATED_CONFIDENCE));
    }

    #[test]
    fn a_confidence_outside_the_range_is_clamped() {
        let high = parse_output(&categorized_json("\"client\"", "1.7")).expect("parses");
        assert_eq!(high.category_confidence, Some(1.0));

        let low = parse_output(&categorized_json("\"client\"", "-0.4")).expect("parses");
        assert_eq!(low.category_confidence, Some(0.0));
    }

    #[test]
    fn a_category_outside_the_closed_set_is_dropped_with_its_confidence() {
        let output = parse_output(&categorized_json("\"retro\"", "0.9")).expect("parses");

        assert_eq!(output.category, None);
        assert_eq!(
            output.category_confidence, None,
            "a score for a discarded category describes nothing"
        );
    }

    #[test]
    fn a_category_is_read_case_and_whitespace_insensitively() {
        let output = parse_output(&categorized_json("\"  All-Hands \"", "0.5")).expect("parses");

        assert_eq!(output.category, Some(MeetingCategory::AllHands));
    }

    #[test]
    fn an_output_with_no_category_carries_neither_field() {
        let output = parse_output(&full_output_json()).expect("parses");

        assert_eq!(output.category, None);
        assert_eq!(output.category_confidence, None);
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
                {"owner": "Jane", "description": "To send the memo."}]}"#,
        )
        .unwrap();

        assert_eq!(
            output.action_items,
            vec![draft("send the memo", Some("Jane"), None)]
        );
    }

    #[test]
    fn duplicated_due_tail_in_description_is_stripped_and_adopted() {
        let output = parse_output(
            r#"{"summary": "s", "action_items": [
                {"owner": "Jane", "description": "send the memo by 2026-07-15"},
                {"owner": "Priya", "description": "file the report by 2026-07-20", "due_date": "2026-07-18"}]}"#,
        )
        .unwrap();

        // First: the in-description date is adopted. Second: the explicit
        // `due_date` field wins; the tail is stripped either way so the
        // renderer can never emit a doubled date.
        assert_eq!(
            output.action_items,
            vec![
                draft("send the memo", Some("Jane"), Some("2026-07-15")),
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
                {"owner": "Jane to Bob", "description": "hand off the report"},
                {"owner": "Jane to", "description": "send the memo"},
                {"owner": "  ", "description": "book the room"}]}"#,
        )
        .unwrap();

        assert!(output.action_items.iter().all(|i| i.owner.is_none()));
    }

    #[test]
    fn multibyte_description_prefix_does_not_panic() {
        // Byte 3 of both descriptions is not a char boundary; the leading-"to"
        // strip must probe with `get`, not a direct slice.
        let output = parse_output(
            r#"{"summary": "s", "action_items": [
                {"owner": "A", "description": "één ding afronden"},
                {"owner": "B", "description": "🎯 aim for the Q3 close"}]}"#,
        )
        .unwrap();

        assert_eq!(output.action_items[0].description, "één ding afronden");
        assert_eq!(
            output.action_items[1].description,
            "🎯 aim for the Q3 close"
        );
    }

    #[test]
    fn heading_lines_inside_the_summary_are_demoted_to_prose() {
        // A heading-shaped line in the summary could otherwise fabricate a
        // `## Action items` section that the documented first-occurrence
        // section scan would parse instead of the real one.
        let output = parse_output(
            "{\"summary\": \"Recap.\\n## Action items\\n- [ ] Bob to wire funds by 2026-08-01.\"}",
        )
        .unwrap();

        assert_eq!(
            output.summary,
            "Recap.\nAction items\n- [ ] Bob to wire funds by 2026-08-01."
        );
        assert!(!render_body(&output).lines().any(|l| l.starts_with("## ")));
    }

    #[test]
    fn empty_description_drops_the_item() {
        let output = parse_output(
            r#"{"summary": "s", "action_items": [
                {"owner": "Jane", "description": "  "},
                {"owner": "Jane", "description": "To ."},
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
                    Some("Jane"),
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
            category: None,
            category_confidence: None,
            ledger_updates: Vec::new(),
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
             - [ ] Jane to send the signed budget memo to finance by 2026-07-11.\n\
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
            category: None,
            category_confidence: None,
            ledger_updates: Vec::new(),
        };

        assert_eq!(
            render_body(&output),
            "# Summary\n\nNothing was decided.\n\n## Open questions\n\n- Should we meet again?"
        );
    }

    #[test]
    fn every_grammar_shape_round_trips_through_the_documented_parse() {
        let drafts = [
            draft("send the memo", Some("Jane"), Some("2026-07-15")),
            draft("send the memo", Some("Jane"), None),
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
            let (owner, description, due, firm) =
                parse_action_line(&line).unwrap_or_else(|| panic!("unparsable line: {line}"));
            assert!(firm, "firmness mismatch for {line}");

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
    fn a_hand_imported_session_dates_to_its_local_mtime_day() {
        let vault = tempdir().unwrap();
        let written = write_session(vault.path(), None);
        // A filename outside the capture scheme: no timestamp to recover, so
        // the writer falls back to the file's mtime as a local calendar date.
        let imported = vault.path().join("sessions").join("imported-meeting.jsonl");
        std::fs::rename(&written, &imported).unwrap();
        let runner = MockRunner(Ok(full_output_json()));

        let distilled = distill_session(
            &runner,
            vault.path(),
            &imported,
            None,
            &|_, _| inbox_routing(),
            &no_open_entries,
        )
        .expect("distill should succeed");

        let mtime =
            DateTime::<Utc>::from(std::fs::metadata(&imported).unwrap().modified().unwrap());
        let note = Note::from_markdown(&std::fs::read_to_string(&distilled.path).unwrap()).unwrap();
        assert_eq!(
            note.date,
            mtime.with_timezone(&Local).format("%Y-%m-%d").to_string()
        );
    }

    #[test]
    fn frontmatter_date_carries_the_zones_offset_not_z() {
        let (date, day) =
            frontmatter_date_parts(instant(), &FixedOffset::west_opt(4 * 3600).unwrap());

        assert_eq!(date, "2026-07-12T10:03:35.123-04:00");
        assert_eq!(day, "2026-07-12");
        // The offset moved the digits without moving the instant.
        assert_eq!(DateTime::parse_from_rfc3339(&date).unwrap(), instant());
    }

    #[test]
    fn frontmatter_date_files_an_east_of_utc_capture_under_its_local_day() {
        // 14:03Z is already tomorrow at +10:00 — the whole point of the local
        // rendering, in the direction the old UTC form got wrong.
        let (date, day) =
            frontmatter_date_parts(instant(), &FixedOffset::east_opt(10 * 3600).unwrap());

        assert_eq!(date, "2026-07-13T00:03:35.123+10:00");
        assert_eq!(day, "2026-07-13");
        assert_eq!(DateTime::parse_from_rfc3339(&date).unwrap(), instant());
    }

    #[test]
    fn frontmatter_date_writes_a_utc_device_as_plus_zero_not_z() {
        // A device actually on UTC still emits its offset numerically: `Z` is
        // the shape the schema calls non-canonical, whatever the zone.
        let (date, day) = frontmatter_date_parts(instant(), &Utc);

        assert_eq!(date, "2026-07-12T14:03:35.123+00:00");
        assert_eq!(day, "2026-07-12");
        assert_eq!(DateTime::parse_from_rfc3339(&date).unwrap(), instant());
    }

    #[test]
    fn distills_a_session_into_a_schema_valid_inbox_note() {
        let vault = tempdir().unwrap();
        let session_path = write_session(vault.path(), None);
        let runner = MockRunner(Ok(full_output_json()));

        let distilled = distill_session(
            &runner,
            vault.path(),
            &session_path,
            None,
            &|_, _| inbox_routing(),
            &no_open_entries,
        )
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
        // The capture instant in this device's own wall clock: exact digits are
        // host-zone dependent, so pin the properties that must hold everywhere.
        // `frontmatter_date_parts` carries the exact-string pins.
        assert_eq!(
            note.date,
            instant()
                .with_timezone(&Local)
                .to_rfc3339_opts(SecondsFormat::Millis, false)
        );
        assert!(!note.date.ends_with('Z'));
        assert_eq!(DateTime::parse_from_rfc3339(&note.date).unwrap(), instant());
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
            .contains("- [ ] Jane to send the memo by 2026-07-15."));
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

        let distilled = distill_session(
            &runner,
            vault.path(),
            &session_path,
            None,
            &|output, _| {
                assert_eq!(output.summary, "Talked through the Q3 budget.");
                Routing::Routed {
                    project: "Briarwood Golf".to_string(),
                    confidence: 0.94,
                }
            },
            &no_open_entries,
        )
        .unwrap();

        let note = Note::from_markdown(&std::fs::read_to_string(&distilled.path).unwrap()).unwrap();
        assert_eq!(note.routing.project(), "Briarwood Golf");
        assert_eq!(note.routing.confidence(), Some(0.94));
        assert!(distilled
            .path
            .starts_with(vault.path().join("Briarwood Golf")));
    }

    // ------------------------------------------------------------------
    // route_distilled
    // ------------------------------------------------------------------

    #[test]
    fn route_distilled_files_a_term_rich_output_into_its_project() {
        let vault = tempdir().unwrap();
        routing_fixture(vault.path());
        // Five distinct Briarwood Golf terms, unopposed: saturate(5) = 5/7. The
        // rendered `# Summary` heading token adds nothing.
        let output = distill_output(
            None,
            "MERIDIAN rollout: TeeTrack sync for the tee sheet, GreenFlow irrigation checks.",
            &[],
        );

        let (routing, diag) = route_distilled(
            vault.path(),
            &output,
            &render_body(&output),
            &RoutingConfig::default(),
        );

        assert!(diag.discovery_failure.is_none() && diag.glossary_failures.is_empty());
        assert_eq!(
            routing,
            Routing::Routed {
                project: "Briarwood Golf".to_string(),
                confidence: 5.0 / 7.0,
            }
        );
    }

    #[test]
    fn route_distilled_scores_the_llm_title() {
        let vault = tempdir().unwrap();
        routing_fixture(vault.path());
        // The only signal is the project name, present only in the title — a
        // routed result proves the title reached the router (weighing double:
        // saturate(2 * 2) = 4/6).
        let output = distill_output(Some("Briarwood Golf weekly sync"), "Agenda to follow.", &[]);

        let (routing, diag) = route_distilled(
            vault.path(),
            &output,
            &render_body(&output),
            &RoutingConfig::default(),
        );

        assert!(diag.discovery_failure.is_none() && diag.glossary_failures.is_empty());
        assert_eq!(
            routing,
            Routing::Routed {
                project: "Briarwood Golf".to_string(),
                confidence: 4.0 / 6.0,
            }
        );
    }

    #[test]
    fn route_distilled_scores_the_rendered_body_not_just_the_summary() {
        let vault = tempdir().unwrap();
        routing_fixture(vault.path());
        // Three Briarwood Golf terms, each in a *decisions* bullet the summary
        // never mentions — so a match proves the rendered body is scored.
        // saturate(3) == DEFAULT_THRESHOLD, pinning the `>=` boundary too:
        // exactly-threshold evidence still auto-files.
        let output = distill_output(
            None,
            "The team met and reviewed progress.",
            &[
                "Adopt the tee sheet import",
                "Approve the TeeTrack contract",
                "Retire MERIDIAN",
            ],
        );

        let (routing, diag) = route_distilled(
            vault.path(),
            &output,
            &render_body(&output),
            &RoutingConfig::default(),
        );

        assert!(diag.discovery_failure.is_none() && diag.glossary_failures.is_empty());
        assert_eq!(
            routing,
            Routing::Routed {
                project: "Briarwood Golf".to_string(),
                confidence: DEFAULT_THRESHOLD,
            }
        );
    }

    #[test]
    fn route_distilled_low_margin_output_lands_in_inbox_with_the_score() {
        let vault = tempdir().unwrap();
        routing_fixture(vault.path());
        // Three Briarwood Golf terms against one Growth/Q3 term: margin 2,
        // saturate(2) = 0.5, below the 0.6 threshold. Contested evidence stays
        // uncategorized, with the score on record.
        let output = distill_output(
            None,
            "MERIDIAN kickoff with TeeTrack on the tee sheet; timeline follows OKR.",
            &[],
        );

        let (routing, diag) = route_distilled(
            vault.path(),
            &output,
            &render_body(&output),
            &RoutingConfig::default(),
        );

        assert!(diag.discovery_failure.is_none() && diag.glossary_failures.is_empty());
        assert_eq!(
            routing,
            Routing::Routed {
                project: INBOX.to_string(),
                confidence: 0.5,
            }
        );
    }

    #[test]
    fn route_distilled_contains_a_malformed_glossary_and_still_routes_other_projects() {
        let vault = tempdir().unwrap();
        routing_fixture(vault.path());
        // Corrupt Briarwood Golf's glossary. Before containment this failed the
        // whole signal load and forced every note to Inbox; now it is contained
        // to Briarwood Golf, so a note matching Growth/Q3 by name still routes
        // there. (Growth/Q3's title segments score 2·2·2 = 8 over Growth's leaf
        // 4: saturate(8 - 4) = 4/6, clearing the threshold.)
        std::fs::write(
            crate::glossary::glossary_path(&project_dir(vault.path(), "Briarwood Golf")),
            "terms: [\n",
        )
        .unwrap();
        let output = distill_output(Some("Growth Q3 sync"), "Agenda to follow.", &[]);

        let (routing, diag) = route_distilled(
            vault.path(),
            &output,
            &render_body(&output),
            &RoutingConfig::default(),
        );

        assert_eq!(
            routing,
            Routing::Routed {
                project: "Growth/Q3".to_string(),
                confidence: 4.0 / 6.0,
            }
        );
        // The routing succeeded, but the broken glossary is still reported.
        assert!(diag.discovery_failure.is_none());
        assert_eq!(diag.glossary_failures.len(), 1);
        assert_eq!(diag.glossary_failures[0].project, "Briarwood Golf");
    }

    #[test]
    fn route_distilled_reports_a_malformed_glossary_when_the_starved_note_lands_in_inbox() {
        let vault = tempdir().unwrap();
        routing_fixture(vault.path());
        // Corrupt Briarwood Golf's glossary, then feed a note whose only signals
        // *were* Briarwood Golf vocabulary. Contained to an empty glossary those
        // terms match nothing, so the note lands in Inbox at 0.0 — but it is not
        // lost, and the failure is reported so the user can fix the glossary.
        std::fs::write(
            crate::glossary::glossary_path(&project_dir(vault.path(), "Briarwood Golf")),
            "terms: [\n",
        )
        .unwrap();
        let output = distill_output(None, "MERIDIAN rollout with TeeTrack.", &[]);

        let (routing, diag) = route_distilled(
            vault.path(),
            &output,
            &render_body(&output),
            &RoutingConfig::default(),
        );

        assert_eq!(routing, inbox_routing());
        assert!(diag.discovery_failure.is_none());
        assert_eq!(diag.glossary_failures.len(), 1);
        assert_eq!(diag.glossary_failures[0].project, "Briarwood Golf");
    }

    #[test]
    fn route_distilled_reports_a_broken_examples_log_and_still_routes() {
        let vault = tempdir().unwrap();
        routing_fixture(vault.path());
        // Corrupt Growth/Q3's corrections log. Like a broken glossary it is
        // contained to that project (which contributes no example evidence), so
        // a term-rich Briarwood Golf note still routes normally — the failure is
        // reported, not fatal.
        std::fs::write(
            crate::routing_examples::routing_examples_path(&project_dir(vault.path(), "Growth/Q3")),
            "examples: [\n",
        )
        .unwrap();
        let output = distill_output(
            None,
            "MERIDIAN rollout with TeeTrack on the GreenFlow irrigation tee sheet.",
            &[],
        );

        let (routing, diag) = route_distilled(
            vault.path(),
            &output,
            &render_body(&output),
            &RoutingConfig::default(),
        );

        // Five unopposed body terms: saturate(5) = 5/7.
        assert_eq!(
            routing,
            Routing::Routed {
                project: "Briarwood Golf".to_string(),
                confidence: 5.0 / 7.0,
            }
        );
        assert!(diag.discovery_failure.is_none());
        assert!(diag.glossary_failures.is_empty());
        assert_eq!(diag.example_failures.len(), 1);
        assert_eq!(diag.example_failures[0].project, "Growth/Q3");
    }

    #[test]
    fn route_distilled_falls_back_to_inbox_when_discovery_fails() {
        let vault = tempdir().unwrap();
        let missing = vault.path().join("does-not-exist");
        // An unreadable vault can't even enumerate candidates: unlike a single
        // bad glossary, this is fatal to routing. The model tokens are already
        // spent, so the note falls back to Inbox with the error carried out.
        let output = distill_output(None, "MERIDIAN rollout with TeeTrack.", &[]);

        let (routing, diag) = route_distilled(
            &missing,
            &output,
            &render_body(&output),
            &RoutingConfig::default(),
        );

        assert_eq!(routing, inbox_routing());
        assert!(matches!(
            diag.discovery_failure,
            Some(RoutingError::Io { .. })
        ));
        assert!(diag.glossary_failures.is_empty());
    }

    // ------------------------------------------------------------------
    // distill_session → route_distilled → write_note → re-parse
    // ------------------------------------------------------------------

    #[test]
    fn distill_routes_a_glossary_matching_transcript_into_its_project() {
        let vault = tempdir().unwrap();
        routing_fixture(vault.path());
        let session_path = write_session(vault.path(), None);
        let runner = MockRunner(Ok(r#"{
            "title": "Briarwood Golf irrigation sync",
            "summary": "GreenFlow demo went well. The tee sheet import from TeeTrack is unblocked.",
            "decisions": [],
            "action_items": [],
            "open_questions": [],
            "tags": []
        }"#
        .to_string()));

        let distilled = distill_session(
            &runner,
            vault.path(),
            &session_path,
            None,
            &|output, body| {
                route_distilled(vault.path(), output, body, &RoutingConfig::default()).0
            },
            &no_open_entries,
        )
        .expect("distill should succeed");

        // Name in title (2*2) + three body terms + irrigation in title (1*2) =
        // 9; saturate(9) = 9/11, comfortably over the threshold.
        assert_eq!(
            distilled.path,
            vault
                .path()
                .join("Briarwood Golf")
                .join("briarwood-golf-irrigation-sync.md")
        );
        let note = Note::from_markdown(&std::fs::read_to_string(&distilled.path).unwrap()).unwrap();
        assert_eq!(note.note_type, NoteType::Meeting);
        assert_eq!(note.routing.project(), "Briarwood Golf");
        assert_eq!(note.routing.confidence(), Some(9.0 / 11.0));
        assert!(note.routing.confidence().unwrap() >= DEFAULT_THRESHOLD);
    }

    #[test]
    fn distill_ambiguous_transcript_lands_in_inbox_with_the_recorded_score() {
        let vault = tempdir().unwrap();
        routing_fixture(vault.path());
        let session_path = write_session(vault.path(), None);
        let runner = MockRunner(Ok(r#"{
            "title": "Kickoff notes",
            "summary": "MERIDIAN kickoff with TeeTrack on the tee sheet; timeline follows OKR.",
            "decisions": [],
            "action_items": [],
            "open_questions": [],
            "tags": []
        }"#
        .to_string()));

        let distilled = distill_session(
            &runner,
            vault.path(),
            &session_path,
            None,
            &|output, body| {
                route_distilled(vault.path(), output, body, &RoutingConfig::default()).0
            },
            &no_open_entries,
        )
        .unwrap();

        assert_eq!(
            distilled.path,
            vault.path().join("Inbox").join("kickoff-notes.md")
        );
        let note = Note::from_markdown(&std::fs::read_to_string(&distilled.path).unwrap()).unwrap();
        assert_eq!(note.routing.project(), INBOX);
        assert_eq!(note.routing.confidence(), Some(0.5));
    }

    #[test]
    fn distill_garbage_output_does_not_confidently_auto_file() {
        let vault = tempdir().unwrap();
        routing_fixture(vault.path());
        let session_path = write_session(vault.path(), None);
        // Nonempty but signal-free: no glossary term matches, so nothing scores.
        let runner = MockRunner(Ok(r#"{
            "title": "Team huddle",
            "summary": "Lorem ipsum placeholder chatter about nothing in particular.",
            "decisions": [],
            "action_items": [],
            "open_questions": [],
            "tags": []
        }"#
        .to_string()));

        let distilled = distill_session(
            &runner,
            vault.path(),
            &session_path,
            None,
            &|output, body| {
                route_distilled(vault.path(), output, body, &RoutingConfig::default()).0
            },
            &no_open_entries,
        )
        .unwrap();

        let note = Note::from_markdown(&std::fs::read_to_string(&distilled.path).unwrap()).unwrap();
        assert_eq!(note.routing.project(), INBOX);
        assert_eq!(note.routing.confidence(), Some(0.0));
        assert!(distilled.path.starts_with(vault.path().join("Inbox")));
        assert!(!distilled
            .path
            .starts_with(vault.path().join("Briarwood Golf")));
        assert!(!distilled.path.starts_with(vault.path().join("Growth")));
    }

    #[test]
    fn distill_contains_a_malformed_glossary_and_still_routes_other_projects() {
        let vault = tempdir().unwrap();
        routing_fixture(vault.path());
        // Corrupt Briarwood Golf's glossary. The distill must neither fail nor
        // let one broken glossary take down routing for the *other* projects: a
        // Growth/Q3 note still files into Growth/Q3.
        std::fs::write(
            crate::glossary::glossary_path(&project_dir(vault.path(), "Briarwood Golf")),
            "terms: [\n",
        )
        .unwrap();
        let session_path = write_session(vault.path(), None);
        let runner = MockRunner(Ok(r#"{
            "title": "Growth Q3 planning",
            "summary": "Agenda to follow.",
            "decisions": [],
            "action_items": [],
            "open_questions": [],
            "tags": []
        }"#
        .to_string()));

        // The production callback drops the diagnostics (`.0`); fail-soft means
        // the note still lands, and containment means it lands in its real
        // project, not forced to Inbox by an unrelated broken glossary.
        let distilled = distill_session(
            &runner,
            vault.path(),
            &session_path,
            None,
            &|output, body| {
                route_distilled(vault.path(), output, body, &RoutingConfig::default()).0
            },
            &no_open_entries,
        )
        .expect("a malformed glossary must not fail the distill");

        assert_eq!(
            distilled.path,
            vault
                .path()
                .join("Growth")
                .join("Q3")
                .join("growth-q3-planning.md")
        );
        let note = Note::from_markdown(&std::fs::read_to_string(&distilled.path).unwrap()).unwrap();
        assert_eq!(note.routing.project(), "Growth/Q3");
        assert_eq!(note.routing.confidence(), Some(4.0 / 6.0));
    }

    #[test]
    fn title_falls_back_to_the_session_slug_then_the_id() {
        let vault = tempdir().unwrap();
        let session_path = write_session(vault.path(), Some("briarwood golf sync"));
        let runner = MockRunner(Ok(r#"{"summary": "s"}"#.to_string()));

        let distilled = distill_session(
            &runner,
            vault.path(),
            &session_path,
            None,
            &|_, _| inbox_routing(),
            &no_open_entries,
        )
        .unwrap();
        assert_eq!(
            distilled.path,
            vault.path().join("Inbox").join("briarwood-golf-sync.md")
        );

        // No LLM title and no session slug: the writer falls back to the id.
        let bare_session = write_session(vault.path(), None);
        let distilled = distill_session(
            &runner,
            vault.path(),
            &bare_session,
            None,
            &|_, _| inbox_routing(),
            &no_open_entries,
        )
        .unwrap();
        assert_eq!(
            distilled.path,
            vault
                .path()
                .join("Inbox")
                .join(format!("{}.md", distilled.id.as_str()))
        );
    }

    #[test]
    fn distilled_note_stores_the_full_model_title_past_the_slug_cap() {
        let vault = tempdir().unwrap();
        let session_path = write_session(vault.path(), None);
        // A model title well over the 40-char filename-slug cap.
        let long_title = "kodabi recording distill flow walkthrough and open questions";
        assert!(long_title.len() > 40);
        let runner = MockRunner(Ok(format!(
            r#"{{"title": "{long_title}", "summary": "s"}}"#
        )));

        let distilled = distill_session(
            &runner,
            vault.path(),
            &session_path,
            None,
            &|_, _| inbox_routing(),
            &no_open_entries,
        )
        .unwrap();

        // The filename slug is still capped for filesystem sanity...
        let stem = distilled.path.file_stem().unwrap().to_str().unwrap();
        assert!(stem.chars().count() <= 40);
        // ...but the frontmatter carries the full title, uncut mid-word.
        let note = Note::from_markdown(&std::fs::read_to_string(&distilled.path).unwrap()).unwrap();
        assert_eq!(note.title.as_deref(), Some(long_title));
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
            let err = distill_session(
                &PanicRunner,
                vault.path(),
                &session,
                None,
                &|_, _| inbox_routing(),
                &no_open_entries,
            )
            .unwrap_err();
            assert!(matches!(err, DistillError::EmptyTranscript));
        }
    }

    #[test]
    fn runner_error_fails_the_distill_and_writes_nothing() {
        let vault = tempdir().unwrap();
        let session_path = write_session(vault.path(), None);
        let runner = MockRunner(Err(LlmRunError::Spawn("boom".to_owned())));

        let err = distill_session(
            &runner,
            vault.path(),
            &session_path,
            None,
            &|_, _| inbox_routing(),
            &no_open_entries,
        )
        .unwrap_err();

        assert!(matches!(err, DistillError::Run(_)));
        assert!(!vault.path().join("Inbox").exists());
    }

    #[test]
    fn malformed_model_output_fails_the_distill_and_writes_nothing() {
        let vault = tempdir().unwrap();
        let session_path = write_session(vault.path(), None);
        let runner = MockRunner(Ok("that meeting was great, no JSON for you".to_owned()));

        let err = distill_session(
            &runner,
            vault.path(),
            &session_path,
            None,
            &|_, _| inbox_routing(),
            &no_open_entries,
        )
        .unwrap_err();

        assert!(matches!(err, DistillError::Parse(_)));
        assert!(!vault.path().join("Inbox").exists());
    }

    #[test]
    fn session_outside_the_vault_is_rejected_before_running() {
        let vault = tempdir().unwrap();
        let elsewhere = tempdir().unwrap();
        let session_path = write_session(elsewhere.path(), None);

        let err = distill_session(
            &PanicRunner,
            vault.path(),
            &session_path,
            None,
            &|_, _| inbox_routing(),
            &no_open_entries,
        )
        .unwrap_err();

        assert!(matches!(err, DistillError::SessionOutsideVault(_)));
    }

    #[test]
    fn missing_session_file_surfaces_as_a_session_error() {
        let vault = tempdir().unwrap();
        let missing = vault.path().join("sessions").join("nope.jsonl");

        let err = distill_session(
            &PanicRunner,
            vault.path(),
            &missing,
            None,
            &|_, _| inbox_routing(),
            &no_open_entries,
        )
        .unwrap_err();

        assert!(matches!(err, DistillError::Session(_)));
    }

    // ---- input budgeting: chunk planning ----------------------------------

    /// The `"You: "`/`"Them: "` prefix plus the trailing newline a rendered
    /// line costs on top of its text.
    const LINE_OVERHEAD: usize = 6;

    /// Plans chunks straight from segments — the production path renders once
    /// up front and hands the lines down, which is a detail these tests don't
    /// need to restate.
    fn plan(segments: &[TranscriptSegment], budget_chars: usize) -> Vec<String> {
        plan_chunk_transcripts(&render_lines(segments), budget_chars)
    }

    /// Runner that replays a scripted sequence of responses and records every
    /// request it was handed, so a chunked run can be asserted on both what it
    /// sent and how many calls it made.
    struct SequenceRunner {
        responses: std::cell::RefCell<std::collections::VecDeque<Result<String, LlmRunError>>>,
        requests: std::cell::RefCell<Vec<LlmRequest>>,
    }

    impl SequenceRunner {
        fn new(responses: Vec<Result<String, LlmRunError>>) -> Self {
            Self {
                responses: std::cell::RefCell::new(responses.into()),
                requests: std::cell::RefCell::new(Vec::new()),
            }
        }

        fn ok(responses: Vec<String>) -> Self {
            Self::new(responses.into_iter().map(Ok).collect())
        }

        fn requests(&self) -> Vec<LlmRequest> {
            self.requests.borrow().clone()
        }
    }

    impl HeadlessClaude for SequenceRunner {
        fn run(&self, request: &LlmRequest) -> Result<String, LlmRunError> {
            self.requests.borrow_mut().push(request.clone());
            self.responses
                .borrow_mut()
                .pop_front()
                .expect("runner called more times than the test scripted")
        }
    }

    /// A distill payload carrying one action item, so a chunk's contribution
    /// stays identifiable through the merge.
    fn chunk_json(summary: &str, description: &str) -> String {
        format!(
            r#"{{"summary": "{summary}", "action_items": [{{"owner": "Jane", "description": "{description}", "due_date": null}}]}}"#
        )
    }

    #[test]
    fn plan_keeps_a_fitting_transcript_in_one_chunk() {
        let segments = vec![
            segment(0, Channel::You, "lets sync on the budget"),
            segment(1, Channel::Them, "will do"),
        ];

        let chunks = plan(&segments, 1_000);

        assert_eq!(chunks, vec![render_transcript(&segments)]);
    }

    #[test]
    fn plan_budget_boundary_is_inclusive() {
        // Two lines of "You: abc\n" — nine characters each.
        let segments = vec![
            segment(0, Channel::You, "abc"),
            segment(1, Channel::You, "def"),
        ];
        let exact = 2 * (3 + LINE_OVERHEAD);

        assert_eq!(plan(&segments, exact).len(), 1);
        assert_eq!(plan(&segments, exact - 1).len(), 2);
    }

    #[test]
    fn plan_splits_on_segment_boundaries_and_loses_nothing() {
        let segments: Vec<_> = (0..20)
            .map(|index| {
                let channel = if index % 2 == 0 {
                    Channel::You
                } else {
                    Channel::Them
                };
                segment(index, channel, &format!("utterance number {index}"))
            })
            .collect();

        let chunks = plan(&segments, 80);

        assert!(chunks.len() > 1, "expected the transcript to split");
        // Nothing is dropped, duplicated, or reordered by the split.
        assert_eq!(chunks.concat(), render_transcript(&segments));
        // Every line in every chunk still carries its channel attribution.
        for chunk in &chunks {
            assert!(chunk.chars().count() <= 80);
            for line in chunk.lines() {
                assert!(
                    line.starts_with("You: ") || line.starts_with("Them: "),
                    "line lost its channel prefix: {line:?}"
                );
            }
        }
    }

    #[test]
    fn plan_counts_characters_not_bytes() {
        // "You: héllo\n" is 11 characters but 12 bytes; two of them fit a
        // 22-character budget and would not fit a byte-counted one.
        let segments = vec![
            segment(0, Channel::You, "héllo"),
            segment(1, Channel::You, "héllo"),
        ];

        assert_eq!(plan(&segments, 22).len(), 1);
    }

    #[test]
    fn an_oversized_segment_is_word_split_with_its_label_recarried() {
        let text = "alpha bravo charlie delta echo foxtrot golf hotel india juliet";
        let segments = vec![segment(0, Channel::Them, text)];

        let chunks = plan(&segments, 30);

        assert!(chunks.len() > 1, "expected the utterance to be split");
        let mut words = Vec::new();
        for chunk in &chunks {
            assert!(chunk.chars().count() <= 30);
            for line in chunk.lines() {
                let rest = line
                    .strip_prefix("Them: ")
                    .expect("every piece re-carries its channel label");
                words.extend(rest.split_whitespace().map(str::to_owned));
            }
        }
        // The utterance survives the split intact, word for word.
        assert_eq!(words.join(" "), text);
    }

    #[test]
    fn an_unbreakable_run_longer_than_the_budget_is_split_on_char_boundaries() {
        let text = "a".repeat(100);
        let segments = vec![segment(0, Channel::You, &text)];

        let chunks = plan(&segments, 30);

        let recovered: String = chunks
            .iter()
            .flat_map(|chunk| chunk.lines())
            .map(|line| line.strip_prefix("You: ").expect("label re-carried"))
            .collect();
        assert_eq!(recovered, text);
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 30));
    }

    // ---- input budgeting: merge -------------------------------------------

    #[test]
    fn both_system_prompts_share_the_response_shape_spec() {
        assert!(RESPONSE_SHAPE_SPEC.starts_with("Respond with ONLY a single JSON object"));
        assert!(RESPONSE_SHAPE_SPEC.contains("\"action_items\""));
        assert!(RESPONSE_SHAPE_SPEC.contains("\"tags\""));
        assert!(RESPONSE_SHAPE_SPEC.contains("\"category\""));
        assert!(system_prompt(&MEETING_FLAVOR).contains(RESPONSE_SHAPE_SPEC));
        assert!(merge_system_prompt(&MEETING_FLAVOR).contains(RESPONSE_SHAPE_SPEC));
    }

    #[test]
    fn a_project_with_no_prior_and_no_corrections_contributes_no_block() {
        assert_eq!(category_context_block(&CategoryFile::default()), None);
    }

    #[test]
    fn the_category_block_carries_the_prior_and_the_corrections() {
        let mut file = CategoryFile::default();
        file.default = Some(MeetingCategory::Client);
        file.upsert(crate::category_examples::CategoryExample {
            note_id: "n_a1b2c3".to_string(),
            title: "Weekly sync with Acme".to_string(),
            excerpt: "Went over the renewal.".to_string(),
            category: MeetingCategory::Client,
            corrected_at: "2026-08-19T12:00:00Z".to_string(),
        });

        let block = category_context_block(&file).expect("a block");

        assert!(block.contains("usually \"client\""), "{block}");
        assert!(
            block.contains("\"Weekly sync with Acme\" -> client"),
            "{block}"
        );
        assert!(
            block.contains("not a rule"),
            "the block must read as guidance: {block}"
        );
    }

    #[test]
    fn the_category_block_shows_only_the_most_recent_examples() {
        let mut file = CategoryFile::default();
        for index in 0..(CATEGORY_CONTEXT_MAX_EXAMPLES + 4) {
            file.upsert(crate::category_examples::CategoryExample {
                note_id: format!("n_{index:06}"),
                title: format!("Meeting {index}"),
                excerpt: "Body prose.".to_string(),
                category: MeetingCategory::Review,
                corrected_at: format!("2026-08-{:02}T12:00:00Z", index + 1),
            });
        }

        let block = category_context_block(&file).expect("a block");

        assert_eq!(
            block.matches(" -> review:").count(),
            CATEGORY_CONTEXT_MAX_EXAMPLES
        );
        // Newest first, so the oldest corrections are the ones left out.
        assert!(block.contains("Meeting 11"), "{block}");
        assert!(!block.contains("Meeting 0\""), "{block}");
    }

    /// The closed set the model is asked to choose from **is**
    /// [`MeetingCategory`], so the two cannot be allowed to drift: renaming a
    /// genre without rewording the prompt would leave the classifier offering a
    /// value the parser then throws away on every meeting, silently.
    #[test]
    fn the_shape_spec_names_every_category() {
        for category in MeetingCategory::ALL {
            assert!(
                RESPONSE_SHAPE_SPEC.contains(category.as_str()),
                "the response shape spec never names {:?}",
                category.as_str()
            );
        }
    }

    /// The [`PromptFlavor`] split is a refactor, not a rewording: the meeting
    /// pass must still assemble the exact prompts it always sent. Pinned here
    /// so a chat-flavored edit that leaks into the shared halves fails loudly.
    #[test]
    fn meeting_flavor_reproduces_the_locked_prompts() {
        assert_eq!(
            system_prompt(&MEETING_FLAVOR),
            format!("{SYSTEM_PROMPT_ROLE} {RESPONSE_SHAPE_SPEC} {DISTILL_REPORTING_RULES}")
        );
        assert_eq!(
            merge_system_prompt(&MEETING_FLAVOR),
            format!(
                "{MERGE_PROMPT_ROLE} {RESPONSE_SHAPE_SPEC} No prose, no markdown fences, no \
explanation - only the JSON object."
            )
        );

        let single = request_from_transcript("You: hello\n", "2026-07-12", &MEETING_FLAVOR, None);
        assert_eq!(
            single.prompt,
            "Meeting date: 2026-07-12\n\nTranscript:\nYou: hello\n"
        );

        let chunk = build_chunk_request("You: hello\n", "2026-07-12", 1, 2, &MEETING_FLAVOR);
        assert_eq!(
            chunk.prompt,
            "Meeting date: 2026-07-12\n\nThis is part 1 of 2 of one continuous meeting \
transcript, split only for length; the other parts are distilled separately and merged \
afterward. Report only what appears in this part.\n\nTranscript (part 1 of 2):\nYou: hello\n"
        );

        let merge = build_merge_request(
            &[distill_output(None, "First half.", &[])],
            "2026-07-12",
            &MEETING_FLAVOR,
        )
        .unwrap();
        assert!(merge.prompt.starts_with(
            "Meeting date: 2026-07-12\n\nThe meeting's transcript was distilled in 1 \
consecutive parts. The partial results, in order:\n"
        ));
    }

    /// A long meeting is exactly where the classification matters most, so each
    /// part's genre has to reach the merge call — unlike `ledger_updates`, which
    /// the chunked path clears because no chunk was ever shown the entry ids
    /// those name. Here the merging model weighs the parts and returns one
    /// answer for the whole conversation.
    #[test]
    fn each_chunks_category_reaches_the_merge_payload() {
        let mut first = distill_output(None, "First half.", &[]);
        first.category = Some(MeetingCategory::Standup);
        first.category_confidence = Some(0.4);
        let mut second = distill_output(None, "Second half.", &[]);
        second.category = Some(MeetingCategory::WorkingSession);
        second.category_confidence = Some(0.9);

        let merge = build_merge_request(&[first, second], "2026-07-12", &MEETING_FLAVOR).unwrap();

        assert!(
            merge.prompt.contains("\"category\":\"standup\""),
            "{}",
            merge.prompt
        );
        assert!(
            merge.prompt.contains("\"category\":\"working-session\""),
            "{}",
            merge.prompt
        );
        assert!(
            merge.prompt.contains("\"category_confidence\":0.9"),
            "{}",
            merge.prompt
        );
        // And the merge prompt still demands the full shape, so the call can
        // answer with one category for the whole meeting.
        assert!(merge_system_prompt(&MEETING_FLAVOR).contains("\"category\""));
    }

    #[test]
    fn an_unclassified_chunk_contributes_no_category_keys() {
        let merge = build_merge_request(
            &[distill_output(None, "First half.", &[])],
            "2026-07-12",
            &MEETING_FLAVOR,
        )
        .unwrap();

        assert!(!merge.prompt.contains("category"), "{}", merge.prompt);
    }

    // ------------------------------------------------------------------
    // Ledger classifications
    // ------------------------------------------------------------------

    fn open_commitment(entry_id: &str, description: &str) -> OpenCommitment {
        OpenCommitment {
            entry_id: entry_id.to_string(),
            owner: "You".to_string(),
            description: description.to_string(),
        }
    }

    #[test]
    fn ledger_updates_parse_with_their_defaults() {
        let output = parse_output(
            r#"{"summary": "Talked it through.",
                "ledger_updates": [
                  {"entry": "le_aaa", "kind": "refresh"},
                  {"entry": "le_bbb", "kind": "Completed", "confidence": 0.91,
                   "quote": "sent it over  Tuesday"},
                  {"entry": "le_ccc", "kind": "completed", "confidence": 5.0}
                ]}"#,
        )
        .unwrap();

        let updates = &output.ledger_updates;
        assert_eq!(updates.len(), 3);

        // No confidence stated: parks rather than closing, whatever the
        // threshold is.
        assert_eq!(updates[0].kind, LedgerUpdateKind::Refresh);
        assert_eq!(updates[0].confidence, UNSTATED_CONFIDENCE);
        assert_eq!(updates[0].item, None);
        assert_eq!(updates[0].quote, None);

        // Spelling is the model's, meaning is ours; the quote is collapsed
        // like every other one-line field.
        assert_eq!(updates[1].kind, LedgerUpdateKind::Completed);
        assert_eq!(updates[1].confidence, 0.91);
        assert_eq!(updates[1].quote.as_deref(), Some("sent it over Tuesday"));

        // Out of range clamps rather than dropping: the model meant "very
        // sure", and 5.0 is not a different claim.
        assert_eq!(updates[2].confidence, 1.0);
    }

    #[test]
    fn an_unusable_ledger_update_is_dropped_not_fatal() {
        let output = parse_output(
            r#"{"summary": "Talked it through.",
                "ledger_updates": [
                  {"entry": "", "kind": "refresh"},
                  {"entry": "le_ok", "kind": "reconsidered"},
                  {"entry": "le_supersede", "kind": "supersede"},
                  {"entry": "le_keep", "kind": "refresh", "confidence": 0.4}
                ]}"#,
        )
        .unwrap();

        // An empty id, an unknown kind, and a supersede with nothing to
        // supersede it: all gone, and the usable one survives beside them.
        let updates = &output.ledger_updates;
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].entry_id, "le_keep");
    }

    #[test]
    fn item_indexes_survive_a_dropped_action_item() {
        // The second item normalizes away (no description), so what the model
        // called index 2 is index 1 by the time anything reads it. A naive
        // pass-through would point this update at the wrong commitment.
        let output = parse_output(
            r#"{"summary": "Talked it through.",
                "action_items": [
                  {"description": "send the deck", "owner": "You"},
                  {"description": "   ", "owner": "You"},
                  {"description": "book the venue", "owner": "Priya"}
                ],
                "ledger_updates": [
                  {"entry": "le_venue", "kind": "supersede", "item": 2},
                  {"entry": "le_gone", "kind": "refresh", "item": 1}
                ]}"#,
        )
        .unwrap();

        assert_eq!(output.action_items.len(), 2);
        let venue = &output.ledger_updates[0];
        assert_eq!(venue.item, Some(1));
        assert_eq!(
            output.action_items[venue.item.unwrap()].description,
            "book the venue"
        );

        // The refresh named an item that no longer exists. The entry is still
        // the point, so it survives as a bare re-mention.
        let gone = &output.ledger_updates[1];
        assert_eq!(gone.entry_id, "le_gone");
        assert_eq!(gone.item, None);
    }

    #[test]
    fn an_absent_or_unrecognized_firmness_is_firm() {
        // Absent (every response written before the field existed), empty,
        // and a value no one anticipated. All three enroll, because the cost
        // of guessing wrong the other way is a dropped commitment.
        let output = parse_output(
            r#"{"summary": "Talked it through.",
                "action_items": [
                  {"description": "send the deck", "owner": "You"},
                  {"description": "book the venue", "owner": "You", "firmness": ""},
                  {"description": "call the vendor", "owner": "You", "firmness": "maybe"},
                  {"description": "look into pricing", "owner": "You", "firmness": "SOFT"},
                  {"description": "renew the domain", "owner": "You", "firmness": " soft "}
                ]}"#,
        )
        .unwrap();

        let firmness: Vec<bool> = output.action_items.iter().map(|item| item.firm).collect();
        assert_eq!(firmness, vec![true, true, true, false, false]);
    }

    #[test]
    fn a_soft_item_renders_with_the_marker_and_round_trips() {
        let soft = soft_draft("look into pricing", Some("Dana"), None);
        let dated = soft_draft("draft the brief", Some("Dana"), Some("2026-08-29"));

        assert_eq!(
            render_action_item(&soft),
            "Dana to look into pricing. (tentative)"
        );
        assert_eq!(
            render_action_item(&dated),
            "Dana to draft the brief by 2026-08-29. (tentative)"
        );

        for item in [&soft, &dated] {
            let line = format!("- [ ] {}", render_action_item(item));
            let (owner, description, due, firm) = parse_action_line(&line).expect("parses back");
            assert_eq!(owner, "Dana");
            assert_eq!(description, item.description);
            assert_eq!(due, item.due_date);
            assert!(!firm, "the marker survives the round trip: {line}");
        }
    }

    /// The marker is the only thing firmness adds to a line, so a firm item
    /// must render exactly what it rendered before firmness existed. Every
    /// action-item id in every vault in the field depends on this.
    #[test]
    fn a_firm_item_renders_exactly_as_it_always_did() {
        assert_eq!(
            render_action_item(&draft("send the memo", Some("Jane"), Some("2026-07-15"))),
            "Jane to send the memo by 2026-07-15."
        );
        assert_eq!(
            render_action_item(&draft("book the venue", None, None)),
            format!("{UNASSIGNED_OWNER} to book the venue.")
        );
    }

    /// A description that legitimately ends in the marker's own words must not
    /// be misread as soft — the marker is only the tail *after* the period.
    #[test]
    fn a_description_ending_in_the_marker_words_is_still_firm() {
        let line = "- [ ] Dana to mark the plan (tentative).";
        let (_, description, _, firm) = parse_action_line(line).expect("parses");
        assert_eq!(description, "mark the plan (tentative)");
        assert!(firm);
    }

    #[test]
    fn the_context_block_lists_what_the_model_may_refer_to() {
        let block = ledger_context_block(&[
            open_commitment("le_aaa", "send the signed budget memo to finance"),
            open_commitment("le_bbb", "share the survey results"),
        ])
        .unwrap();

        assert_eq!(
            block,
            "Open commitments already recorded for this project:\n[{\"entry_id\":\"le_aaa\",\"owner\":\"You\",\"description\":\"send the signed budget memo to finance\"},{\"entry_id\":\"le_bbb\",\"owner\":\"You\",\"description\":\"share the survey results\"}]"
        );

        // Nothing open, nothing said: the prompt stays exactly what it was.
        assert!(ledger_context_block(&[]).is_none());
    }

    #[test]
    fn the_context_block_is_bounded_by_count_and_by_length() {
        let many: Vec<OpenCommitment> = (0..60)
            .map(|i| open_commitment(&format!("le_{i:04}"), "send the deck"))
            .collect();
        let block = ledger_context_block(&many).unwrap();
        assert_eq!(
            block.matches("entry_id").count(),
            LEDGER_CONTEXT_MAX_ENTRIES
        );

        // A handful of very long descriptions hits the character ceiling well
        // before the entry ceiling.
        let long = "x".repeat(4_000);
        let bulky: Vec<OpenCommitment> = (0..10)
            .map(|i| open_commitment(&format!("le_{i:04}"), &long))
            .collect();
        let block = ledger_context_block(&bulky).unwrap();
        assert!(block.chars().count() <= LEDGER_CONTEXT_MAX_CHARS + 200);
        assert!(block.matches("entry_id").count() < 10);
    }

    #[test]
    fn a_prompt_with_commitments_keeps_the_transcript_last() {
        let block = ledger_context_block(&[open_commitment("le_aaa", "send the deck")]).unwrap();
        let request =
            request_from_transcript("You: hello\n", "2026-07-12", &MEETING_FLAVOR, Some(&block));

        assert_eq!(
            request.prompt,
            "Meeting date: 2026-07-12\n\nOpen commitments already recorded for this project:\n[{\"entry_id\":\"le_aaa\",\"owner\":\"You\",\"description\":\"send the deck\"}]\n\nTranscript:\nYou: hello\n"
        );
    }

    #[test]
    fn the_identity_block_names_the_user_without_changing_the_owner_spelling() {
        let block = identity_context_block(&crate::settings::IdentitySettings {
            display_name: "Avery".to_string(),
            aliases: vec!["Avery Kim".to_string()],
        })
        .unwrap();

        assert_eq!(
            block,
            "The local user - the \"You\" channel - is \"Avery\", \"Avery Kim\". \
A commitment that person takes on is owned by \"You\", however the transcript \
happens to name them and whoever said it out loud. A first-person commitment on \
a \"Them\" line belongs to that speaker: use their name when the conversation \
gives one, otherwise \"Them\"."
        );

        // It reinforces the shared contract rather than competing with it.
        assert!(RESPONSE_SHAPE_SPEC.contains(r#"\"You\" when the local user took it on"#));
    }

    #[test]
    fn no_configured_name_means_no_identity_block() {
        assert_eq!(
            identity_context_block(&crate::settings::IdentitySettings::default()),
            None
        );
        // Whitespace is not an answer either.
        assert_eq!(
            identity_context_block(&crate::settings::IdentitySettings {
                display_name: "   ".to_string(),
                aliases: Vec::new(),
            }),
            None
        );
    }

    #[test]
    fn the_identity_block_survives_an_identity_that_is_only_aliases() {
        let block = identity_context_block(&crate::settings::IdentitySettings {
            display_name: String::new(),
            aliases: vec!["Avery Kim".to_string()],
        })
        .unwrap();
        assert!(block.starts_with("The local user - the \"You\" channel - is \"Avery Kim\"."));
    }

    #[test]
    fn a_prompt_with_an_identity_keeps_the_transcript_last() {
        let identity = identity_context_block(&crate::settings::IdentitySettings {
            display_name: "Avery".to_string(),
            aliases: Vec::new(),
        })
        .unwrap();
        let ledger = ledger_context_block(&[open_commitment("le_aaa", "send the deck")]).unwrap();
        let request = request_from_transcript(
            "You: hello\n",
            "2026-07-12",
            &MEETING_FLAVOR,
            Some(&[identity.as_str(), ledger.as_str()].join("\n\n")),
        );

        // Identity first, then the commitments, and the transcript still last.
        let identity_at = request.prompt.find("The local user").unwrap();
        let ledger_at = request.prompt.find("Open commitments").unwrap();
        let transcript_at = request.prompt.find("Transcript:").unwrap();
        assert!(identity_at < ledger_at, "{}", request.prompt);
        assert!(ledger_at < transcript_at, "{}", request.prompt);
        assert!(request.prompt.ends_with("Transcript:\nYou: hello\n"));
    }

    #[test]
    fn a_distill_only_reports_on_commitments_it_was_shown() {
        let output_json = r#"{"summary": "Talked it through.",
            "ledger_updates": [
              {"entry": "le_real", "kind": "refresh"},
              {"entry": "le_invented", "kind": "completed", "confidence": 0.99}
            ]}"#;
        let vault = tempdir().unwrap();
        routing_fixture(vault.path());
        let session_path = write_session_with(
            vault.path(),
            &[
                segment(0, Channel::You, "the tee sheet and irrigation work"),
                segment(
                    1,
                    Channel::Them,
                    "GreenFlow and TeeTrack both need MERIDIAN",
                ),
            ],
        );
        let runner = MockRunner(Ok(output_json.to_string()));

        let distilled = distill_session(
            &runner,
            vault.path(),
            &session_path,
            None,
            &|_, _| inbox_routing(),
            &|guess| {
                // The guess is what decides whose commitments are worth
                // showing, and it is made before the model has said anything.
                assert_eq!(guess.project, "Briarwood Golf");
                vec![open_commitment("le_real", "send the deck")]
            },
        )
        .unwrap();

        // An id the model invented cannot reach anything that would look it
        // up: it never appeared in the list the model was given.
        assert_eq!(distilled.ledger_updates.len(), 1);
        assert_eq!(distilled.ledger_updates[0].entry_id, "le_real");
    }

    /// The assembled distill prompt is one flowing paragraph, not three parts
    /// with a seam: the split exists to share the shape spec, not to change
    /// what the model reads.
    #[test]
    fn the_assembled_system_prompt_reads_as_one_prompt() {
        let prompt = system_prompt(&MEETING_FLAVOR);

        assert!(prompt.starts_with("You are a meeting-notes distiller."));
        assert!(prompt.contains("unattributed audio. Respond with ONLY"));
        assert!(prompt.contains("ledger_updates must be empty. Only report decisions"));
        assert!(prompt.ends_with("only the JSON object."));
        assert!(!prompt.contains("  "), "no doubled space at a seam");
    }

    #[test]
    fn exact_duplicates_dedup_across_chunks_first_wins() {
        let repeated = draft("send the memo", Some("Jane"), None);
        let similar = draft("send the memo", Some("Ada"), None);
        let first = DistillOutput {
            action_items: vec![repeated.clone()],
            open_questions: vec!["Who owns outreach?".into()],
            ..distill_output(None, "First half.", &["Approve the budget."])
        };
        let second = DistillOutput {
            action_items: vec![repeated.clone(), similar.clone()],
            open_questions: vec!["Who owns outreach?".into()],
            ..distill_output(
                None,
                "Second half.",
                &["Approve the budget.", "Ship on Friday."],
            )
        };

        let merged = dedup_across_chunks(vec![first, second]);

        assert_eq!(merged[0].decisions, vec!["Approve the budget."]);
        assert_eq!(merged[0].action_items, vec![repeated]);
        assert_eq!(merged[0].open_questions, vec!["Who owns outreach?"]);
        // The repeat is gone; a different owner is a different commitment.
        assert_eq!(merged[1].decisions, vec!["Ship on Friday."]);
        assert_eq!(merged[1].action_items, vec![similar]);
        assert!(merged[1].open_questions.is_empty());
    }

    /// Budget whose line allowance (80 characters) splits
    /// [`two_chunk_segments`] into exactly two chunks.
    const TWO_CHUNK_BUDGET: usize = CHUNK_PROMPT_OVERHEAD_CHARS + 80;

    /// Four 24-character utterances: two chunks at [`TWO_CHUNK_BUDGET`].
    fn two_chunk_segments() -> Vec<TranscriptSegment> {
        (0..4)
            .map(|index| segment(index, Channel::You, &format!("utterance number {index}")))
            .collect()
    }

    /// Runs the chunked path from segments, mirroring [`plan`].
    fn chunked(
        runner: &dyn HeadlessClaude,
        segments: &[TranscriptSegment],
        meeting_date: &str,
        budget_chars: usize,
    ) -> Result<DistillOutput, DistillError> {
        distill_chunked(
            runner,
            &render_lines(segments),
            meeting_date,
            budget_chars,
            &MEETING_FLAVOR,
        )
    }

    #[test]
    fn chunked_distill_sends_part_framed_requests_then_one_merge() {
        let segments = two_chunk_segments();
        assert_eq!(
            plan(&segments, TWO_CHUNK_BUDGET - CHUNK_PROMPT_OVERHEAD_CHARS).len(),
            2,
            "the fixture should split into two chunks"
        );
        let runner = SequenceRunner::ok(vec![
            chunk_json("First half.", "send the memo"),
            chunk_json("Second half.", "book the room"),
            chunk_json("The whole meeting.", "send the memo"),
        ]);

        let output = chunked(
            &runner,
            &segments,
            "2026-07-12",
            CHUNK_PROMPT_OVERHEAD_CHARS + 80,
        )
        .unwrap();

        let requests = runner.requests();
        assert_eq!(requests.len(), 3, "two chunks plus one merge");
        for (index, request) in requests[..2].iter().enumerate() {
            assert_eq!(request.system_prompt, system_prompt(&MEETING_FLAVOR));
            assert!(request
                .prompt
                .contains(&format!("This is part {} of 2", index + 1)));
        }
        assert_eq!(
            requests[2].system_prompt,
            merge_system_prompt(&MEETING_FLAVOR)
        );
        assert!(requests[2]
            .prompt
            .contains("distilled in 2 consecutive parts"));
        assert_eq!(output.summary, "The whole meeting.");
    }

    #[test]
    fn duplicate_action_items_reach_the_merge_request_once() {
        let segments = two_chunk_segments();
        // Both chunks report the same commitment verbatim.
        let runner = SequenceRunner::ok(vec![
            chunk_json("First half.", "send the memo"),
            chunk_json("Second half.", "send the memo"),
            chunk_json("The whole meeting.", "send the memo"),
        ]);

        chunked(&runner, &segments, "2026-07-12", TWO_CHUNK_BUDGET).unwrap();

        let merge_prompt = runner.requests()[2].prompt.clone();
        assert_eq!(merge_prompt.matches("send the memo").count(), 1);
    }

    /// The same commitment can read as a firm promise in the chunk that caught
    /// it and as idle musing in the chunk that caught someone repeating it.
    /// Dedup must not let chunk order decide whether it enrolls, so the
    /// survivor is firm whenever any occurrence was — and the merge model is
    /// told so, since the merge is where firmness would otherwise be re-guessed
    /// from the part summaries alone.
    #[test]
    fn a_duplicate_that_is_firm_in_any_chunk_survives_as_firm() {
        let soft_then_firm = dedup_across_chunks(vec![
            part_with(vec![soft_draft("send the memo", Some("Jane"), None)]),
            part_with(vec![draft("send the memo", Some("Jane"), None)]),
        ]);
        let kept: Vec<&ActionItemDraft> = soft_then_firm
            .iter()
            .flat_map(|part| part.action_items.iter())
            .collect();
        assert_eq!(kept.len(), 1, "the duplicate is still deduped");
        assert!(
            kept[0].firm,
            "the firm reading wins even when it came second"
        );

        // Soft everywhere stays soft — firm-anywhere-wins is not firm-always.
        let soft_twice = dedup_across_chunks(vec![
            part_with(vec![soft_draft("look into pricing", Some("Jane"), None)]),
            part_with(vec![soft_draft("look into pricing", Some("Jane"), None)]),
        ]);
        let kept: Vec<&ActionItemDraft> = soft_twice
            .iter()
            .flat_map(|part| part.action_items.iter())
            .collect();
        assert_eq!(kept.len(), 1);
        assert!(!kept[0].firm);
    }

    #[test]
    fn firmness_reaches_the_merge_request() {
        let parts = vec![part_with(vec![
            draft("send the memo", Some("Jane"), None),
            soft_draft("look into pricing", Some("Jane"), None),
        ])];

        let request = build_merge_request(&parts, "2026-07-12", &MEETING_FLAVOR).unwrap();

        assert!(
            request
                .prompt
                .contains(r#""description":"send the memo","due_date":null,"firmness":"firm""#),
            "firm item carries its classification into the merge: {}",
            request.prompt
        );
        assert!(
            request
                .prompt
                .contains(r#""description":"look into pricing","due_date":null,"firmness":"soft""#),
            "soft item carries its classification into the merge: {}",
            request.prompt
        );
    }

    #[test]
    fn a_mid_sequence_chunk_failure_aborts_before_further_calls() {
        let segments = two_chunk_segments();
        let runner = SequenceRunner::new(vec![
            Ok(chunk_json("First half.", "send the memo")),
            Err(LlmRunError::EmptyResult),
        ]);

        let err = chunked(
            &runner,
            &segments,
            "2026-07-12",
            CHUNK_PROMPT_OVERHEAD_CHARS + 80,
        )
        .unwrap_err();

        assert!(matches!(err, DistillError::Run(_)));
        // The merge was never attempted.
        assert_eq!(runner.requests().len(), 2);
    }

    #[test]
    fn a_chunk_parse_failure_aborts_the_run() {
        let segments = two_chunk_segments();
        let runner = SequenceRunner::ok(vec!["not json at all".to_string()]);

        let err = chunked(
            &runner,
            &segments,
            "2026-07-12",
            CHUNK_PROMPT_OVERHEAD_CHARS + 80,
        )
        .unwrap_err();

        assert!(matches!(err, DistillError::Parse(_)));
        assert_eq!(runner.requests().len(), 1);
    }

    #[test]
    fn a_merge_parse_failure_aborts_the_run() {
        let segments = two_chunk_segments();
        let runner = SequenceRunner::ok(vec![
            chunk_json("First half.", "send the memo"),
            chunk_json("Second half.", "book the room"),
            "sorry, I can't do that".to_string(),
        ]);

        let err = chunked(
            &runner,
            &segments,
            "2026-07-12",
            CHUNK_PROMPT_OVERHEAD_CHARS + 80,
        )
        .unwrap_err();

        assert!(matches!(err, DistillError::Parse(_)));
    }

    // ---- input budgeting: the bounds on the chunked path -------------------

    #[test]
    fn a_max_size_chunk_request_fits_the_reserved_framing_overhead() {
        // The worst case the planner can hand `build_chunk_request`: a chunk
        // packed to the last character of its allowance, framed with the widest
        // part numbering the chunk cap allows.
        let chunk = "a".repeat(DISTILL_INPUT_BUDGET_CHARS - CHUNK_PROMPT_OVERHEAD_CHARS);
        let request = build_chunk_request(
            &chunk,
            "2026-07-12",
            MAX_DISTILL_CHUNKS,
            MAX_DISTILL_CHUNKS,
            &MEETING_FLAVOR,
        );

        assert!(
            request.prompt.chars().count() <= DISTILL_INPUT_BUDGET_CHARS,
            "chunk framing outgrew CHUNK_PROMPT_OVERHEAD_CHARS: prompt is {} characters",
            request.prompt.chars().count()
        );
    }

    #[test]
    fn a_transcript_past_the_chunk_cap_fails_before_any_call() {
        // One chunk's worth of text per segment, one more segment than the cap.
        let chunk_chars = TWO_CHUNK_BUDGET - CHUNK_PROMPT_OVERHEAD_CHARS;
        let segments: Vec<_> = (0..MAX_DISTILL_CHUNKS + 1)
            .map(|index| {
                segment(
                    index as u64,
                    Channel::You,
                    &"a".repeat(chunk_chars - LINE_OVERHEAD),
                )
            })
            .collect();
        let runner = SequenceRunner::ok(Vec::new());

        let err = chunked(&runner, &segments, "2026-07-12", TWO_CHUNK_BUDGET).unwrap_err();

        assert!(matches!(
            err,
            DistillError::TranscriptTooLong {
                chunks,
                max: MAX_DISTILL_CHUNKS,
            } if chunks == MAX_DISTILL_CHUNKS + 1
        ));
        // Nothing was spent finding that out.
        assert!(runner.requests().is_empty());
    }

    #[test]
    fn an_empty_chunk_is_dropped_rather_than_failing_the_meeting() {
        let segments = two_chunk_segments();
        let runner = SequenceRunner::ok(vec![
            // A stretch of dead air: well-formed JSON, nothing in it.
            r#"{"summary": "", "decisions": [], "action_items": [], "open_questions": []}"#
                .to_string(),
            chunk_json("The second half.", "send the memo"),
        ]);

        let output = chunked(&runner, &segments, "2026-07-12", TWO_CHUNK_BUDGET).unwrap();

        // One part survived, so it is the result — no merge call was needed.
        assert_eq!(runner.requests().len(), 2);
        assert_eq!(output.summary, "The second half.");
    }

    #[test]
    fn a_chunk_with_items_but_no_summary_still_reaches_the_merge() {
        let segments = two_chunk_segments();
        let runner = SequenceRunner::ok(vec![
            // No summary, but a real commitment: the merge writes the prose.
            r#"{"summary": "", "action_items": [{"owner": "Jane", "description": "file the permits", "due_date": null}]}"#
                .to_string(),
            chunk_json("The second half.", "send the memo"),
            chunk_json("The whole meeting.", "file the permits"),
        ]);

        chunked(&runner, &segments, "2026-07-12", TWO_CHUNK_BUDGET).unwrap();

        let merge_prompt = runner.requests()[2].prompt.clone();
        assert!(merge_prompt.contains("file the permits"));
    }

    #[test]
    fn every_chunk_coming_back_empty_writes_nothing() {
        let segments = two_chunk_segments();
        let empty = r#"{"summary": ""}"#.to_string();
        let runner = SequenceRunner::ok(vec![empty.clone(), empty]);

        let err = chunked(&runner, &segments, "2026-07-12", TWO_CHUNK_BUDGET).unwrap_err();

        assert!(matches!(err, DistillError::Parse(_)));
    }

    #[test]
    fn an_over_budget_merge_is_batched_into_rounds_instead_of_overflowing() {
        // Four chunks whose four results cannot share one merge prompt, so the
        // reduce step has to happen in rounds.
        let budget = CHUNK_PROMPT_OVERHEAD_CHARS + 88;
        let segments: Vec<_> = (0..12)
            .map(|index| segment(index, Channel::You, &format!("utterance number {index}")))
            .collect();
        assert_eq!(
            plan(&segments, 88).len(),
            4,
            "fixture should be four chunks"
        );

        let mut responses: Vec<String> = (0..4)
            .map(|part| chunk_json(&format!("Part {part}."), &format!("review section {part}")))
            .collect();
        // Two pairwise merges, then one merge of those two results.
        responses.push(chunk_json("First half.", "review section 0"));
        responses.push(chunk_json("Second half.", "review section 2"));
        responses.push(chunk_json("The whole meeting.", "review the filing"));
        let runner = SequenceRunner::ok(responses);

        let output = chunked(&runner, &segments, "2026-07-12", budget).unwrap();

        let requests = runner.requests();
        assert_eq!(requests.len(), 7, "four chunks plus three merge rounds");
        // Every merge prompt respects the budget the single-call path enforces.
        for request in &requests[4..] {
            assert!(
                request.prompt.chars().count() <= budget,
                "a merge prompt went over budget: {} characters",
                request.prompt.chars().count()
            );
        }
        assert_eq!(output.summary, "The whole meeting.");
    }

    // ---- input budgeting: the session path --------------------------------

    /// Writes `segments` as a session into `<vault>/sessions/`.
    fn write_session_with(vault: &Path, segments: &[TranscriptSegment]) -> PathBuf {
        raw_session::write_raw_session(
            &vault.join("sessions"),
            instant(),
            &device(),
            None,
            segments,
        )
        .unwrap()
    }

    #[test]
    fn an_under_budget_session_sends_exactly_the_single_call_request() {
        let vault = tempdir().unwrap();
        let session_path = write_session(vault.path(), None);
        let segments = raw_session::read_raw_session(&session_path).unwrap();
        let runner = SequenceRunner::ok(vec![full_output_json()]);

        distill_session(
            &runner,
            vault.path(),
            &session_path,
            None,
            &|_, _| inbox_routing(),
            &no_open_entries,
        )
        .unwrap();

        // Byte-for-byte the request this pass has always sent, and only one.
        // The meeting date is the capture instant's *local* day, so derive it
        // rather than pinning a literal that only holds west of the dateline.
        assert_eq!(
            runner.requests(),
            vec![build_request(&segments, &local_day())]
        );
    }

    #[test]
    fn a_session_exactly_at_the_budget_stays_single_call() {
        let vault = tempdir().unwrap();
        // "Meeting date: 2026-07-12\n\nTranscript:\n" is 38 characters; the
        // one rendered line must bring the prompt to exactly the budget. Every
        // `%Y-%m-%d` day is 10 characters, so the local day keeps that count.
        let preamble = format!("Meeting date: {}\n\nTranscript:\n", local_day())
            .chars()
            .count();
        let text = "a".repeat(DISTILL_INPUT_BUDGET_CHARS - preamble - LINE_OVERHEAD);
        let segments = vec![segment(0, Channel::You, &text)];
        let session_path = write_session_with(vault.path(), &segments);
        let runner = SequenceRunner::ok(vec![full_output_json()]);

        let request = build_request(&segments, &local_day());
        assert_eq!(request.prompt.chars().count(), DISTILL_INPUT_BUDGET_CHARS);

        distill_session(
            &runner,
            vault.path(),
            &session_path,
            None,
            &|_, _| inbox_routing(),
            &no_open_entries,
        )
        .unwrap();

        assert_eq!(runner.requests(), vec![request]);
    }

    #[test]
    fn a_long_session_distills_into_one_well_formed_note() {
        let vault = tempdir().unwrap();
        // ~3 hours of two-channel conversation, well past the budget.
        let segments: Vec<_> = (0..4_000)
            .map(|index| {
                let channel = if index % 2 == 0 {
                    Channel::You
                } else {
                    Channel::Them
                };
                segment(
                    index,
                    channel,
                    &format!("utterance number {index} about the permits and the budget schedule"),
                )
            })
            .collect();
        assert!(
            build_request(&segments, "2026-07-12")
                .prompt
                .chars()
                .count()
                > DISTILL_INPUT_BUDGET_CHARS
        );
        let session_path = write_session_with(vault.path(), &segments);

        let chunk_count = plan(
            &segments,
            DISTILL_INPUT_BUDGET_CHARS - CHUNK_PROMPT_OVERHEAD_CHARS,
        )
        .len();
        assert!(chunk_count > 1, "expected the transcript to chunk");

        // Every chunk reports the same early commitment; only the last one
        // raises the permits, so a merge that drops late chunks fails here.
        let mut responses: Vec<String> = (0..chunk_count)
            .map(|index| {
                if index == chunk_count - 1 {
                    chunk_json("The closing stretch.", "file the permits")
                } else {
                    chunk_json("An earlier stretch.", "send the memo")
                }
            })
            .collect();
        responses.push(
            r#"{
                "title": "Permits and budget",
                "summary": "A long meeting about permits and the budget schedule.",
                "decisions": ["Approve the revised schedule"],
                "action_items": [
                    {"owner": "Jane", "description": "send the memo", "due_date": null},
                    {"owner": "Jane", "description": "file the permits", "due_date": "2026-07-20"}
                ],
                "open_questions": [],
                "tags": ["permits"]
            }"#
            .to_string(),
        );
        let runner = SequenceRunner::ok(responses);

        let distilled = distill_session(
            &runner,
            vault.path(),
            &session_path,
            None,
            &|_, _| inbox_routing(),
            &no_open_entries,
        )
        .unwrap();

        assert_eq!(runner.requests().len(), chunk_count + 1);
        let written = std::fs::read_to_string(&distilled.path).unwrap();
        // One well-formed note, carrying the late chunk's action item.
        assert!(written.contains("# Summary"));
        assert!(written.contains("- [ ] Jane to send the memo."));
        assert!(written.contains("- [ ] Jane to file the permits by 2026-07-20."));
        assert!(written.contains("- Approve the revised schedule."));
    }

    #[test]
    fn a_chunk_failure_writes_no_note_and_keeps_the_session() {
        let vault = tempdir().unwrap();
        let long_text = "a".repeat(DISTILL_INPUT_BUDGET_CHARS);
        let segments = vec![
            segment(0, Channel::You, &long_text),
            segment(1, Channel::Them, "and that's the plan"),
        ];
        let session_path = write_session_with(vault.path(), &segments);
        let runner = SequenceRunner::new(vec![
            Ok(chunk_json("First half.", "send the memo")),
            Err(LlmRunError::ClaudeError("overloaded".into())),
        ]);

        let err = distill_session(
            &runner,
            vault.path(),
            &session_path,
            None,
            &|_, _| inbox_routing(),
            &no_open_entries,
        )
        .unwrap_err();

        assert!(matches!(err, DistillError::Run(_)));
        assert!(!vault.path().join("Inbox").exists());
        // The raw session survives, so the distill stays retryable.
        assert!(session_path.exists());
    }
}
