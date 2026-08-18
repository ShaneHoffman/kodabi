//! Integration test that drives a real headless `claude` subprocess and proves
//! this ticket's Done-when for the *chat* distill pass: a stored chat
//! transcript yields a schema-valid `type: chat` note sourced back to its
//! transcript, shaped the way a chat should be shaped, and the raw tool `input`
//! the user never reviewed reaches neither the prompt nor the note.
//!
//! The sibling of `distill_real.rs`, and `#[ignore]` for the same reasons: it
//! spends real Claude usage (one distill-sized call on the distill default
//! model) and requires a working, authenticated `claude` CLI on `PATH`
//! (subscription login or `ANTHROPIC_API_KEY` — see `kodabi_llm`'s crate docs).
//! Run with:
//!
//! ```text
//! cargo test -p kodabi-llm --test chat_distill_real -- --ignored
//! ```
//!
//! What this does *not* cover: the shell path that reaches
//! [`kodabi_core::chat_distill::distill_chat`] in the running app —
//! `spawn_chat_sweep` → `spawn_chat_distill` → note on disk — because it needs
//! a Tauri `AppHandle` for knowledge-base resolution, event emission, and
//! managed chat state. Everything below that boundary is exercised here.

use std::sync::Mutex;

use chrono::{TimeZone, Utc};
use serde_json::{json, Value};

use kodabi_core::chat::{tool_use_summary, ChatRecord, ChatTranscript};
use kodabi_core::chat_distill::distill_chat;
use kodabi_core::device::DeviceId;
use kodabi_core::distill::inbox_routing;
use kodabi_core::llm::{HeadlessClaude, LlmRequest, LlmRunError};
use kodabi_core::note::{Note, NoteType, Source, INBOX};
use kodabi_llm::{ClaudeConfig, ClaudeRunner};

/// A proper noun planted in one tool call's raw `input` and appearing nowhere
/// else in the fixture: not in a user turn, not in an assistant turn, not in
/// the tool's own summary.
///
/// [`tool_use_summary`] renders a `search_notes` call's `query` and never its
/// `project` filter, so this string lives only in bytes the user never
/// reviewed. If it surfaces in the outbound prompt or in the written note,
/// `ChatRecord::ToolUse.input` leaked.
///
/// Deliberately a plausible project name rather than a random token: a
/// summarizer that had actually seen it would carry it straight into its prose,
/// which is what gives the note-level assertion teeth against a real model.
const TOOL_INPUT_SENTINEL: &str = "Halcyon";

/// The sections [`kodabi_core::distill::render_body`] may emit after
/// `# Summary`, in the order it emits them. Each is optional; a chat note that
/// reaches none of them is correct.
const BODY_SECTIONS: [&str; 3] = ["## Decisions", "## Action items", "## Open questions"];

// --- doubles ---------------------------------------------------------------

/// The real runner, plus a record of what actually left the machine.
///
/// Delegates every call unchanged; the recording is the point. It lets the
/// privacy invariant be checked at the seam it is really about (the outbound
/// request) and not only at the far end of a nondeterministic model, and it
/// pins the call count so a fixture that quietly grew past the input budget
/// into the map-reduce path would fail loudly.
struct RecordingRunner {
    inner: ClaudeRunner,
    requests: Mutex<Vec<LlmRequest>>,
}

impl RecordingRunner {
    fn new() -> Self {
        Self {
            inner: ClaudeRunner::new(ClaudeConfig::distill()),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<LlmRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl HeadlessClaude for RecordingRunner {
    fn run(&self, request: &LlmRequest) -> Result<String, LlmRunError> {
        self.requests.lock().unwrap().push(request.clone());
        self.inner.run(request)
    }
}

// --- fixture ---------------------------------------------------------------

fn user(text: &str) -> ChatRecord {
    ChatRecord::User {
        ts: "2026-07-10T16:15:40Z".to_owned(),
        text: text.to_owned(),
    }
}

fn assistant(text: &str) -> ChatRecord {
    ChatRecord::Assistant {
        ts: "2026-07-10T16:15:52Z".to_owned(),
        text: text.to_owned(),
    }
}

/// A tool call whose summary is derived from its input by the same function the
/// chat view calls, so the fixture cannot drift into claiming a summary the
/// real renderer would never have produced.
fn tool_use(tool: &str, input: Value) -> ChatRecord {
    ChatRecord::ToolUse {
        ts: "2026-07-10T16:15:45Z".to_owned(),
        tool: tool.to_owned(),
        summary: tool_use_summary(tool, &input),
        input,
    }
}

/// A realistic multi-turn chat that is **pure question and answer**: the user
/// asks two questions and takes on nothing, the assistant answers and commits
/// to nothing. Nobody says "I will", no owner is named as taking anything on,
/// and no date is stated or implied anywhere — so a rendered action item with
/// an owner or a due date could only have been invented, which is what makes
/// the assertion below fair.
///
/// The two user turns plus the two assistant turns carry roughly 1,065
/// characters, comfortably clearing `MIN_CHAT_DISTILL_CHARS` (400) so
/// `has_distillable_substance` passes and the runner is actually reached.
fn qa_chat_records() -> Vec<ChatRecord> {
    vec![
        ChatRecord::Meta {
            chat_id: "3c9a35a1-0000-4000-8000-000000000000".to_owned(),
            model: "sonnet".to_owned(),
            started_at: "2026-07-10T16:15:00Z".to_owned(),
        },
        user(
            "I keep running into MERIDIAN in my notes and I am not sure I have a clean \
             definition of it. What is it, and which projects has it actually come up in?",
        ),
        // A read tool, and the one carrying the sentinel: `project` is a real
        // `search_notes` filter that `tool_use_summary` never renders.
        tool_use(
            "mcp__kodabi__search_notes",
            json!({
                "query": "MERIDIAN",
                "project": TOOL_INPUT_SENTINEL,
                "limit": 10,
            }),
        ),
        assistant(
            "MERIDIAN is the regional systems migration program. It appears in eleven notes \
             across Briarwood Golf and Cedar Ridge, almost all of them weekly ops syncs. The \
             Briarwood notes use it as the umbrella for moving the tee sheet and point of sale \
             off TeeTrack; the Cedar Ridge notes use the same name for the general ledger side \
             of the same migration. That is why it can read as two separate projects when you \
             skim the titles.",
        ),
        user(
            "So are those two uses the same program, or did somebody reuse the name for \
             something unrelated?",
        ),
        tool_use(
            "mcp__kodabi__get_note",
            json!({ "id": "n_a1b2c3", "include_body": true }),
        ),
        assistant(
            "From what the notes themselves say, it is one program. The Cedar Ridge kickoff \
             note defines MERIDIAN as covering both the point of sale and the general ledger \
             cutover, and the Briarwood glossary entry points at that same definition rather \
             than a narrower one. The one thing the record does not settle is who owns the \
             ledger half once the cutover finishes; no note names an owner, so I cannot answer \
             that from what is written.",
        ),
    ]
}

/// The lines of one `##` section of a rendered note body, blank lines dropped.
fn section_lines<'b>(body: &'b str, heading: &str) -> Vec<&'b str> {
    body.lines()
        .skip_while(|line| *line != heading)
        .skip(1)
        .take_while(|line| !line.starts_with("## "))
        .filter(|line| !line.trim().is_empty())
        .collect()
}

/// Whether a rendered action item ends in the optional ` by YYYY-MM-DD` clause.
fn has_due_date(rendered: &str) -> bool {
    rendered.rsplit_once(" by ").is_some_and(|(_, tail)| {
        tail.len() == 10
            && tail
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'-')
    })
}

// --- the test --------------------------------------------------------------

#[test]
#[ignore = "spawns a real headless `claude` process and spends real usage on the distill model"]
fn distills_a_stored_chat_into_a_schema_valid_chat_note() {
    let vault = tempfile::tempdir().expect("tempdir");
    let device = DeviceId::parse("k4m2xp7q").unwrap();
    let started = Utc.with_ymd_and_hms(2026, 7, 10, 16, 15, 0).unwrap();

    // The real writer, so the bytes the pass reads back are the bytes the app
    // would have produced: the `chats/` layout, the session filename scheme,
    // and the serde tagging of every record.
    let transcript = ChatTranscript::create(vault.path(), &device, started).expect("transcript");
    for record in qa_chat_records() {
        transcript.append(&record).expect("append should persist");
    }
    let chat_path = transcript.path().to_path_buf();
    let chat_file = chat_path
        .file_name()
        .and_then(|name| name.to_str())
        .expect("transcript filename")
        .to_owned();

    let runner = RecordingRunner::new();
    let distilled = distill_chat(
        &runner,
        vault.path(),
        &chat_path,
        &|_, _| inbox_routing(),
        &no_open_entries,
    )
    .expect("distill should succeed");

    let written = std::fs::read_to_string(&distilled.path).expect("note file should exist");
    let note = Note::from_markdown(&written).expect("note should be schema-valid");

    // --- a chat note landed, sourced back to its transcript ---------------
    assert_eq!(note.id, distilled.id);
    assert_eq!(note.note_type, NoteType::Chat);
    assert_eq!(note.routing.project(), INBOX);
    assert_eq!(note.routing.confidence(), Some(0.0));
    // The vault-relative transcript path, not the bare `chat` keyword: pinning
    // the variant means a regression to `Source::Keyword` fails here rather
    // than passing on string equality.
    assert_eq!(
        note.source,
        Source::RawArtifact(format!("chats/{chat_file}"))
    );
    // The chat's own start instant carried at the device's local offset, never
    // "today": 16:15Z on the 10th is the 10th or the 11th in every real zone.
    assert!(
        note.date.starts_with("2026-07-10") || note.date.starts_with("2026-07-11"),
        "date should be the chat's own day, got: {}",
        note.date
    );

    // --- the body is what `render_body` promises --------------------------
    assert!(note.body.starts_with("# Summary"), "body:\n{}", note.body);

    let summary = section_lines(&note.body, "# Summary").join("\n");
    assert!(
        summary.chars().count() >= 40,
        "expected real summary prose, got {summary:?}"
    );

    // Every `##` heading is one of the three `render_body` may emit, and they
    // appear in its order. A chat note is free to omit all three; it is not
    // free to invent a fourth or to reorder them.
    let seen: Vec<usize> = note
        .body
        .lines()
        .filter(|line| line.starts_with("## "))
        .map(|heading| {
            BODY_SECTIONS
                .iter()
                .position(|section| *section == heading)
                .unwrap_or_else(|| panic!("unknown section {heading:?} in body:\n{}", note.body))
        })
        .collect();
    assert!(
        seen.windows(2).all(|pair| pair[0] < pair[1]),
        "sections out of order in body:\n{}",
        note.body
    );

    // --- the chat-shaped emphasis took ------------------------------------
    // `CHAT_FLAVOR` tells the model a chat is usually thinking out loud, so
    // empty decisions and action items are the correct answer, and that only a
    // commitment the *user* actually made is an action item. This conversation
    // contains none.
    //
    // The gate is deliberately not "no `## Action items` section at all": that
    // asks a real model for perfect restraint on every run, and one over-eager
    // `- [ ] Unassigned to work out who owns the ledger half.` would turn this
    // into a coin flip. What is never defensible is a fabricated fact — an
    // owner nobody volunteered, or a date nobody stated. That is the mirror
    // image of `distill_real.rs`, where a meeting with two owned commitments
    // must yield at least one owned item.
    //
    // So this loop is a regression guard, and on a well-behaved model it is
    // expected to iterate zero times: observed runs against sonnet emitted no
    // action items at all and correctly routed the unresolved ownership of the
    // ledger half to `## Open questions` instead, which is exactly the shape
    // `CHAT_FLAVOR` asks for. It fires only when that restraint breaks.
    for line in section_lines(&note.body, "## Action items") {
        let rendered = line
            .strip_prefix("- [ ] ")
            .unwrap_or_else(|| panic!("not a checkbox line: {line}"));
        let rendered = rendered
            .strip_suffix('.')
            .unwrap_or_else(|| panic!("no terminal period: {line}"));
        assert!(
            rendered.starts_with("Unassigned to "),
            "nobody in this chat committed to anything, so an owned action item is invented: \
             {line}"
        );
        assert!(
            !has_due_date(rendered),
            "no date is stated or implied anywhere in this chat: {line}"
        );
    }

    // --- the privacy invariant, end to end --------------------------------
    let requests = runner.requests();
    assert_eq!(requests.len(), 1, "the fixture should fit one call");
    let request = &requests[0];

    // The prompt is framed as a conversation, and the tool call contributed its
    // human-readable summary — the same string the chat view rendered.
    assert!(
        request
            .system_prompt
            .starts_with("You are a chat-notes distiller."),
        "system prompt: {}",
        request.system_prompt
    );
    assert!(
        request.prompt.starts_with("Chat date: 2026-07-1"),
        "prompt: {}",
        request.prompt
    );
    assert!(
        request.prompt.contains("\n\nConversation:\nYou: ")
            && request.prompt.contains("\nClaude: ")
            && request
                .prompt
                .contains("\nTool: Searched notes for \"MERIDIAN\""),
        "prompt: {}",
        request.prompt
    );

    // Nothing from `ToolUse.input` left the machine, and nothing from it came
    // back in the note — frontmatter and body alike, so the raw file text.
    let sentinel = TOOL_INPUT_SENTINEL.to_lowercase();
    for (label, haystack) in [
        ("system prompt", request.system_prompt.as_str()),
        ("prompt", request.prompt.as_str()),
        ("note", written.as_str()),
    ] {
        assert!(
            !haystack.to_lowercase().contains(&sentinel),
            "raw tool input leaked into the {label}:\n{haystack}"
        );
    }
}

/// The fetcher for a distill with no ledger behind it: this test exercises the
/// real model against the note pipeline, not the commitment classifications.
fn no_open_entries(
    _: &kodabi_core::routing::RouteGuess,
) -> Vec<kodabi_core::distill::OpenCommitment> {
    Vec::new()
}
