//! The designed chat view's session orchestration (Phase 3, FOUNDING_DOC §4).
//!
//! The pure protocol — argv, stream parsing, stdin encoding, tool copy, and
//! the transcript — lives in `kodabi_core::chat`; the process spawn lives in
//! `kodabi_llm::chat`. This module owns only the inherently Tauri-bound parts:
//! resolving machine paths through `app.path()`, holding the one live session
//! in managed state, pumping parsed stream items into `chat:event` emissions
//! and transcript appends, and reaping the child tree on true app exit. It
//! follows `terminal_cmds.rs` (Tauri-coupled shell code) throughout.
//!
//! One live session at a time, held in [`ChatState`]. It survives hide-to-tray
//! and view switches (the view re-hydrates from [`chat_open`]'s structured
//! snapshot); it is reaped only on true app exit (the `RunEvent` hook in
//! `lib.rs`) or an explicit restart.
//!
//! Permission prompts: the spawn routes MCP write-tool requests onto the
//! stream as `can_use_tool` control requests. At most one can be pending at a
//! time (the CLI blocks on its own request), held in the shared session state
//! until the user answers the inline card, the turn is stopped, or the
//! session ends — and every non-answer path resolves to deny, so a lost
//! prompt can never become an allow.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};

use kodabi_core::chat::{
    self, ChatRecord, ChatStreamItem, ChatTranscript, PermissionDecision, PermissionResolution,
};
use kodabi_core::device::DeviceId;
use kodabi_llm::chat::{spawn_chat, ChatChild, ChatChildEvent, ChatProcessConfig};

/// Every chat lifecycle event, tagged payloads with the session's `chat_id`
/// on each variant. Mirrors `ChatEventPayload` in `src/chat.ts`.
pub const CHAT_EVENT: &str = "chat:event";

/// Coalesce window for streamed text deltas: batches token-rate emissions into
/// a few events per frame. Wider than the terminal's 8ms because prose reads
/// fine at 60Hz and the payload is re-rendered markdown, not raw bytes.
const COALESCE_MS: u64 = 16;
/// Flush the delta coalescer once this much text piled up regardless.
const MAX_DELTA_BYTES: usize = 8 * 1024;
const POISONED: &str = "chat state lock poisoned";
/// The deny message the model sees when the user declines a permission card
/// (or a stop/exit resolves the card for them).
const DENY_MESSAGE: &str = "The user declined this action in Kodabi.";

/// The one live chat session, or none. Managed at builder level like
/// `TerminalState`.
#[derive(Default)]
pub struct ChatState(pub Mutex<Option<ChatSessionState>>);

/// A pending `can_use_tool` request the UI has not answered yet.
struct PendingPermission {
    request_id: String,
    tool: String,
    input: Value,
    question: String,
}

/// State shared between the command handlers and the pump thread.
struct SharedChat {
    log: Mutex<Vec<ChatEntryDto>>,
    streaming: Mutex<String>,
    pending: Mutex<Option<PendingPermission>>,
    transcript: Mutex<ChatTranscript>,
    /// Set by `chat_cancel` so the pump renders the interrupted turn's error
    /// result as "stopped", not as a failure.
    interrupting: AtomicBool,
    /// Set by a deliberate reap (restart / app exit) so the pump stays silent
    /// instead of emitting a spurious `exited` event.
    reaped: AtomicBool,
}

/// A running chat session. The pump thread owns the event receiver; what
/// stays here is what the command handlers need.
pub struct ChatSessionState {
    chat_id: String,
    child: Arc<ChatChild>,
    shared: Arc<SharedChat>,
    /// Uniquifies interrupt control-request ids within the session.
    control_counter: AtomicU64,
}

impl ChatSessionState {
    fn snapshot(&self) -> ChatSnapshot {
        ChatSnapshot {
            chat_id: self.chat_id.clone(),
            running: self.child.is_alive(),
            entries: self
                .shared
                .log
                .lock()
                .map(|l| l.clone())
                .unwrap_or_default(),
            streaming_text: self
                .shared
                .streaming
                .lock()
                .ok()
                .filter(|s| !s.is_empty())
                .map(|s| s.clone()),
            pending_permission: self
                .shared
                .pending
                .lock()
                .ok()
                .and_then(|p| p.as_ref().map(PendingPermissionDto::from)),
        }
    }

    /// Resolves a pending permission as denied without user input (a stop, a
    /// restart, or app exit), best-effort: the deny is queued to the child if
    /// it still runs, and recorded in the transcript either way.
    fn deny_pending(&self, app: Option<&AppHandle>, resolution: PermissionResolution) {
        let Some(pending) = self.shared.pending.lock().ok().and_then(|mut p| p.take()) else {
            return;
        };
        let _ = self.child.write_line(chat::permission_response_json(
            &pending.request_id,
            &PermissionDecision::Deny {
                message: DENY_MESSAGE.to_owned(),
            },
        ));
        self.shared.record(ChatRecord::Permission {
            ts: chat::now_ts(),
            request_id: pending.request_id.clone(),
            tool: pending.tool.clone(),
            allowed: false,
            resolution,
        });
        self.shared
            .resolve_permission_entry(&pending.question, false, resolution);
        if let Some(app) = app {
            emit(
                app,
                &ChatEventPayload::PermissionResolved {
                    chat_id: self.chat_id.clone(),
                    request_id: pending.request_id,
                    allowed: false,
                    resolution,
                },
            );
        }
    }

    /// Deliberate teardown: deny any pending card, mark reaped (so the pump
    /// stays quiet), and kill the `claude → kodabi-mcp` tree.
    fn reap(&self) {
        self.deny_pending(None, PermissionResolution::SessionClosed);
        self.shared.reaped.store(true, Ordering::Relaxed);
        self.child.kill();
    }
}

impl SharedChat {
    /// Appends to the transcript, best-effort: a full disk must not take the
    /// live conversation down with it.
    fn record(&self, record: ChatRecord) {
        if let Ok(transcript) = self.transcript.lock() {
            let _ = transcript.append(&record);
        }
    }

    fn push_entry(&self, entry: ChatEntryDto) {
        if let Ok(mut log) = self.log.lock() {
            log.push(entry);
        }
    }

    /// Marks the log's permission entry for `question` resolved, so snapshots
    /// replay the collapsed card, not a live one.
    fn resolve_permission_entry(
        &self,
        question: &str,
        allowed: bool,
        resolution: PermissionResolution,
    ) {
        if let Ok(mut log) = self.log.lock() {
            if let Some(ChatEntryDto::Permission {
                allowed: entry_allowed,
                resolution: entry_resolution,
                ..
            }) = log.iter_mut().rev().find(|entry| {
                matches!(entry, ChatEntryDto::Permission { question: q, resolution: None, .. } if q == question)
            }) {
                *entry_allowed = Some(allowed);
                *entry_resolution = Some(resolution_str(resolution).to_owned());
            }
        }
    }
}

/// One rendered entry of the conversation log. Mirrors `ChatEntry` in
/// `src/chat.ts`.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatEntryDto {
    User {
        text: String,
    },
    Assistant {
        text: String,
    },
    ToolUse {
        summary: String,
    },
    /// A permission card. `resolution`/`allowed` are `None` while it waits
    /// (the live card itself is `ChatSnapshot::pending_permission`; this
    /// entry keeps its place in the flow) and set once resolved.
    Permission {
        question: String,
        tool: String,
        allowed: Option<bool>,
        resolution: Option<String>,
    },
    Error {
        message: String,
    },
}

/// Mirrors `PendingPermission` in `src/chat.ts`.
#[derive(Clone, Serialize)]
pub struct PendingPermissionDto {
    request_id: String,
    tool: String,
    question: String,
}

impl From<&PendingPermission> for PendingPermissionDto {
    fn from(pending: &PendingPermission) -> Self {
        Self {
            request_id: pending.request_id.clone(),
            tool: pending.tool.clone(),
            question: pending.question.clone(),
        }
    }
}

/// Mirrors `ChatSnapshot` in `src/chat.ts`. Seeds a freshly mounted chat
/// view, including mid-stream text and a still-open permission card.
#[derive(Clone, Serialize)]
pub struct ChatSnapshot {
    chat_id: String,
    running: bool,
    entries: Vec<ChatEntryDto>,
    streaming_text: Option<String>,
    pending_permission: Option<PendingPermissionDto>,
}

/// Mirrors `ChatEventPayload` in `src/chat.ts`.
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEventPayload {
    /// A coalesced run of streamed assistant text.
    Delta {
        chat_id: String,
        text: String,
    },
    /// A completed assistant block, authoritative over the accumulated deltas.
    AssistantDone {
        chat_id: String,
        text: String,
    },
    ToolUse {
        chat_id: String,
        summary: String,
    },
    PermissionRequest {
        chat_id: String,
        request_id: String,
        tool: String,
        question: String,
    },
    PermissionResolved {
        chat_id: String,
        request_id: String,
        allowed: bool,
        resolution: PermissionResolution,
    },
    /// End of a turn. `stopped` means the user interrupted it; `error` is a
    /// real failure message (never set for a stop).
    TurnDone {
        chat_id: String,
        stopped: bool,
        error: Option<String>,
    },
    /// The chat process exited (naturally — a restart/app-exit reap is silent).
    Exited {
        chat_id: String,
        code: Option<i32>,
    },
}

fn resolution_str(resolution: PermissionResolution) -> &'static str {
    match resolution {
        PermissionResolution::User => "user",
        PermissionResolution::Cancelled => "cancelled",
        PermissionResolution::SessionClosed => "session_closed",
    }
}

fn emit(app: &AppHandle, payload: &ChatEventPayload) {
    let _ = app.emit(CHAT_EVENT, payload);
}

/// Ensures a live session and returns a snapshot to hydrate the chat view.
/// Idempotent: reuses the running session so a view switch or hide-to-tray
/// does not restart the conversation; only a missing or dead session spawns.
#[tauri::command]
pub fn chat_open(app: AppHandle, state: State<'_, ChatState>) -> Result<ChatSnapshot, String> {
    let mut guard = state.0.lock().map_err(|_| POISONED.to_string())?;
    if let Some(session) = guard.as_ref() {
        if session.child.is_alive() {
            return Ok(session.snapshot());
        }
    }
    guard.take();
    let session = spawn_session(&app)?;
    let snapshot = session.snapshot();
    *guard = Some(session);
    Ok(snapshot)
}

/// Sends a user message: records it, mirrors it into the snapshot log, and
/// queues it to the process. The caller renders the message itself; the
/// answer streams back as `chat:event`s.
#[tauri::command]
pub fn chat_send(state: State<'_, ChatState>, text: String) -> Result<(), String> {
    let guard = state.0.lock().map_err(|_| POISONED.to_string())?;
    let session = guard
        .as_ref()
        .filter(|s| s.child.is_alive())
        .ok_or_else(|| "chat session is not running".to_string())?;

    session.shared.record(ChatRecord::User {
        ts: chat::now_ts(),
        text: text.clone(),
    });
    session
        .shared
        .push_entry(ChatEntryDto::User { text: text.clone() });
    session
        .child
        .write_line(chat::user_message_json(&text))
        .map_err(|e| e.to_string())
}

/// Stops the in-flight turn: any open permission card resolves to deny
/// (`cancelled`), then the interrupt goes to the process. The turn ends with
/// a `turn_done` whose `stopped` is true.
#[tauri::command]
pub fn chat_cancel(app: AppHandle, state: State<'_, ChatState>) -> Result<(), String> {
    let guard = state.0.lock().map_err(|_| POISONED.to_string())?;
    let Some(session) = guard.as_ref().filter(|s| s.child.is_alive()) else {
        return Ok(());
    };
    session.deny_pending(Some(&app), PermissionResolution::Cancelled);
    session.shared.interrupting.store(true, Ordering::Relaxed);
    let counter = session.control_counter.fetch_add(1, Ordering::Relaxed);
    session
        .child
        .write_line(chat::interrupt_request_json(&format!(
            "kodabi_interrupt_{counter}"
        )))
        .map_err(|e| e.to_string())
}

/// Answers the pending permission card. A stale `request_id` (already
/// resolved by a stop or exit) is a quiet no-op, so a race between the click
/// and a resolution never errors.
#[tauri::command]
pub fn chat_permission_respond(
    app: AppHandle,
    state: State<'_, ChatState>,
    request_id: String,
    allow: bool,
) -> Result<(), String> {
    let guard = state.0.lock().map_err(|_| POISONED.to_string())?;
    let Some(session) = guard.as_ref() else {
        return Ok(());
    };
    let Some(pending) = session
        .shared
        .pending
        .lock()
        .ok()
        .and_then(|mut p| match p.as_ref() {
            Some(pending) if pending.request_id == request_id => p.take(),
            _ => None,
        })
    else {
        return Ok(());
    };

    let decision = if allow {
        PermissionDecision::Allow {
            updated_input: pending.input.clone(),
        }
    } else {
        PermissionDecision::Deny {
            message: DENY_MESSAGE.to_owned(),
        }
    };
    session
        .child
        .write_line(chat::permission_response_json(&request_id, &decision))
        .map_err(|e| e.to_string())?;

    session.shared.record(ChatRecord::Permission {
        ts: chat::now_ts(),
        request_id: request_id.clone(),
        tool: pending.tool.clone(),
        allowed: allow,
        resolution: PermissionResolution::User,
    });
    session
        .shared
        .resolve_permission_entry(&pending.question, allow, PermissionResolution::User);
    emit(
        &app,
        &ChatEventPayload::PermissionResolved {
            chat_id: session.chat_id.clone(),
            request_id,
            allowed: allow,
            resolution: PermissionResolution::User,
        },
    );
    Ok(())
}

/// Reaps the current session (if any) and starts a fresh conversation with a
/// new transcript — the "Start a new chat" action after an exit, or a
/// deliberate reset.
#[tauri::command]
pub fn chat_restart(app: AppHandle, state: State<'_, ChatState>) -> Result<ChatSnapshot, String> {
    {
        let mut guard = state.0.lock().map_err(|_| POISONED.to_string())?;
        if let Some(session) = guard.take() {
            session.reap();
        }
    }
    let session = spawn_session(&app)?;
    let snapshot = session.snapshot();
    *state.0.lock().map_err(|_| POISONED.to_string())? = Some(session);
    Ok(snapshot)
}

/// Reaps the live session on true app exit. Called from the `RunEvent` hook in
/// `lib.rs` beside the terminal's reap — NOT from `CloseRequested`, which only
/// hides to tray.
pub fn reap(app: &AppHandle) {
    if let Some(state) = app.try_state::<ChatState>() {
        if let Ok(mut guard) = state.0.lock() {
            if let Some(session) = guard.take() {
                session.reap();
            }
        }
    }
}

/// Spawns the chat `claude` wired to the shared MCP config, creates the
/// transcript, and starts the pump thread.
fn spawn_session(app: &AppHandle) -> Result<ChatSessionState, String> {
    let mcp_path = crate::terminal_cmds::write_mcp_config(app)?;
    let kb_root = crate::transcribe::knowledge_base_dir(app)?;
    let device = app
        .try_state::<DeviceId>()
        .ok_or_else(|| "device identity not initialized".to_string())?;

    let chat_id = uuid::Uuid::new_v4().to_string();
    let config = ChatProcessConfig::from_env();
    let started_at = chrono::Utc::now();

    let transcript =
        ChatTranscript::create(&kb_root, &device, started_at).map_err(|e| e.to_string())?;
    transcript
        .append(&ChatRecord::Meta {
            chat_id: chat_id.clone(),
            model: config.model.clone(),
            started_at: started_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        })
        .map_err(|e| e.to_string())?;

    let (child, events) =
        spawn_chat(&config, &mcp_path, &chat_id, &kb_root).map_err(|e| e.to_string())?;
    let child = Arc::new(child);

    let shared = Arc::new(SharedChat {
        log: Mutex::new(Vec::new()),
        streaming: Mutex::new(String::new()),
        pending: Mutex::new(None),
        transcript: Mutex::new(transcript),
        interrupting: AtomicBool::new(false),
        reaped: AtomicBool::new(false),
    });

    let pump_app = app.clone();
    let pump_shared = Arc::clone(&shared);
    let pump_child = Arc::clone(&child);
    let pump_chat_id = chat_id.clone();
    std::thread::spawn(move || {
        run_pump(pump_app, pump_chat_id, events, pump_shared, pump_child);
    });

    Ok(ChatSessionState {
        chat_id,
        child,
        shared,
        control_counter: AtomicU64::new(1),
    })
}

/// Drains the child's events: coalesces text deltas into `delta` emissions,
/// turns completed items into log entries + transcript records + events, and
/// registers permission requests for the command handlers to answer.
fn run_pump(
    app: AppHandle,
    chat_id: String,
    events: mpsc::Receiver<ChatChildEvent>,
    shared: Arc<SharedChat>,
    child: Arc<ChatChild>,
) {
    let mut pending_delta = String::new();

    let flush_delta = |pending_delta: &mut String, shared: &SharedChat| {
        if pending_delta.is_empty() {
            return;
        }
        if let Ok(mut streaming) = shared.streaming.lock() {
            streaming.push_str(pending_delta);
        }
        emit(
            &app,
            &ChatEventPayload::Delta {
                chat_id: chat_id.clone(),
                text: std::mem::take(pending_delta),
            },
        );
    };

    loop {
        let event = match events.recv_timeout(Duration::from_millis(COALESCE_MS)) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => {
                flush_delta(&mut pending_delta, &shared);
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match event {
            ChatChildEvent::Item(ChatStreamItem::TextDelta { text }) => {
                pending_delta.push_str(&text);
                if pending_delta.len() >= MAX_DELTA_BYTES {
                    flush_delta(&mut pending_delta, &shared);
                }
            }
            ChatChildEvent::Item(item) => {
                flush_delta(&mut pending_delta, &shared);
                handle_item(&app, &chat_id, item, &shared, &child);
            }
            ChatChildEvent::Exited { code } => {
                flush_delta(&mut pending_delta, &shared);
                handle_exit(&app, &chat_id, code, &shared);
                break;
            }
        }
    }
}

fn handle_item(
    app: &AppHandle,
    chat_id: &str,
    item: ChatStreamItem,
    shared: &SharedChat,
    child: &ChatChild,
) {
    match item {
        ChatStreamItem::TextDelta { .. } => unreachable!("deltas are coalesced by the pump loop"),
        ChatStreamItem::Init { .. } => {}
        ChatStreamItem::AssistantText { text } => {
            if let Ok(mut streaming) = shared.streaming.lock() {
                streaming.clear();
            }
            shared.record(ChatRecord::Assistant {
                ts: chat::now_ts(),
                text: text.clone(),
            });
            shared.push_entry(ChatEntryDto::Assistant { text: text.clone() });
            emit(
                app,
                &ChatEventPayload::AssistantDone {
                    chat_id: chat_id.to_owned(),
                    text,
                },
            );
        }
        ChatStreamItem::ToolUse { name, input, .. } => {
            let summary = chat::tool_use_summary(&name, &input);
            shared.record(ChatRecord::ToolUse {
                ts: chat::now_ts(),
                tool: name,
                input,
                summary: summary.clone(),
            });
            shared.push_entry(ChatEntryDto::ToolUse {
                summary: summary.clone(),
            });
            emit(
                app,
                &ChatEventPayload::ToolUse {
                    chat_id: chat_id.to_owned(),
                    summary,
                },
            );
        }
        ChatStreamItem::PermissionRequest {
            request_id,
            tool_name,
            display_name,
            input,
        } => {
            let question = chat::permission_question(&tool_name, display_name.as_deref(), &input);
            if let Ok(mut pending) = shared.pending.lock() {
                *pending = Some(PendingPermission {
                    request_id: request_id.clone(),
                    tool: tool_name.clone(),
                    input,
                    question: question.clone(),
                });
            }
            shared.push_entry(ChatEntryDto::Permission {
                question: question.clone(),
                tool: tool_name.clone(),
                allowed: None,
                resolution: None,
            });
            emit(
                app,
                &ChatEventPayload::PermissionRequest {
                    chat_id: chat_id.to_owned(),
                    request_id,
                    tool: tool_name,
                    question,
                },
            );
        }
        ChatStreamItem::UnknownControlRequest { request_id } => {
            // Ack it generically: an unanswered control request could wedge
            // the CLI, and this build has nothing useful to say to it.
            let _ = child.write_line(chat::control_ack_json(&request_id));
        }
        ChatStreamItem::TurnResult { is_error, result } => {
            let stopped = shared.interrupting.swap(false, Ordering::Relaxed) && is_error;
            let error = if is_error && !stopped {
                let message = result
                    .filter(|text| !text.trim().is_empty())
                    .unwrap_or_else(|| "the turn failed".to_owned());
                shared.record(ChatRecord::Error {
                    ts: chat::now_ts(),
                    message: message.clone(),
                });
                shared.push_entry(ChatEntryDto::Error {
                    message: message.clone(),
                });
                Some(message)
            } else {
                None
            };
            emit(
                app,
                &ChatEventPayload::TurnDone {
                    chat_id: chat_id.to_owned(),
                    stopped,
                    error,
                },
            );
        }
    }
}

fn handle_exit(app: &AppHandle, chat_id: &str, code: Option<i32>, shared: &SharedChat) {
    // An exit with a card still open resolves it: record the deny (there is
    // no process left to write it to) so the audit trail stays truthful.
    if let Some(pending) = shared.pending.lock().ok().and_then(|mut p| p.take()) {
        shared.record(ChatRecord::Permission {
            ts: chat::now_ts(),
            request_id: pending.request_id.clone(),
            tool: pending.tool.clone(),
            allowed: false,
            resolution: PermissionResolution::SessionClosed,
        });
        shared.resolve_permission_entry(
            &pending.question,
            false,
            PermissionResolution::SessionClosed,
        );
        if !shared.reaped.load(Ordering::Relaxed) {
            emit(
                app,
                &ChatEventPayload::PermissionResolved {
                    chat_id: chat_id.to_owned(),
                    request_id: pending.request_id,
                    allowed: false,
                    resolution: PermissionResolution::SessionClosed,
                },
            );
        }
    }
    if !shared.reaped.load(Ordering::Relaxed) {
        emit(
            app,
            &ChatEventPayload::Exited {
                chat_id: chat_id.to_owned(),
                code,
            },
        );
    }
}
