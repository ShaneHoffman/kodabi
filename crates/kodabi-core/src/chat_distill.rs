//! Chat-sessions-as-documents: turns a persisted chat transcript into a
//! distilled `type: chat` note (FOUNDING_DOC §3.6, "chats are documents too").
//!
//! The sibling of [`crate::distill`], not a fork of it. Everything from the
//! model call down — budget check, map-reduce, body rendering, routing, the
//! note write — is [`distill::distill_rendered`], shared verbatim, so a chat
//! note and a meeting note can never drift on the frontmatter contract or the
//! fail-hard rule. What differs is only what *has* to: the artifact being read
//! ([`chat::read_transcript`] rather than a raw session), the prompt copy
//! ([`CHAT_FLAVOR`]), and the note's type and source.
//!
//! # What reaches the model
//!
//! The distill prompt is the one place a chat's content leaves the machine, so
//! the invariant is narrow and deliberate: **only what the user saw in the chat
//! view goes into the prompt.** User and assistant turns go in as prose; a tool
//! call contributes its human-readable [`chat::tool_use_summary`] — the same
//! string the view rendered — and never its raw `input`, which is arbitrary
//! JSON the user may never have reviewed. Permission and error records are
//! dropped entirely: a denied write and a failed turn are non-events, and
//! feeding them in only invites the model to summarize an outage.
//!
//! # Why a chat is distilled differently
//!
//! A meeting is action-item-heavy; a chat is usually thinking out loud, where
//! the knowledge is the answers reached and the questions left open. The
//! response *shape* is identical (that contract is
//! [`distill::RESPONSE_SHAPE_SPEC`], shared by every pass); only the reporting
//! emphasis changes.

use std::path::Path;

use chrono::{DateTime, Local, Utc};

use crate::chat::{self, ChatRecord};
use crate::distill::{
    self, distill_rendered, DistillError, DistillOutput, DistilledNote, PromptFlavor, RenderedLine,
};
use crate::llm::HeadlessClaude;
use crate::naming;
use crate::note::{NoteType, Routing, Source};

/// Prompt prefix for the local user's turns. Deliberately the same token the
/// meeting pass uses for the local speaker, so "You" means one thing.
const USER_LABEL: &str = "You";

/// Prompt prefix for the assistant's turns. Named rather than reusing the
/// meeting pass's "Them" so its "the other participant(s)" semantics can't
/// leak into what is a two-party human/assistant exchange.
const ASSISTANT_LABEL: &str = "Claude";

/// Prompt prefix for a tool call, carrying only its summary.
const TOOL_LABEL: &str = "Tool";

/// The chat pass's prompt copy. Same JSON contract as the meeting pass, a
/// different reader and a different reporting emphasis.
pub(crate) const CHAT_FLAVOR: PromptFlavor = PromptFlavor {
    role: "You are a chat-notes distiller. You will be given the date of a chat between the local \
user and Kodabi's assistant (Claude Code, grounded in the user's own knowledge base) and the \
conversation, one line per turn, each prefixed with its speaker: \"You\" is the local user, \
\"Claude\" is the assistant, and \"Tool\" lines record what the assistant looked up. Distill what \
the conversation established, not how it was conducted.",
    rules: "Report the answers and conclusions the conversation actually reached, and the \
questions it left open. A chat is usually thinking out loud, so decisions and action items are \
often empty and that is the correct answer. Only report an action item the user actually \
committed to. Never invent owners, dates, decisions, or facts not present in the conversation, \
and never report on the assistant's tool use as if it were content. No prose, no markdown \
fences, no explanation - only the JSON object.",
    merge_role: "You are a chat-notes distiller. One chat conversation was distilled in \
consecutive parts; you will be given the chat's date and those partial results, in order, as a \
JSON array. Merge them into ONE result for the whole conversation: write a single unified summary \
(not a list of per-part recaps), keep every distinct decision and action item exactly once (two \
entries are the same only when they describe the same commitment), keep only the open questions no \
later part resolved, and choose one title and the most representative tags. Never invent owners, \
dates, decisions, or facts not present in the partial results.",
    date_label: "Chat date",
    body_label: "Conversation",
    split_subject: "chat conversation",
    merge_subject: "The conversation",
};

/// Renders the transcript into prompt lines, one per conversational turn.
///
/// See the module docs for why `Meta`, `Permission`, and `Error` are dropped
/// and why a `ToolUse` contributes its summary but never its `input`.
fn render_chat_lines(records: &[ChatRecord]) -> Vec<RenderedLine> {
    records
        .iter()
        .filter_map(|record| match record {
            ChatRecord::User { text, .. } => RenderedLine::new(USER_LABEL, text),
            ChatRecord::Assistant { text, .. } => RenderedLine::new(ASSISTANT_LABEL, text),
            ChatRecord::ToolUse { summary, .. } => RenderedLine::new(TOOL_LABEL, summary),
            ChatRecord::Meta { .. } | ChatRecord::Permission { .. } | ChatRecord::Error { .. } => {
                None
            }
        })
        .collect()
}

/// When the chat started: the `Meta` record's `started_at` (written at spawn,
/// so it is present on every transcript this app produced), else the filename's
/// capture timestamp. `None` when neither is recoverable — a hand-copied file
/// with a mangled name — which sends the caller to the mtime fallback.
fn chat_started_at(
    records: &[ChatRecord],
    parsed_name: Option<&naming::ParsedSessionName>,
) -> Option<DateTime<Utc>> {
    records
        .iter()
        .find_map(|record| match record {
            ChatRecord::Meta { started_at, .. } => DateTime::parse_from_rfc3339(started_at)
                .ok()
                .map(|at| at.with_timezone(&Utc)),
            _ => None,
        })
        .or_else(|| {
            parsed_name.and_then(|parsed| naming::parse_session_timestamp(&parsed.timestamp))
        })
}

/// Distills the chat transcript at `chat_path` into a `type: chat` note under
/// `vault_root`, returning where it landed.
///
/// Mirrors [`distill::distill_session`] contract for contract: it **fails
/// hard** — a runner error or unusable output writes no note and leaves the
/// transcript untouched, so the next sweep retries it — makes one model call
/// for a conversation inside the input budget, map-reduces a longer one, and
/// refuses a transcript past the chunk cap before spending anything.
///
/// A conversation under [`chat::MIN_CHAT_DISTILL_CHARS`] is rejected with
/// [`DistillError::ThinChat`] before the runner is touched: an empty session is
/// a skip, not a failure, and filing a note for it would be noise.
///
/// The note's `date` comes from the chat's own start instant, carried with the
/// device's local offset ([`distill::frontmatter_date_parts`]), and its
/// `source` is the transcript's vault-relative path (`chats/<file>.jsonl`).
pub fn distill_chat(
    runner: &dyn HeadlessClaude,
    vault_root: &Path,
    chat_path: &Path,
    route: &dyn Fn(&DistillOutput, &str) -> Routing,
) -> Result<DistilledNote, DistillError> {
    let records = chat::read_transcript(chat_path).map_err(DistillError::ChatTranscript)?;
    if !chat::has_distillable_substance(&records) {
        return Err(DistillError::ThinChat {
            chars: conversation_chars(&records),
            min: chat::MIN_CHAT_DISTILL_CHARS,
        });
    }

    // Everything derivable without the model comes first, so a bad path fails
    // before a token is spent.
    let source_rel = chat_path
        .strip_prefix(vault_root)
        .map_err(|_| DistillError::SessionOutsideVault(chat_path.to_path_buf()))?;
    let source = Source::parse(&source_rel.to_string_lossy().replace('\\', "/"))?;

    let parsed_name = chat_path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(naming::parse_session_filename);
    let (date, prompt_date) = match chat_started_at(&records, parsed_name.as_ref()) {
        Some(at) => distill::frontmatter_date_parts(at, &Local),
        None => {
            // No start instant to recover: prefer the file's mtime over
            // "today", which would stamp an imported chat as happening now.
            // Date-only, because the clock time is then a guess the schema
            // lets us omit — the same fallback the meeting pass takes.
            let fallback = std::fs::metadata(chat_path)
                .and_then(|meta| meta.modified())
                .map(DateTime::<Utc>::from)
                .unwrap_or_else(|_| Utc::now());
            let date = distill::frontmatter_date_parts(fallback, &Local).1;
            (date.clone(), date)
        }
    };

    let lines = render_chat_lines(&records);

    distill_rendered(
        runner,
        vault_root,
        distill::RenderedDistill {
            flavor: &CHAT_FLAVOR,
            lines: &lines,
            note_type: NoteType::Chat,
            date: &date,
            prompt_date: &prompt_date,
            source,
            // No slug seed: unlike a session filename, a chat filename carries
            // no title (`ChatTranscript::create` passes `slug: None`, and the
            // only slug it ever writes is a collision counter like "2"). With
            // no model title the note falls back to its id, which beats a file
            // named after a disambiguator.
            title_seed_fallback: None,
        },
        route,
    )
}

/// Characters of actual conversation, for the [`DistillError::ThinChat`]
/// message. Counts exactly what [`chat::has_distillable_substance`] counts.
fn conversation_chars(records: &[ChatRecord]) -> usize {
    records
        .iter()
        .filter_map(|record| match record {
            ChatRecord::User { text, .. } | ChatRecord::Assistant { text, .. } => Some(text),
            _ => None,
        })
        .map(|text| text.chars().count())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{ChatTranscript, PermissionResolution};
    use crate::device::DeviceId;
    use crate::distill::RESPONSE_SHAPE_SPEC;
    use crate::llm::{LlmRequest, LlmRunError};
    use crate::note::INBOX;
    use chrono::{FixedOffset, TimeZone};
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Mutex;

    // --- doubles ----------------------------------------------------------

    /// Returns one canned response and records what it was asked.
    struct MockRunner {
        response: String,
        requests: Mutex<Vec<LlmRequest>>,
    }

    impl MockRunner {
        fn new(response: &str) -> Self {
            Self {
                response: response.to_string(),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn requests(&self) -> Vec<LlmRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl HeadlessClaude for MockRunner {
        fn run(&self, request: &LlmRequest) -> Result<String, LlmRunError> {
            self.requests.lock().unwrap().push(LlmRequest {
                system_prompt: request.system_prompt.clone(),
                prompt: request.prompt.clone(),
            });
            Ok(self.response.clone())
        }
    }

    /// Asserts the model is never reached.
    struct PanicRunner;

    impl HeadlessClaude for PanicRunner {
        fn run(&self, _request: &LlmRequest) -> Result<String, LlmRunError> {
            panic!("the runner must not be called");
        }
    }

    fn inbox(_output: &DistillOutput, _body: &str) -> Routing {
        Routing::Routed {
            project: INBOX.to_string(),
            confidence: 0.0,
        }
    }

    const OUTPUT_JSON: &str = r#"{"title": "Irrigation contractor comparison",
"summary": "Compared GreenFlow against the two alternate bidders.",
"decisions": ["Shortlist GreenFlow and Cascade."],
"action_items": [{"owner": "You", "description": "request formal bids", "due_date": "2026-07-15"}],
"open_questions": ["Does the bid cover the clubhouse drainage?"],
"tags": ["research"]}"#;

    // --- fixtures ---------------------------------------------------------

    fn meta(started_at: &str) -> ChatRecord {
        ChatRecord::Meta {
            chat_id: "3c9a35a1-0000-4000-8000-000000000000".to_string(),
            model: "sonnet".to_string(),
            started_at: started_at.to_string(),
        }
    }

    fn user(text: &str) -> ChatRecord {
        ChatRecord::User {
            ts: "2026-07-10T16:15:40Z".to_string(),
            text: text.to_string(),
        }
    }

    fn assistant(text: &str) -> ChatRecord {
        ChatRecord::Assistant {
            ts: "2026-07-10T16:15:45Z".to_string(),
            text: text.to_string(),
        }
    }

    /// Long enough that a user+assistant pair clears the substance bar.
    fn long(prefix: &str) -> String {
        format!("{prefix} {}", "detail ".repeat(60))
    }

    /// A transcript with real substance, written into `<vault>/chats/`.
    fn write_chat(vault: &Path, records: &[ChatRecord]) -> PathBuf {
        let device = DeviceId::parse("k4m2xp7q").unwrap();
        let started = Utc.with_ymd_and_hms(2026, 7, 10, 16, 15, 0).unwrap();
        let transcript = ChatTranscript::create(vault, &device, started).unwrap();
        for record in records {
            transcript.append(record).unwrap();
        }
        transcript.path().to_path_buf()
    }

    fn substantive() -> Vec<ChatRecord> {
        vec![
            meta("2026-07-10T16:15:00Z"),
            user(&long("Which irrigation contractor should we shortlist?")),
            assistant(&long("GreenFlow and Cascade both cleared the budget.")),
        ]
    }

    // --- prompt rendering -------------------------------------------------

    #[test]
    fn renders_user_and_assistant_turns_with_speaker_labels() {
        let lines = render_chat_lines(&[user("Which contractor?"), assistant("GreenFlow.")]);
        let rendered: String = lines.iter().map(|line| line.line()).collect();

        assert_eq!(rendered, "You: Which contractor?\nClaude: GreenFlow.\n");
    }

    #[test]
    fn tool_use_lines_carry_the_summary_and_never_the_input() {
        // The privacy invariant: only what the user saw in the view reaches
        // the prompt. `input` is arbitrary JSON they may never have reviewed.
        let records = vec![
            user("Which contractor?"),
            ChatRecord::ToolUse {
                ts: "2026-07-10T16:15:42Z".to_string(),
                tool: "mcp__kodabi__add_glossary_term".to_string(),
                input: json!({ "definition": "SENTINEL-NEVER-IN-A-PROMPT" }),
                summary: "Asked to add glossary term \"AEC\" to Briarwood Golf".to_string(),
            },
            assistant("GreenFlow."),
        ];

        let rendered: String = render_chat_lines(&records)
            .iter()
            .map(|line| line.line())
            .collect();

        assert!(rendered.contains("Tool: Asked to add glossary term \"AEC\" to Briarwood Golf"));
        assert!(
            !rendered.contains("SENTINEL-NEVER-IN-A-PROMPT"),
            "tool input must never reach the prompt: {rendered}"
        );
    }

    #[test]
    fn meta_permission_and_error_records_are_dropped() {
        let records = vec![
            meta("2026-07-10T16:15:00Z"),
            user("Which contractor?"),
            ChatRecord::Permission {
                ts: "2026-07-10T16:15:50Z".to_string(),
                request_id: "03eeffbb".to_string(),
                tool: "mcp__kodabi__file_note_to_project".to_string(),
                allowed: false,
                resolution: PermissionResolution::Cancelled,
            },
            ChatRecord::Error {
                ts: "2026-07-10T16:16:10Z".to_string(),
                message: "the turn failed".to_string(),
            },
            assistant("GreenFlow."),
        ];

        let rendered: String = render_chat_lines(&records)
            .iter()
            .map(|line| line.line())
            .collect();

        assert_eq!(rendered, "You: Which contractor?\nClaude: GreenFlow.\n");
    }

    #[test]
    fn chat_prompt_shares_the_response_shape_spec_verbatim() {
        // The grammar contract, pinned across its third consumer.
        assert!(distill::system_prompt(&CHAT_FLAVOR).contains(RESPONSE_SHAPE_SPEC));
    }

    #[test]
    fn the_assembled_chat_prompt_reads_as_one_prompt() {
        let prompt = distill::system_prompt(&CHAT_FLAVOR);

        assert!(prompt.starts_with("You are a chat-notes distiller."));
        assert!(prompt.ends_with("only the JSON object."));
        assert!(!prompt.contains("  "), "no doubled space at a seam");
    }

    // --- date derivation --------------------------------------------------

    /// A fixed zone so the rendered strings pin on any host.
    fn tz() -> FixedOffset {
        FixedOffset::west_opt(7 * 3600).unwrap()
    }

    #[test]
    fn chat_date_prefers_the_meta_started_at() {
        let parsed = naming::parse_session_filename("20260101T000000000Z-k4m2xp7q.jsonl");
        let at =
            chat_started_at(&[meta("2026-07-10T16:15:00Z"), user("hi")], parsed.as_ref()).unwrap();

        assert_eq!(
            distill::frontmatter_date_parts(at, &tz()).0,
            "2026-07-10T09:15:00.000-07:00"
        );
    }

    #[test]
    fn chat_date_falls_back_to_the_filename_timestamp() {
        let parsed = naming::parse_session_filename("20260710T161500000Z-k4m2xp7q.jsonl");
        let at = chat_started_at(&[user("hi")], parsed.as_ref()).unwrap();

        assert_eq!(distill::frontmatter_date_parts(at, &tz()).1, "2026-07-10");
    }

    #[test]
    fn chat_date_is_none_when_neither_source_is_recoverable() {
        assert!(chat_started_at(&[user("hi")], None).is_none());
        // A `Meta` whose timestamp doesn't parse must not win over nothing.
        assert!(chat_started_at(&[meta("not a timestamp")], None).is_none());
    }

    // --- end to end -------------------------------------------------------

    #[test]
    fn distill_chat_writes_a_chat_note_with_a_chats_source() {
        let vault = tempfile::tempdir().unwrap();
        let chat_path = write_chat(vault.path(), &substantive());
        let runner = MockRunner::new(OUTPUT_JSON);

        let distilled = distill_chat(&runner, vault.path(), &chat_path, &inbox).unwrap();

        let markdown = std::fs::read_to_string(&distilled.path).unwrap();
        assert!(markdown.contains("type: chat"), "{markdown}");
        assert!(
            markdown.contains(&format!(
                "source: chats/{}",
                chat_path.file_name().unwrap().to_str().unwrap()
            )),
            "{markdown}"
        );
        assert!(markdown.contains("project: Inbox"), "{markdown}");
        assert!(markdown.contains("# Summary"), "{markdown}");
        assert!(markdown.contains("## Decisions"), "{markdown}");
        assert!(
            markdown.contains("- [ ] You to request formal bids by 2026-07-15."),
            "{markdown}"
        );
        assert!(markdown.contains("## Open questions"), "{markdown}");

        // The chat prompt reached the model, framed as a conversation.
        let requests = runner.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].prompt.starts_with("Chat date: 2026-07-10"));
        assert!(requests[0].prompt.contains("\n\nConversation:\nYou: "));
        assert!(requests[0]
            .system_prompt
            .starts_with("You are a chat-notes distiller."));
    }

    #[test]
    fn distill_chat_never_calls_the_runner_for_a_thin_transcript() {
        let vault = tempfile::tempdir().unwrap();
        let chat_path = write_chat(
            vault.path(),
            &[meta("2026-07-10T16:15:00Z"), user("hi"), assistant("hello")],
        );

        let err = distill_chat(&PanicRunner, vault.path(), &chat_path, &inbox).unwrap_err();

        assert!(
            matches!(err, DistillError::ThinChat { chars: 7, min } if min == chat::MIN_CHAT_DISTILL_CHARS),
            "{err:?}"
        );
        // Nothing filed, and the transcript is left for a later sweep.
        assert!(!vault.path().join(INBOX).exists());
        assert!(chat_path.exists());
    }

    #[test]
    fn distill_chat_refuses_a_transcript_outside_the_vault() {
        let vault = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let chat_path = write_chat(elsewhere.path(), &substantive());

        let err = distill_chat(&PanicRunner, vault.path(), &chat_path, &inbox).unwrap_err();

        assert!(
            matches!(err, DistillError::SessionOutsideVault(_)),
            "{err:?}"
        );
    }

    #[test]
    fn a_missing_transcript_fails_without_a_call() {
        let vault = tempfile::tempdir().unwrap();
        let missing = vault.path().join(chat::CHATS_DIR).join("nope.jsonl");

        let err = distill_chat(&PanicRunner, vault.path(), &missing, &inbox).unwrap_err();

        assert!(matches!(err, DistillError::ChatTranscript(_)), "{err:?}");
    }
}
