//! Pure logic for the designed chat view: the headless Claude Code spawn plan,
//! the stream-json wire protocol (both directions), human-readable tool
//! summaries, and the persisted chat transcript.
//!
//! The chat view (FOUNDING_DOC §4, "two AI surfaces, one architecture") drives
//! a *headless* `claude` wired to the `kodabi` MCP server: one long-lived
//! process per session, user messages in on stdin as newline-delimited JSON,
//! events out on stdout, and MCP write-tool permission prompts routed over the
//! same pipes as `can_use_tool` control requests (`--permission-prompt-tool
//! stdio`). The side-effecting process spawn lives in `kodabi-llm`'s chat
//! module and the Tauri orchestration in `src-tauri/src/chat_cmds.rs`;
//! everything here is pure and unit-tested without a subprocess, mirroring
//! [`crate::terminal`].
//!
//! Because every Kodabi `claude` spawn sets
//! [`crate::terminal::SKIP_HISTORY_ENV`], Claude Code keeps no transcript of
//! its own — the JSONL transcript written by [`ChatTranscript`] under
//! [`CHATS_DIR`] is the *only* durable record of a chat, and it is the raw
//! artifact the chat-sessions-as-documents pass will later distill into a
//! `type: chat` note. The retention sweep does not touch `chats/`; a chat
//! transcript is user knowledge, not a recovery artifact.

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::device::DeviceId;
use crate::naming;
use crate::terminal::READ_TOOL_PERMISSIONS;

/// Default model for chat turns. Distill favours throughput (`sonnet` there
/// too, but per-call); chat is interactive and grounded by the MCP tools, so
/// `sonnet` balances latency against answer quality. Overridable via the
/// `KODABI_CHAT_MODEL` env read in `kodabi-llm`'s chat config.
pub const CHAT_DEFAULT_MODEL: &str = "sonnet";

/// Knowledge-base root directory holding chat transcripts, reserved in
/// `note::RESERVED_ROOT_DIRS` so a project can never claim it. Sibling of
/// `sessions/` but deliberately separate: membership of the needs-attention
/// list is derived from `sessions/*.jsonl`, and a chat transcript must never
/// look like an unclaimed capture.
pub const CHATS_DIR: &str = "chats";

/// The argv (after the program name) for the *headless chat* `claude` launch.
/// The third spawn shape in the tree, between the hermetic distill runner
/// (`kodabi-llm`: `-p`, no tools, no MCP) and the interactive terminal
/// (`crate::terminal`: no `-p`, real TTY):
///
/// - Bidirectional stream-json: user messages go in on stdin, events come out
///   on stdout, and one process carries the whole multi-turn session.
/// - `--tools ""` disables every built-in tool, so the model can only reach
///   the `kodabi` MCP tools — chat never edits files or runs commands.
/// - The read tools are pre-approved via `--allowedTools`
///   ([`READ_TOOL_PERMISSIONS`], the same single source the terminal uses);
///   the write tools are deliberately absent and `--permission-mode` is
///   deliberately omitted, so a write surfaces as a `can_use_tool` control
///   request for the UI's inline permission card to answer — the headless
///   analog of the terminal's TTY prompt.
/// - `--permission-prompt-tool stdio` is what routes those permission
///   requests onto stdout as control requests (verified against a live
///   `claude`; absent from `--help` but it is the mechanism the Agent SDK
///   itself uses). Without it the CLI silently auto-denies unapproved tools.
/// - `--session-id` pins Claude Code's session id to the caller-supplied
///   UUID, so the app's chat id and the CLI's session id are one value.
///
/// The child env ([`crate::terminal::SKIP_HISTORY_ENV`]) and working
/// directory are the shell's job — they are not argv.
pub fn chat_argv(mcp_config: &Path, model: &str, session_id: &str) -> Vec<OsString> {
    vec![
        OsString::from("-p"),
        OsString::from("--input-format"),
        OsString::from("stream-json"),
        OsString::from("--output-format"),
        OsString::from("stream-json"),
        OsString::from("--include-partial-messages"),
        OsString::from("--verbose"),
        OsString::from("--model"),
        OsString::from(model),
        OsString::from("--system-prompt"),
        OsString::from(chat_system_prompt()),
        OsString::from("--tools"),
        OsString::from(""),
        OsString::from("--mcp-config"),
        mcp_config.as_os_str().to_os_string(),
        OsString::from("--strict-mcp-config"),
        OsString::from("--setting-sources"),
        OsString::from(""),
        OsString::from("--allowedTools"),
        OsString::from(READ_TOOL_PERMISSIONS.join(",")),
        OsString::from("--permission-prompt-tool"),
        OsString::from("stdio"),
        OsString::from("--session-id"),
        OsString::from(session_id),
    ]
}

/// The chat session's system prompt. User-facing in effect (its voice shapes
/// every answer), so it follows the copy rules: plain sentences, no em dashes.
pub fn chat_system_prompt() -> &'static str {
    "You are Kodabi's chat assistant. Kodabi turns the user's meetings and captured \
     thoughts into notes that live in a local knowledge base, organized into projects. \
     Ground every answer in that knowledge base through the kodabi tools: search notes, \
     open a note by id, and list projects. More tools may appear over time; use them \
     when they fit. Name the notes you drew on. If a search finds nothing, say so \
     rather than guessing. When the user asks you to file a note into a project or to \
     add a glossary term, call the matching write tool; Kodabi will ask the user to \
     approve the call, so never ask for permission yourself in prose. Keep answers \
     short and specific."
}

// ---------------------------------------------------------------------------
// Stream parsing: CLI stdout → typed items
// ---------------------------------------------------------------------------

/// One semantically interesting item from the CLI's stdout stream. A single
/// stdout line can carry several (an `assistant` snapshot may hold multiple
/// content blocks), so [`parse_stream_line`] returns a `Vec`; an empty vec
/// means the line carried nothing the chat needs (thinking deltas, status
/// noise, acks of our own control requests, or an unknown type from a newer
/// CLI — degrading quietly on a version bump is the contract).
#[derive(Debug, Clone, PartialEq)]
pub enum ChatStreamItem {
    /// The per-turn `system`/`init` line. `session_id` echoes the
    /// `--session-id` we supplied; re-emitted at every turn start.
    Init { session_id: String },
    /// A streamed assistant text fragment (`text_delta`).
    TextDelta { text: String },
    /// A completed assistant text block, authoritative over any accumulated
    /// deltas for it.
    AssistantText { text: String },
    /// A completed tool call with its full input. Parsed from the `assistant`
    /// snapshot, never from `content_block_start` (whose `input` is empty
    /// while the arguments stream in as JSON deltas).
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    /// A `can_use_tool` control request: the CLI is blocked waiting for
    /// [`permission_response_json`] with this `request_id`.
    PermissionRequest {
        request_id: String,
        tool_name: String,
        display_name: Option<String>,
        input: Value,
    },
    /// A control request of a subtype this build doesn't know. The pump must
    /// still acknowledge it ([`control_ack_json`]) or the CLI could hang
    /// waiting on a reply.
    UnknownControlRequest { request_id: String },
    /// End of a turn. `is_error` is also `true` for a turn the user
    /// interrupted, so the caller distinguishes "stopped" from "failed" by
    /// whether it initiated an interrupt.
    TurnResult {
        is_error: bool,
        result: Option<String>,
    },
}

/// Parses one stdout line into the items the chat cares about. Never errors:
/// unparsable or unrecognized lines yield an empty vec.
pub fn parse_stream_line(line: &str) -> Vec<ChatStreamItem> {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    match value.get("type").and_then(Value::as_str) {
        Some("system") if value.get("subtype").and_then(Value::as_str) == Some("init") => value
            .get("session_id")
            .and_then(Value::as_str)
            .map(|session_id| ChatStreamItem::Init {
                session_id: session_id.to_string(),
            })
            .into_iter()
            .collect(),
        Some("stream_event") => {
            let delta = &value["event"]["delta"];
            if value["event"]["type"] == "content_block_delta" && delta["type"] == "text_delta" {
                delta
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|text| ChatStreamItem::TextDelta {
                        text: text.to_string(),
                    })
                    .into_iter()
                    .collect()
            } else {
                Vec::new()
            }
        }
        Some("assistant") => value["message"]["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|block| match block.get("type").and_then(Value::as_str) {
                        Some("text") => block.get("text").and_then(Value::as_str).map(|text| {
                            ChatStreamItem::AssistantText {
                                text: text.to_string(),
                            }
                        }),
                        Some("tool_use") => Some(ChatStreamItem::ToolUse {
                            id: block["id"].as_str().unwrap_or_default().to_string(),
                            name: block["name"].as_str().unwrap_or_default().to_string(),
                            input: block.get("input").cloned().unwrap_or(Value::Null),
                        }),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Some("control_request") => {
            let Some(request_id) = value.get("request_id").and_then(Value::as_str) else {
                return Vec::new();
            };
            let request = &value["request"];
            if request["subtype"] == "can_use_tool" {
                vec![ChatStreamItem::PermissionRequest {
                    request_id: request_id.to_string(),
                    tool_name: request["tool_name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    display_name: request["display_name"].as_str().map(str::to_string),
                    input: request.get("input").cloned().unwrap_or(Value::Null),
                }]
            } else {
                vec![ChatStreamItem::UnknownControlRequest {
                    request_id: request_id.to_string(),
                }]
            }
        }
        Some("result") => vec![ChatStreamItem::TurnResult {
            is_error: value
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            result: value
                .get("result")
                .and_then(Value::as_str)
                .map(str::to_string),
        }],
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Stdin encoding: host → CLI
// ---------------------------------------------------------------------------

/// A user chat message as one stream-json stdin line (no trailing newline; the
/// writer adds it).
pub fn user_message_json(text: &str) -> String {
    json!({
        "type": "user",
        "message": { "role": "user", "content": [{ "type": "text", "text": text }] },
    })
    .to_string()
}

/// The user's answer to a `can_use_tool` permission request.
#[derive(Debug, Clone, PartialEq)]
pub enum PermissionDecision {
    /// Run the tool. `updated_input` echoes the requested input (the protocol
    /// allows editing it; chat passes it through unchanged).
    Allow { updated_input: Value },
    /// Refuse the tool; `message` is what the model sees as the tool result.
    Deny { message: String },
}

/// Encodes the reply to a [`ChatStreamItem::PermissionRequest`]. Shape
/// verified against a live `claude`: the tool runs on `behavior: "allow"` and
/// the turn continues with the deny message as an error result otherwise.
pub fn permission_response_json(request_id: &str, decision: &PermissionDecision) -> String {
    let response = match decision {
        PermissionDecision::Allow { updated_input } => {
            json!({ "behavior": "allow", "updatedInput": updated_input })
        }
        PermissionDecision::Deny { message } => {
            json!({ "behavior": "deny", "message": message })
        }
    };
    json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": request_id, "response": response },
    })
    .to_string()
}

/// A generic success acknowledgement for a control request the chat doesn't
/// understand ([`ChatStreamItem::UnknownControlRequest`]) — leaving one
/// unanswered risks wedging the CLI.
pub fn control_ack_json(request_id: &str) -> String {
    json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": request_id, "response": {} },
    })
    .to_string()
}

/// An interrupt request cancelling the in-flight turn. The CLI acknowledges
/// it and ends the turn with a `result` whose `is_error` is `true` — the
/// caller initiated the interrupt, so it renders that result as "stopped",
/// not as a failure. `request_id` is caller-chosen and only needs to be
/// unique within the session.
pub fn interrupt_request_json(request_id: &str) -> String {
    json!({
        "type": "control_request",
        "request_id": request_id,
        "request": { "subtype": "interrupt" },
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// Human-readable tool copy (user-facing: plain sentences, no em dashes)
// ---------------------------------------------------------------------------

/// Strips the `mcp__kodabi__` namespace for display.
fn short_tool_name(tool_name: &str) -> &str {
    tool_name.strip_prefix("mcp__kodabi__").unwrap_or(tool_name)
}

fn input_str<'v>(input: &'v Value, key: &str) -> Option<&'v str> {
    input.get(key).and_then(Value::as_str)
}

/// One quiet log line describing a completed tool call. Unknown tools fall
/// back to their bare name, so a tool added later (e.g. task #92's
/// `list_outstanding_items`) degrades to something truthful with no change
/// here.
pub fn tool_use_summary(tool_name: &str, input: &Value) -> String {
    match short_tool_name(tool_name) {
        "search_notes" => match input_str(input, "query") {
            Some(query) => format!("Searched notes for \"{query}\""),
            None => "Searched notes".to_string(),
        },
        "get_note" => match input_str(input, "id") {
            Some(id) => format!("Opened note {id}"),
            None => "Opened a note".to_string(),
        },
        "list_projects" => "Listed projects".to_string(),
        "file_note_to_project" => match (input_str(input, "id"), input_str(input, "project")) {
            (Some(id), Some(project)) => format!("Filed note {id} to {project}"),
            _ => "Filed a note".to_string(),
        },
        "add_glossary_term" => match (input_str(input, "term"), input_str(input, "project")) {
            (Some(term), Some(project)) => {
                format!("Added glossary term \"{term}\" to {project}")
            }
            _ => "Added a glossary term".to_string(),
        },
        other => format!("Used {other}"),
    }
}

/// The question an inline permission card asks for a write-tool request.
/// `display_name` is the CLI-provided human title (e.g. "Add Glossary Term"),
/// the fallback when the tool is one this build doesn't know.
pub fn permission_question(tool_name: &str, display_name: Option<&str>, input: &Value) -> String {
    match short_tool_name(tool_name) {
        "file_note_to_project" => match (input_str(input, "id"), input_str(input, "project")) {
            (Some(id), Some(project)) => format!("File note {id} to {project}?"),
            _ => "File this note to a project?".to_string(),
        },
        "add_glossary_term" => match (input_str(input, "term"), input_str(input, "project")) {
            (Some(term), Some(project)) => format!("Add glossary term \"{term}\" to {project}?"),
            _ => "Add a glossary term?".to_string(),
        },
        other => format!("Allow {}?", display_name.unwrap_or(other)),
    }
}

// ---------------------------------------------------------------------------
// Transcript persistence
// ---------------------------------------------------------------------------

/// How a pending permission request was resolved, recorded alongside the
/// allow/deny bit so the transcript is a faithful audit trail: a prompt lost
/// to a stop or an exit is a deny, and the record says which kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionResolution {
    /// The user answered the card.
    User,
    /// The turn was stopped while the card was open.
    Cancelled,
    /// The session ended (app exit or restart) while the card was open.
    SessionClosed,
}

/// One line of the persisted chat transcript JSONL. `ts` values are UTC
/// RFC 3339 with a `Z` suffix ([`now_ts`]), per the repo's UTC rule. Streaming
/// deltas are never persisted; `Assistant` holds the completed block text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatRecord {
    /// First line of every transcript. `chat_id` doubles as Claude Code's
    /// session id (the spawn pins `--session-id` to it).
    Meta {
        chat_id: String,
        model: String,
        started_at: String,
    },
    User {
        ts: String,
        text: String,
    },
    Assistant {
        ts: String,
        text: String,
    },
    ToolUse {
        ts: String,
        tool: String,
        input: Value,
        summary: String,
    },
    Permission {
        ts: String,
        request_id: String,
        tool: String,
        allowed: bool,
        resolution: PermissionResolution,
    },
    Error {
        ts: String,
        message: String,
    },
}

/// The current instant in the transcript's timestamp format (UTC RFC 3339,
/// seconds precision, `Z` suffix — the `vault::corrected_at` shape).
pub fn now_ts() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// An append-only chat transcript under `<kb_root>/chats/`, named by the
/// session filename scheme (`{timestamp}-{deviceId}.jsonl`) so transcripts
/// sort chronologically and import safely across devices, exactly like
/// `sessions/`.
#[derive(Debug)]
pub struct ChatTranscript {
    path: PathBuf,
}

impl ChatTranscript {
    /// Creates the transcript file (and `chats/` if needed). A same-device
    /// same-millisecond collision is resolved with a numbered slug, mirroring
    /// `raw_session`'s discipline: a name clash is a real collision to
    /// resolve, never something to overwrite.
    pub fn create(
        kb_root: &Path,
        device: &DeviceId,
        started_at: DateTime<Utc>,
    ) -> io::Result<Self> {
        let dir = kb_root.join(CHATS_DIR);
        std::fs::create_dir_all(&dir)?;

        let mut attempt: u32 = 1;
        loop {
            let slug = if attempt == 1 {
                None
            } else {
                Some(naming::numbered_slug(None, attempt))
            };
            let name = naming::session_filename(started_at, device, slug.as_deref(), "jsonl");
            let path = dir.join(name);
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(Self { path }),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                    attempt += 1;
                    if attempt > 1000 {
                        return Err(err);
                    }
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// The transcript's on-disk path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one record as a JSONL line and flushes it, so a crash loses at
    /// most the record being written.
    pub fn append(&self, record: &ChatRecord) -> io::Result<()> {
        let mut line = serde_json::to_string(record)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        line.push('\n');
        let mut file = OpenOptions::new().append(true).open(&self.path)?;
        file.write_all(line.as_bytes())?;
        file.flush()
    }
}

/// Reads a transcript back into records, skipping lines that don't parse (a
/// truncated final line after a crash must not poison the rest).
pub fn read_transcript(path: &Path) -> io::Result<Vec<ChatRecord>> {
    let content = std::fs::read_to_string(path)?;
    Ok(content
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    // Fixture lines captured from a live `claude` (2.1.x) driven exactly as
    // `chat_argv` spawns it; opaque long fields trimmed, structure verbatim.
    const INIT_LINE: &str = r#"{"type":"system","subtype":"init","cwd":"C:\\spike","session_id":"38780a8b-87d8-4d62-aca5-337f3b029851","tools":["mcp__kodabi__add_glossary_term","mcp__kodabi__file_note_to_project","mcp__kodabi__get_note","mcp__kodabi__list_projects","mcp__kodabi__search_notes"],"mcp_servers":[{"name":"kodabi","status":"connected"}],"model":"claude-haiku-4-5-20251001","permissionMode":"default"}"#;
    const TEXT_DELTA_LINE: &str = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"pong"}},"session_id":"0a7bb7d5-08fd-4176-a4e0-638b98731fa2","parent_tool_use_id":null,"uuid":"183cf401"}"#;
    const THINKING_DELTA_LINE: &str = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"The user is asking","estimated_tokens":null}},"session_id":"0a7bb7d5","parent_tool_use_id":null,"uuid":"77a5984b"}"#;
    const ASSISTANT_TEXT_LINE: &str = r#"{"type":"assistant","message":{"model":"claude-haiku-4-5-20251001","id":"msg_011","type":"message","role":"assistant","content":[{"type":"text","text":"pong"}],"stop_reason":null},"parent_tool_use_id":null,"session_id":"0a7bb7d5"}"#;
    const ASSISTANT_TOOL_USE_LINE: &str = r#"{"type":"assistant","message":{"model":"claude-haiku-4-5-20251001","id":"msg_011","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_0133","name":"mcp__kodabi__add_glossary_term","input":{"term":"AEC","definition":"acoustic echo cancellation","project":"Briarwood Golf"},"caller":{"type":"direct"}}],"stop_reason":null},"parent_tool_use_id":null,"session_id":"38780a8b"}"#;
    const CAN_USE_TOOL_LINE: &str = r#"{"type":"control_request","request_id":"03eeffbb-d44e-4be3-b536-0f4e003d6f35","request":{"subtype":"can_use_tool","tool_name":"mcp__kodabi__add_glossary_term","display_name":"Add Glossary Term","input":{"project":"Briarwood Golf","term":"AEC","definition":"acoustic echo cancellation"},"permission_suggestions":[{"type":"addRules","rules":[{"toolName":"mcp__kodabi__add_glossary_term"}],"behavior":"allow","destination":"localSettings"}],"tool_use_id":"toolu_01GU"}}"#;
    const RESULT_LINE: &str = r#"{"type":"result","subtype":"success","is_error":false,"duration_api_ms":2133,"num_turns":1,"stop_reason":"end_turn","session_id":"0a7bb7d5","total_cost_usd":0.014,"result":"pong"}"#;
    const INTERRUPTED_RESULT_LINE: &str = r#"{"type":"result","is_error":true,"duration_api_ms":983,"num_turns":2,"stop_reason":null,"session_id":"cefc537c","total_cost_usd":0.0006}"#;
    const CONTROL_ACK_LINE: &str = r#"{"type":"control_response","response":{"subtype":"success","request_id":"spike_1","response":{"still_queued":[]}}}"#;
    const STATUS_LINE: &str = r#"{"type":"system","subtype":"status","status":"requesting","uuid":"936a1984","session_id":"0a7bb7d5"}"#;

    fn os(argv: &[OsString]) -> Vec<String> {
        argv.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn chat_argv_is_headless_streaming_with_reads_preapproved() {
        let argv = os(&chat_argv(
            Path::new("C:/cfg/kodabi.mcp.json"),
            CHAT_DEFAULT_MODEL,
            "3c9a35a1-0000-4000-8000-000000000000",
        ));

        for expected in [
            "-p",
            "--input-format",
            "--output-format",
            "--include-partial-messages",
            "--strict-mcp-config",
            "--permission-prompt-tool",
        ] {
            assert!(argv.contains(&expected.to_string()), "missing {expected}");
        }
        assert_eq!(argv.iter().filter(|a| *a == "stream-json").count(), 2);
        assert!(argv.contains(&"C:/cfg/kodabi.mcp.json".to_string()));
        assert!(argv.contains(&"3c9a35a1-0000-4000-8000-000000000000".to_string()));
        assert!(argv.contains(&"stdio".to_string()));

        // Reads pre-approved, writes absent, and no permission mode: an
        // unapproved write must surface as a can_use_tool request, never be
        // auto-answered.
        let allowed = &argv[argv.iter().position(|a| a == "--allowedTools").unwrap() + 1];
        assert_eq!(allowed, &READ_TOOL_PERMISSIONS.join(","));
        assert!(!allowed.contains("file_note_to_project"));
        assert!(!allowed.contains("add_glossary_term"));
        assert!(!argv.contains(&"--permission-mode".to_string()));

        // No built-in tools: `--tools` is present with an empty value.
        let tools = &argv[argv.iter().position(|a| a == "--tools").unwrap() + 1];
        assert_eq!(tools, "");
    }

    #[test]
    fn system_prompt_follows_copy_style() {
        assert!(!chat_system_prompt().contains('\u{2014}'), "no em dashes");
    }

    #[test]
    fn parses_init_line() {
        assert_eq!(
            parse_stream_line(INIT_LINE),
            vec![ChatStreamItem::Init {
                session_id: "38780a8b-87d8-4d62-aca5-337f3b029851".to_string()
            }]
        );
    }

    #[test]
    fn parses_text_delta_and_ignores_thinking_delta() {
        assert_eq!(
            parse_stream_line(TEXT_DELTA_LINE),
            vec![ChatStreamItem::TextDelta {
                text: "pong".to_string()
            }]
        );
        assert!(parse_stream_line(THINKING_DELTA_LINE).is_empty());
    }

    #[test]
    fn parses_assistant_snapshots_per_block() {
        assert_eq!(
            parse_stream_line(ASSISTANT_TEXT_LINE),
            vec![ChatStreamItem::AssistantText {
                text: "pong".to_string()
            }]
        );
        let items = parse_stream_line(ASSISTANT_TOOL_USE_LINE);
        assert_eq!(items.len(), 1);
        let ChatStreamItem::ToolUse { id, name, input } = &items[0] else {
            panic!("expected ToolUse, got {items:?}");
        };
        assert_eq!(id, "toolu_0133");
        assert_eq!(name, "mcp__kodabi__add_glossary_term");
        assert_eq!(input["term"], "AEC");
    }

    #[test]
    fn parses_can_use_tool_request() {
        let items = parse_stream_line(CAN_USE_TOOL_LINE);
        assert_eq!(items.len(), 1);
        let ChatStreamItem::PermissionRequest {
            request_id,
            tool_name,
            display_name,
            input,
        } = &items[0]
        else {
            panic!("expected PermissionRequest, got {items:?}");
        };
        assert_eq!(request_id, "03eeffbb-d44e-4be3-b536-0f4e003d6f35");
        assert_eq!(tool_name, "mcp__kodabi__add_glossary_term");
        assert_eq!(display_name.as_deref(), Some("Add Glossary Term"));
        assert_eq!(input["project"], "Briarwood Golf");
    }

    #[test]
    fn unknown_control_request_still_carries_its_id() {
        let line = r#"{"type":"control_request","request_id":"abc-123","request":{"subtype":"something_new"}}"#;
        assert_eq!(
            parse_stream_line(line),
            vec![ChatStreamItem::UnknownControlRequest {
                request_id: "abc-123".to_string()
            }]
        );
    }

    #[test]
    fn parses_results_including_interrupted() {
        assert_eq!(
            parse_stream_line(RESULT_LINE),
            vec![ChatStreamItem::TurnResult {
                is_error: false,
                result: Some("pong".to_string())
            }]
        );
        assert_eq!(
            parse_stream_line(INTERRUPTED_RESULT_LINE),
            vec![ChatStreamItem::TurnResult {
                is_error: true,
                result: None
            }]
        );
    }

    #[test]
    fn noise_and_garbage_parse_to_nothing() {
        for line in [
            STATUS_LINE,
            CONTROL_ACK_LINE,
            "not json at all",
            "{}",
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}"#,
        ] {
            assert!(parse_stream_line(line).is_empty(), "expected noise: {line}");
        }
    }

    #[test]
    fn user_message_encodes_the_stream_json_shape() {
        let value: Value = serde_json::from_str(&user_message_json("hi there")).unwrap();
        assert_eq!(value["type"], "user");
        assert_eq!(value["message"]["role"], "user");
        assert_eq!(value["message"]["content"][0]["type"], "text");
        assert_eq!(value["message"]["content"][0]["text"], "hi there");
    }

    #[test]
    fn permission_responses_encode_allow_and_deny() {
        let allow = permission_response_json(
            "req-1",
            &PermissionDecision::Allow {
                updated_input: json!({"term": "AEC"}),
            },
        );
        let value: Value = serde_json::from_str(&allow).unwrap();
        assert_eq!(value["type"], "control_response");
        assert_eq!(value["response"]["subtype"], "success");
        assert_eq!(value["response"]["request_id"], "req-1");
        assert_eq!(value["response"]["response"]["behavior"], "allow");
        assert_eq!(value["response"]["response"]["updatedInput"]["term"], "AEC");

        let deny = permission_response_json(
            "req-2",
            &PermissionDecision::Deny {
                message: "User declined in Kodabi".to_string(),
            },
        );
        let value: Value = serde_json::from_str(&deny).unwrap();
        assert_eq!(value["response"]["response"]["behavior"], "deny");
        assert_eq!(
            value["response"]["response"]["message"],
            "User declined in Kodabi"
        );
    }

    #[test]
    fn interrupt_and_ack_encode_control_shapes() {
        let value: Value = serde_json::from_str(&interrupt_request_json("stop-1")).unwrap();
        assert_eq!(value["type"], "control_request");
        assert_eq!(value["request_id"], "stop-1");
        assert_eq!(value["request"]["subtype"], "interrupt");

        let value: Value = serde_json::from_str(&control_ack_json("abc-123")).unwrap();
        assert_eq!(value["type"], "control_response");
        assert_eq!(value["response"]["request_id"], "abc-123");
    }

    #[test]
    fn tool_summaries_cover_known_tools_and_fall_back() {
        assert_eq!(
            tool_use_summary("mcp__kodabi__search_notes", &json!({"query": "budget"})),
            "Searched notes for \"budget\""
        );
        assert_eq!(
            tool_use_summary("mcp__kodabi__get_note", &json!({"id": "n_a1b2c3"})),
            "Opened note n_a1b2c3"
        );
        assert_eq!(
            tool_use_summary("mcp__kodabi__list_projects", &json!({})),
            "Listed projects"
        );
        assert_eq!(
            tool_use_summary(
                "mcp__kodabi__file_note_to_project",
                &json!({"id": "n_a1b2c3", "project": "Briarwood Golf"})
            ),
            "Filed note n_a1b2c3 to Briarwood Golf"
        );
        assert_eq!(
            tool_use_summary(
                "mcp__kodabi__add_glossary_term",
                &json!({"term": "AEC", "project": "Briarwood Golf"})
            ),
            "Added glossary term \"AEC\" to Briarwood Golf"
        );
        // Unknown tool (e.g. a later list_outstanding_items): truthful fallback.
        assert_eq!(
            tool_use_summary("mcp__kodabi__list_outstanding_items", &json!({})),
            "Used list_outstanding_items"
        );
    }

    #[test]
    fn permission_questions_read_as_questions() {
        assert_eq!(
            permission_question(
                "mcp__kodabi__file_note_to_project",
                Some("File Note To Project"),
                &json!({"id": "n_a1b2c3", "project": "Briarwood Golf"})
            ),
            "File note n_a1b2c3 to Briarwood Golf?"
        );
        assert_eq!(
            permission_question(
                "mcp__kodabi__add_glossary_term",
                Some("Add Glossary Term"),
                &json!({"term": "AEC", "project": "Briarwood Golf"})
            ),
            "Add glossary term \"AEC\" to Briarwood Golf?"
        );
        assert_eq!(
            permission_question("mcp__kodabi__delete_note", Some("Delete Note"), &json!({})),
            "Allow Delete Note?"
        );
        assert_eq!(
            permission_question("mcp__kodabi__delete_note", None, &json!({})),
            "Allow delete_note?"
        );
    }

    #[test]
    fn chat_record_roundtrips_through_serde() {
        let records = vec![
            ChatRecord::Meta {
                chat_id: "3c9a35a1".to_string(),
                model: "sonnet".to_string(),
                started_at: "2026-07-12T14:03:35Z".to_string(),
            },
            ChatRecord::User {
                ts: "2026-07-12T14:03:40Z".to_string(),
                text: "What projects do I have?".to_string(),
            },
            ChatRecord::ToolUse {
                ts: "2026-07-12T14:03:42Z".to_string(),
                tool: "mcp__kodabi__list_projects".to_string(),
                input: json!({}),
                summary: "Listed projects".to_string(),
            },
            ChatRecord::Permission {
                ts: "2026-07-12T14:03:50Z".to_string(),
                request_id: "req-1".to_string(),
                tool: "mcp__kodabi__add_glossary_term".to_string(),
                allowed: false,
                resolution: PermissionResolution::Cancelled,
            },
        ];
        for record in records {
            let line = serde_json::to_string(&record).unwrap();
            let back: ChatRecord = serde_json::from_str(&line).unwrap();
            assert_eq!(back, record);
        }
    }

    #[test]
    fn transcript_appends_and_reads_back_in_a_tempdir() {
        let kb = tempfile::tempdir().unwrap();
        let device = DeviceId::parse("k4m2xp7q").unwrap();
        let started = Utc.with_ymd_and_hms(2026, 7, 12, 14, 3, 35).unwrap();

        let transcript = ChatTranscript::create(kb.path(), &device, started).unwrap();
        assert!(transcript.path().starts_with(kb.path().join(CHATS_DIR)));
        assert_eq!(
            transcript.path().file_name().unwrap().to_str().unwrap(),
            "20260712T140335000Z-k4m2xp7q.jsonl"
        );

        let meta = ChatRecord::Meta {
            chat_id: "3c9a35a1".to_string(),
            model: "sonnet".to_string(),
            started_at: "2026-07-12T14:03:35Z".to_string(),
        };
        let user = ChatRecord::User {
            ts: "2026-07-12T14:03:40Z".to_string(),
            text: "hi".to_string(),
        };
        transcript.append(&meta).unwrap();
        transcript.append(&user).unwrap();

        assert_eq!(
            read_transcript(transcript.path()).unwrap(),
            vec![meta, user]
        );
    }

    #[test]
    fn transcript_resolves_same_instant_collisions_with_a_numbered_name() {
        let kb = tempfile::tempdir().unwrap();
        let device = DeviceId::parse("k4m2xp7q").unwrap();
        let started = Utc.with_ymd_and_hms(2026, 7, 12, 14, 3, 35).unwrap();

        let first = ChatTranscript::create(kb.path(), &device, started).unwrap();
        let second = ChatTranscript::create(kb.path(), &device, started).unwrap();

        assert_ne!(first.path(), second.path());
        assert_eq!(
            second.path().file_name().unwrap().to_str().unwrap(),
            "20260712T140335000Z-k4m2xp7q-2.jsonl"
        );
    }

    #[test]
    fn truncated_final_line_does_not_poison_a_transcript_read() {
        let kb = tempfile::tempdir().unwrap();
        let device = DeviceId::parse("k4m2xp7q").unwrap();
        let started = Utc.with_ymd_and_hms(2026, 7, 12, 14, 3, 35).unwrap();
        let transcript = ChatTranscript::create(kb.path(), &device, started).unwrap();

        let user = ChatRecord::User {
            ts: "2026-07-12T14:03:40Z".to_string(),
            text: "hi".to_string(),
        };
        transcript.append(&user).unwrap();
        // Simulate a crash mid-write: a dangling, unterminated JSON fragment.
        let mut file = OpenOptions::new()
            .append(true)
            .open(transcript.path())
            .unwrap();
        file.write_all(b"{\"type\":\"assistant\",\"ts\":\"2026")
            .unwrap();
        drop(file);

        assert_eq!(read_transcript(transcript.path()).unwrap(), vec![user]);
    }
}
