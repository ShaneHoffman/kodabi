//! The chat-sessions-as-documents pass's shell wiring: the background spawn
//! that turns an ended chat's transcript into a `type: chat` note, and the
//! startup sweep that catches the ones no hook could.
//!
//! The sibling of [`crate::distill_cmds`], sharing its two gates (the distill
//! lock and the in-flight claim set) so the two passes queue behind one
//! headless `claude` rather than racing. All real logic lives in
//! `kodabi_core::chat_distill`; this module resolves the KB root, builds the
//! runner, resolves the routing threshold, and reports progress.
//!
//! # Why a sweep and not just a hook
//!
//! A chat session ends three ways ([`crate::chat_cmds`]): the user restarts it,
//! the CLI dies on its own, or the app exits. The first two get a hook and
//! distill immediately. The third cannot — a thread spawned during
//! `RunEvent::Exit` dies with the process, and a distill is a call that can take
//! minutes. So app exit deliberately schedules nothing, and [`spawn_chat_sweep`]
//! picks the transcript up on the next launch. That also covers the case no hook
//! could: the app being killed mid-conversation.
//!
//! # Never distill a live transcript
//!
//! Every other failure here is retryable, because a failed distill leaves the
//! transcript untouched and the next sweep tries again. Distilling a chat that
//! is *still going* is the one that isn't: the note claims the transcript via
//! its `source:`, the sweep skips claimed transcripts forever, and the rest of
//! the conversation is never seen again. Hence two independent guards — the
//! live session's path is excluded outright, and [`CHAT_SWEEP_GRACE`] excludes
//! anything touched recently.

use std::path::{Path, PathBuf};
use std::time::Duration;

use kodabi_core::distill::{route_distilled, DistillError};
use kodabi_llm::{ClaudeConfig, ClaudeRunner};
use tauri::{AppHandle, Emitter, Manager};

use crate::chat_cmds::ChatState;
use crate::distill_cmds::{claim_in_flight, clear_in_flight, distill_lock, panic_message};
use crate::routing_env::{report_signal_failures, routing_config_from_env};
use crate::transcribe::knowledge_base_dir;

/// Event the frontend can subscribe to for chat-distill progress.
///
/// Its own channel rather than variants on `distill:state`: that channel drives
/// the meeting pass's label machine, so a chat finishing mid-meeting-distill
/// would overwrite the meeting's outcome with the chat's. Per-pipeline channels
/// are the house pattern (`transcription:state`, `distill:state`, `chat:event`).
pub const CHAT_DISTILL_STATE_EVENT: &str = "chat-distill:state";

/// Environment opt-out for the whole pass, honoured at the spawn boundary.
///
/// The meeting distill is feature-gated (`parakeet`/`whisper`), so a dev build
/// never spends a real headless call on it. Chat text is real in every build, so
/// without this every ended dev chat would cost a call. Set it to anything
/// non-empty to skip.
///
/// Sandbox mode (`KODABI_SANDBOX`) therefore sets this by default, so an
/// agent-driven preview never spends live calls distilling fixture
/// conversation. It only defaults it — an explicitly empty value reads as unset
/// here, which is how a sandboxed run turns distill back on. See
/// `crate::sandbox`.
const DISABLE_ENV: &str = "KODABI_DISABLE_CHAT_DISTILL";

/// How long a chat transcript must sit untouched before the sweep will treat it
/// as ended. Mirrors the in-flight spill's stale grace in spirit: generous,
/// because the cost of waiting is one deferred note and the cost of not waiting
/// is an unrecoverable half-distilled conversation.
const CHAT_SWEEP_GRACE: Duration = Duration::from_secs(120);

/// Payload for [`CHAT_DISTILL_STATE_EVENT`]. Tagged on `status`, mirroring
/// `distill:state`. Every terminal variant carries `chat_path` so a subscriber
/// can attribute an outcome to the right transcript.
///
/// `Skipped` is terminal and benign: the conversation never got far enough to
/// be worth a note (`kodabi_core::chat::has_distillable_substance`), which is a
/// non-event, not a failure.
#[derive(Clone, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum ChatDistillStateEvent {
    Distilling,
    RoutingFallback { message: String },
    Saved { path: String, chat_path: String },
    Skipped { reason: String, chat_path: String },
    Error { message: String, chat_path: String },
}

/// [`run`]'s failure split: a thin conversation is a benign skip, everything
/// else is a real failure.
enum ChatDistillFailure {
    Thin { reason: String },
    Other(String),
}

/// Queues the ended chat transcript at `chat_path` for distillation on a
/// background thread. Returns whether a run was actually queued: `false` means
/// the pass is disabled, or a run already holds this transcript.
///
/// Concurrent runs serialize on [`crate::distill_cmds::distill_lock`], shared
/// with the meeting pass.
#[must_use]
pub(crate) fn spawn_chat_distill(app: &AppHandle, chat_path: PathBuf) -> bool {
    if distill_disabled() {
        return false;
    }
    // Claim before the thread starts, so a sweep running alongside an
    // end-of-session hook can't queue the same transcript twice.
    if !claim_in_flight(&chat_path) {
        return false;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let _guard = distill_lock();
        let _ = app.emit(CHAT_DISTILL_STATE_EVENT, ChatDistillStateEvent::Distilling);
        // `catch_unwind` for the same reason the meeting pass uses it: a panic
        // between the two emits would leave subscribers stuck on "distilling"
        // and poison the shared lock.
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&app, &chat_path)));
        clear_in_flight(&chat_path);
        let event = match outcome {
            Ok(Ok(path)) => {
                // Deliberately **no** retention hook here. The meeting pass
                // calls `retention::apply_post_distill` at this point, and that
                // function would accept a chat path perfectly happily (it only
                // checks the `.jsonl` extension) — which is exactly why its
                // absence has to be stated rather than merely omitted.
                // `chats/` is outside the retention sweep on purpose: a chat
                // transcript is text the user wrote and read, and nothing else
                // preserves it (`CLAUDE_CODE_SKIP_PROMPT_HISTORY` means Claude
                // Code keeps no copy of its own).
                //
                // The distill wrote its note straight through kodabi-core, so
                // this is the only in-app announcement of that write; without
                // it the new note would wait on the file watcher.
                let _ = app.emit(crate::events::VAULT_CHANGED_EVENT, ());
                ChatDistillStateEvent::Saved {
                    path: path.display().to_string(),
                    chat_path: chat_path.display().to_string(),
                }
            }
            Ok(Err(ChatDistillFailure::Thin { reason })) => ChatDistillStateEvent::Skipped {
                reason,
                chat_path: chat_path.display().to_string(),
            },
            Ok(Err(ChatDistillFailure::Other(message))) => {
                eprintln!("chat distill failed: {message}");
                ChatDistillStateEvent::Error {
                    message,
                    chat_path: chat_path.display().to_string(),
                }
            }
            Err(panic) => {
                let message = format!("chat distill panicked: {}", panic_message(panic.as_ref()));
                eprintln!("{message}");
                ChatDistillStateEvent::Error {
                    message,
                    chat_path: chat_path.display().to_string(),
                }
            }
        };
        let _ = app.emit(CHAT_DISTILL_STATE_EVENT, event);
    });
    true
}

/// Distills every chat transcript no note claims yet, on a background thread.
///
/// Called once at startup, beside `transcribe::spawn_recovery`. This is the
/// only path that covers a chat ended by quitting the app or by a crash, and it
/// doubles as the retry mechanism for a distill that failed earlier: membership
/// is derived from disk every launch, so nothing has to be remembered.
pub(crate) fn spawn_chat_sweep(app: &AppHandle) {
    if distill_disabled() {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let Ok(kb) = knowledge_base_dir(&app) else {
            return;
        };
        let settled_before = chrono::Utc::now() - CHAT_SWEEP_GRACE;
        let chats = match kodabi_core::chats::list_undistilled_chats(&kb, settled_before) {
            Ok(chats) => chats,
            Err(err) => {
                eprintln!("chat sweep: could not list undistilled chats: {err}");
                return;
            }
        };
        let live = live_transcript(&app);
        for chat_path in chats {
            // The live session's transcript is still being appended to; a note
            // claiming it would strand the rest of the conversation. The mtime
            // grace above usually catches this already, but a session opened
            // between the two reads would slip past it.
            if live.as_deref() == Some(chat_path.as_path()) {
                continue;
            }
            // Each spawn serializes on the shared distill lock, so this queues
            // them rather than running them at once.
            let _ = spawn_chat_distill(&app, chat_path);
        }
    });
}

/// The live chat session's transcript path, when a session is open. `None` at
/// startup in the normal case — nothing calls `chat_open` until the chat view
/// mounts.
fn live_transcript(app: &AppHandle) -> Option<PathBuf> {
    let state = app.try_state::<ChatState>()?;
    let guard = state.0.lock().ok()?;
    guard.as_ref().and_then(|session| session.transcript_path())
}

/// Whether the pass is switched off for this process.
fn distill_disabled() -> bool {
    std::env::var_os(DISABLE_ENV).is_some_and(|value| !value.is_empty())
}

/// Resolves the KB root, builds the distill-configured runner, and runs the
/// pure `kodabi-core` pass. Routing goes through the same confidence-split
/// router the meeting pass uses, with the threshold resolved from the
/// environment at this boundary.
fn run(app: &AppHandle, chat_path: &Path) -> Result<PathBuf, ChatDistillFailure> {
    let kb = knowledge_base_dir(app).map_err(ChatDistillFailure::Other)?;
    let runner = ClaudeRunner::new(ClaudeConfig::distill_from_env());
    let config = routing_config_from_env();
    kodabi_core::chat_distill::distill_chat(&runner, &kb, chat_path, &|output, body| {
        let (routing, diagnostics) = route_distilled(&kb, output, body, &config);
        if let Some(err) = &diagnostics.discovery_failure {
            let message =
                format!("routing signals failed to load; filing this note to Inbox: {err}");
            eprintln!("chat distill: {message}");
            let _ = app.emit(
                CHAT_DISTILL_STATE_EVENT,
                ChatDistillStateEvent::RoutingFallback { message },
            );
        }
        report_signal_failures("chat", &diagnostics.glossary_failures);
        report_signal_failures("chat", &diagnostics.example_failures);
        routing
    })
    .map(|distilled| distilled.path)
    .map_err(|err| match err {
        DistillError::ThinChat { .. } => ChatDistillFailure::Thin {
            reason: err.to_string(),
        },
        other => ChatDistillFailure::Other(other.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire shape the frontend would switch on, pinned like
    /// `distill:state`'s. Terminal variants must carry their transcript.
    #[test]
    fn chat_distill_events_carry_their_chat_path() {
        let saved = serde_json::to_value(ChatDistillStateEvent::Saved {
            path: "Briarwood Golf/comparison.md".into(),
            chat_path: "chats/20260710T161500000Z-k4m2xp7q.jsonl".into(),
        })
        .unwrap();
        assert_eq!(saved["status"], "saved");
        assert_eq!(
            saved["chat_path"],
            "chats/20260710T161500000Z-k4m2xp7q.jsonl"
        );

        let skipped = serde_json::to_value(ChatDistillStateEvent::Skipped {
            reason: "too thin".into(),
            chat_path: "chats/thin.jsonl".into(),
        })
        .unwrap();
        assert_eq!(skipped["status"], "skipped");
        assert_eq!(skipped["chat_path"], "chats/thin.jsonl");

        let distilling = serde_json::to_value(ChatDistillStateEvent::Distilling).unwrap();
        assert_eq!(distilling["status"], "distilling");
    }
}
