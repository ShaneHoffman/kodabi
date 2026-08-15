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

use crate::index_state::IndexState;

/// Every chat lifecycle event, tagged payloads with the session's `chat_id`
/// on each variant. Mirrors `ChatEventPayload` in `src/chat.ts`.
pub const CHAT_EVENT: &str = "chat:event";

/// Coalesce window for streamed text deltas: batches token-rate emissions into
/// a few events per frame. Wider than the terminal's 8ms because prose reads
/// fine at 60Hz and the payload is re-rendered markdown, not raw bytes.
const COALESCE_MS: u64 = 16;
/// Flush the delta coalescer once this much text piled up regardless.
const MAX_DELTA_BYTES: usize = 8 * 1024;
/// Same reasoning as `terminal_cmds::POISONED`: a panicked holder leaves the
/// session unrecoverable, and restarting the chat is the way out.
const POISONED: &str = "Chat hit an internal error. Reopen this view to continue.";

/// Writing the user's message to the child failed. The interrupt and
/// permission-answer paths word their own, because what did not happen differs;
/// what they share is the reassurance, and it is this one: the session object is
/// still there, so the reader's next attempt is a real one.
const CHAT_SEND_FAILED: &str = "Couldn't send the message. The chat is still open; try again.";

/// Spawning or transcript setup failed. As with the terminal, naming `claude`
/// is the actionable half.
const CHAT_START_FAILED: &str =
    "Couldn't start the chat. Check that the claude command is installed, then reopen this view.";
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
    /// Set by `chat_send`, cleared at `TurnResult` and exit. The snapshot
    /// cannot infer an in-flight turn from its visible traces — while the
    /// model thinks or a read tool runs there is no streamed text and no
    /// pending card — so this flag is what keeps a mid-turn remount showing
    /// Stop instead of re-enabling Send.
    turn_active: AtomicBool,
    /// Set by `chat_cancel` so the pump renders the interrupted turn's error
    /// result as "stopped", not as a failure.
    interrupting: AtomicBool,
    /// Set by a deliberate reap (restart / app exit) so the pump stays silent
    /// instead of emitting a spurious `exited` event.
    reaped: AtomicBool,
    /// Claimed by whichever end path gets there first, so one session distills
    /// at most once. See [`claim_distill_once`].
    distill_scheduled: AtomicBool,
    /// Set by `chat_restart` before it reaps, so the pump distills this session
    /// once it has drained. See [`should_schedule_distill`].
    distill_on_reap: AtomicBool,
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
        let running = self.child.is_alive();
        ChatSnapshot {
            chat_id: self.chat_id.clone(),
            running,
            turn_active: running && self.shared.turn_active.load(Ordering::Relaxed),
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
    ///
    /// Deliberately schedules **no** distill, and deliberately does not wait
    /// for the pump: `kill` only stops the child, so events the reader thread
    /// has already parsed are still in flight and the transcript is not
    /// complete when this returns. Whether the session distills is recorded on
    /// [`SharedChat::distill_on_reap`] and acted on by the pump, which is the
    /// one place that knows every record has landed. `chat_restart` sets it
    /// (the app is live); app exit cannot distill at all, because a thread
    /// spawned during `RunEvent::Exit` dies with the process, so it leaves the
    /// flag clear and the startup sweep covers it.
    fn reap(&self) {
        self.deny_pending(None, PermissionResolution::SessionClosed);
        self.shared.reaped.store(true, Ordering::Relaxed);
        self.child.kill();
    }

    /// This session's transcript path, for the startup sweep's live-session
    /// exclusion. Cloned rather than borrowed: the path outlives the lock.
    ///
    /// `None` on a poisoned lock rather than an empty path: the caller compares
    /// this against every candidate transcript, and a default `PathBuf` matches
    /// none of them — it would silently drop the exclusion and let the sweep
    /// distill a conversation that is still open.
    pub(crate) fn transcript_path(&self) -> Option<std::path::PathBuf> {
        self.shared
            .transcript
            .lock()
            .ok()
            .map(|transcript| transcript.path().to_path_buf())
    }
}

/// Claims the one chat distill this session gets, returning `true` for the
/// first caller only.
///
/// Two end paths can fire for a single session: reaping the child on restart
/// also makes the pump observe an exit. Without this, one conversation would
/// spend two headless calls and land two suffix-disambiguated notes, which
/// nothing downstream dedupes.
fn claim_distill_once(flag: &AtomicBool) -> bool {
    !flag.swap(true, Ordering::SeqCst)
}

/// Whether the pump should distill the session it just saw end.
///
/// The pump is the only place that may make this call, because it is the only
/// place that runs *after* every queued event has been recorded. `kill` does
/// not drain the channel, so a caller that schedules a distill right after
/// reaping races the tail of the conversation into the transcript — and a note
/// that claims a half-written transcript is the one failure no later sweep can
/// repair, since the sweep skips claimed transcripts forever.
///
/// An unreaped exit is the CLI dying on its own, which always distills. A
/// reaped one distills only when the reaper asked for it: `chat_restart` does,
/// app exit does not (its thread would die with the process).
fn should_schedule_distill(reaped: bool, distill_on_reap: bool) -> bool {
    !reaped || distill_on_reap
}

/// Schedules the ended session's transcript for a chat distill, at most once.
fn schedule_chat_distill(app: &AppHandle, shared: &SharedChat) {
    if !claim_distill_once(&shared.distill_scheduled) {
        return;
    }
    let Ok(transcript) = shared.transcript.lock() else {
        return;
    };
    let _ = crate::chat_distill_cmds::spawn_chat_distill(app, transcript.path().to_path_buf());
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
        /// The note this call read, when it read one — the chat's citation
        /// chip. `None` for every other tool, and for a `get_note` the index
        /// couldn't resolve.
        note: Option<NoteRefDto>,
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

/// A note an answer drew on, enriched from the index when the call was seen.
/// Mirrors `ChatNoteRef` in `src/chat.ts`.
///
/// The title is a snapshot: a note renamed later still shows the title it had
/// when the answer read it. The `id` is what navigation uses, so the chip keeps
/// working regardless; re-resolving titles on every render would cost an index
/// read per chip to correct a label nobody is looking at.
#[derive(Clone, Serialize)]
pub struct NoteRefDto {
    id: String,
    title: String,
    /// The note's project, or `None` for one still sitting in the inbox.
    project: Option<String>,
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
    /// A turn is in flight (`chat_send` accepted, `TurnResult` not yet seen).
    /// Carried explicitly because mid-turn there may be nothing else to see:
    /// no streamed text while the model thinks, no card while a read tool runs.
    turn_active: bool,
    entries: Vec<ChatEntryDto>,
    streaming_text: Option<String>,
    pending_permission: Option<PendingPermissionDto>,
}

/// Mirrors `ChatEventPayload` in `src/chat.ts`.
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEventPayload {
    /// A coalesced run of streamed assistant text.
    Delta { chat_id: String, text: String },
    /// A completed assistant block, authoritative over the accumulated deltas.
    AssistantDone { chat_id: String, text: String },
    ToolUse {
        chat_id: String,
        summary: String,
        note: Option<NoteRefDto>,
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
    Exited { chat_id: String, code: Option<i32> },
}

/// The citation chip a tool call earns, if any: the note it read, resolved to a
/// title and project through `lookup`.
///
/// `lookup` is a parameter rather than a direct index call so the decision
/// (which tools cite, what a failed resolution does) stays testable without an
/// index — the caller supplies the real read.
fn tool_use_note_ref(
    tool_name: &str,
    input: &Value,
    lookup: impl FnOnce(&str) -> Option<(String, Option<String>)>,
) -> Option<NoteRefDto> {
    let id = chat::cited_note_id(tool_name, input)?;
    let (title, project) = lookup(id)?;
    Some(NoteRefDto {
        id: id.to_owned(),
        title,
        project,
    })
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
        .ok_or_else(|| {
            "The chat isn't running. Start a new chat to send this message.".to_string()
        })?;

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
        .map_err(|err| crate::user_errors::reported("chat_send", err, CHAT_SEND_FAILED))?;
    session.shared.turn_active.store(true, Ordering::Relaxed);
    // A cancel that raced the previous turn's natural end leaves a stale
    // interrupt intent; consumed here, it cannot relabel this turn's real
    // failure as "stopped".
    session.shared.interrupting.store(false, Ordering::Relaxed);
    Ok(())
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
        .map_err(|err| {
            crate::user_errors::reported(
                "chat_cancel",
                err,
                "Couldn't stop the turn. The chat is still open; try again.",
            )
        })
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
    if let Err(err) = session
        .child
        .write_line(chat::permission_response_json(&request_id, &decision))
    {
        // The decision never reached the child (it is exiting), so this must
        // not be recorded as the user's resolution. Put the card back for the
        // pump's exit handler, which owns recording a `session_closed` deny —
        // otherwise this race leaves the prompt with no transcript record.
        if let Ok(mut pending_slot) = session.shared.pending.lock() {
            *pending_slot = Some(pending);
        }
        return Err(crate::user_errors::reported(
            "chat_permission_respond",
            err,
            "Couldn't send your answer. The chat is still open; try again.",
        ));
    }

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
    // One guard across reap + spawn + store (the `chat_open` shape): released
    // mid-restart, a concurrent `chat_open` could store its own session in the
    // gap and have it overwritten here — leaking a live, unreapable
    // `claude → kodabi-mcp` pair.
    let mut guard = state.0.lock().map_err(|_| POISONED.to_string())?;
    if let Some(session) = guard.take() {
        // Marked before the reap, never scheduled here: the pump owns the
        // timing. Distilling on this thread would read the transcript while
        // the pump is still writing the last turn into it.
        session.shared.distill_on_reap.store(true, Ordering::SeqCst);
        session.reap();
    }
    let session = spawn_session(&app)?;
    let snapshot = session.snapshot();
    *guard = Some(session);
    Ok(snapshot)
}

/// Reaps the live session on true app exit. Called from the `RunEvent` hook in
/// `lib.rs` beside the terminal's reap — NOT from `CloseRequested`, which only
/// hides to tray.
///
/// Second caller: `updater_cmds::updater_prepare_install`, because the updater
/// exits the process from inside `Update::install()` and never reaches that
/// hook. Idempotent, so the two paths cannot double-reap.
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
        .ok_or_else(|| "Chat isn't ready yet. Restart Kodabi and try again.".to_string())?;

    let chat_id = uuid::Uuid::new_v4().to_string();
    let config = ChatProcessConfig::from_env();
    let started_at = chrono::Utc::now();

    let transcript = ChatTranscript::create(&kb_root, &device, started_at)
        .map_err(|err| crate::user_errors::reported("chat_open", err, CHAT_START_FAILED))?;
    transcript
        .append(&ChatRecord::Meta {
            chat_id: chat_id.clone(),
            model: config.model.clone(),
            started_at: started_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        })
        .map_err(|err| crate::user_errors::reported("chat_open", err, CHAT_START_FAILED))?;

    let (child, events) = spawn_chat(&config, &mcp_path, &chat_id, &kb_root)
        .map_err(|err| crate::user_errors::reported("chat_open", err, CHAT_START_FAILED))?;
    let child = Arc::new(child);

    let shared = Arc::new(SharedChat {
        log: Mutex::new(Vec::new()),
        streaming: Mutex::new(String::new()),
        pending: Mutex::new(None),
        transcript: Mutex::new(transcript),
        turn_active: AtomicBool::new(false),
        interrupting: AtomicBool::new(false),
        reaped: AtomicBool::new(false),
        distill_scheduled: AtomicBool::new(false),
        distill_on_reap: AtomicBool::new(false),
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
            // Resolved here, while the call is in hand: the entry it lands in
            // is what a later `chat_open` replays, so the snapshot carries the
            // same chip the live event did.
            let note = tool_use_note_ref(&name, &input, |id| {
                app.try_state::<IndexState>()
                    .and_then(|index| index.note_title_project(id))
            });
            shared.record(ChatRecord::ToolUse {
                ts: chat::now_ts(),
                tool: name,
                input,
                summary: summary.clone(),
            });
            shared.push_entry(ChatEntryDto::ToolUse {
                summary: summary.clone(),
                note: note.clone(),
            });
            emit(
                app,
                &ChatEventPayload::ToolUse {
                    chat_id: chat_id.to_owned(),
                    summary,
                    note,
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
            shared.turn_active.store(false, Ordering::Relaxed);
            let stopped = shared.interrupting.swap(false, Ordering::Relaxed) && is_error;
            let error = if is_error && !stopped {
                let detail = result.filter(|text| !text.trim().is_empty());
                // The transcript keeps what Claude actually said, unwrapped.
                shared.record(ChatRecord::Error {
                    ts: chat::now_ts(),
                    message: detail
                        .clone()
                        .unwrap_or_else(|| "the turn failed".to_owned()),
                });
                // One sentence for both renderings of this failure. The live
                // `turn_done` event and the snapshot log used to be worded
                // differently for the same turn, because the view prefixed the
                // event and rendered the restored entry bare. Claude's own text
                // is the detail rather than something to replace: it is written
                // for a reader ("usage limit reached").
                let shown = match detail {
                    Some(text) => format!("Couldn't finish the answer: {text}"),
                    None => "Couldn't finish the answer. Try sending the message again.".to_owned(),
                };
                shared.push_entry(ChatEntryDto::Error {
                    message: shown.clone(),
                });
                Some(shown)
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
    shared.turn_active.store(false, Ordering::Relaxed);
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
    let reaped = shared.reaped.load(Ordering::Relaxed);
    if !reaped {
        emit(
            app,
            &ChatEventPayload::Exited {
                chat_id: chat_id.to_owned(),
                code,
            },
        );
    }
    // Outside the `!reaped` guard, and last: the pump has drained every queued
    // event by now, so this is the first moment the transcript is complete.
    // See [`should_schedule_distill`] for why no other thread may decide this.
    if should_schedule_distill(reaped, shared.distill_on_reap.load(Ordering::SeqCst)) {
        schedule_chat_distill(app, shared);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The double-fire guard, tested on the flag alone — which is exactly why
    /// it is extracted: reaping a session also makes the pump observe an exit,
    /// so both end paths reach `schedule_chat_distill` for one conversation.
    #[test]
    fn a_session_schedules_at_most_one_chat_distill() {
        let scheduled = AtomicBool::new(false);

        assert!(claim_distill_once(&scheduled), "the first claim wins");
        assert!(!claim_distill_once(&scheduled), "the second is refused");
        assert!(!claim_distill_once(&scheduled));
    }

    /// The three end paths, as the pump sees them. Extracted and tested pure
    /// for the same reason as the claim above: the decision is reached on the
    /// pump thread, after the drain, and there is no way to observe that
    /// ordering from a test.
    #[test]
    fn only_the_pump_distills_and_only_when_the_reaper_asked() {
        // The CLI died on its own: the session really ended, so it distills.
        assert!(should_schedule_distill(false, false));

        // App exit reaped it. A thread spawned during `RunEvent::Exit` dies
        // with the process, so exit deliberately leaves this to the sweep.
        assert!(!should_schedule_distill(true, false));

        // `chat_restart` reaped it and asked for the distill. It runs here,
        // on the pump, rather than on the restarting thread — which would
        // race the last turn into the transcript it is about to read.
        assert!(should_schedule_distill(true, true));
    }

    #[test]
    fn separate_sessions_each_get_their_own_claim() {
        let first = AtomicBool::new(false);
        let second = AtomicBool::new(false);

        assert!(claim_distill_once(&first));
        assert!(claim_distill_once(&second));
    }

    /// A note the answer read becomes a chip, resolved through the index.
    #[test]
    fn reading_a_note_earns_a_citation() {
        let note = tool_use_note_ref(
            "mcp__kodabi__get_note",
            &serde_json::json!({"id": "n_a1b2c3"}),
            |id| {
                assert_eq!(id, "n_a1b2c3");
                Some((
                    "Deck railing material decision".to_owned(),
                    Some("Briarwood Golf".to_owned()),
                ))
            },
        )
        .expect("a resolved note cites");

        assert_eq!(note.id, "n_a1b2c3");
        assert_eq!(note.title, "Deck railing material decision");
        assert_eq!(note.project.as_deref(), Some("Briarwood Golf"));
    }

    /// An unresolvable id (no index this session, or a row the reconcile hasn't
    /// caught up to) drops the chip. The tool line still renders.
    #[test]
    fn an_unresolvable_note_drops_the_citation() {
        let note = tool_use_note_ref(
            "mcp__kodabi__get_note",
            &serde_json::json!({"id": "n_missing"}),
            |_| None,
        );

        assert!(note.is_none());
    }

    /// A non-citing tool never reaches the index at all.
    #[test]
    fn a_search_costs_no_index_read() {
        let note = tool_use_note_ref(
            "mcp__kodabi__search_notes",
            &serde_json::json!({"query": "budget"}),
            |_| panic!("a search must not resolve a note"),
        );

        assert!(note.is_none());
    }

    /// The wire shape the frontend mirrors: `note` is present on every tool-use
    /// entry, as an object or as an explicit null.
    #[test]
    fn tool_use_entries_always_carry_a_note_field() {
        let cited = serde_json::to_value(ChatEntryDto::ToolUse {
            summary: "Opened note n_a1b2c3".to_owned(),
            note: Some(NoteRefDto {
                id: "n_a1b2c3".to_owned(),
                title: "Deck railing material decision".to_owned(),
                project: None,
            }),
        })
        .unwrap();
        assert_eq!(cited["kind"], "tool_use");
        assert_eq!(cited["note"]["id"], "n_a1b2c3");
        assert_eq!(cited["note"]["title"], "Deck railing material decision");
        assert!(cited["note"]["project"].is_null());

        let uncited = serde_json::to_value(ChatEntryDto::ToolUse {
            summary: "Listed projects".to_owned(),
            note: None,
        })
        .unwrap();
        assert!(uncited["note"].is_null());
    }
}
