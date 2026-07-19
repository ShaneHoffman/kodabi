//! The end-of-meeting distill pass's shell wiring: a thin command + the
//! background spawn that [`crate::transcribe`] chains after saving a raw
//! session. All real logic lives in `kodabi_core::distill`; this module only
//! resolves the KB root, builds the headless runner, resolves the routing
//! threshold from the environment (via [`crate::routing_env`]), and reports
//! progress — same shape as `transcribe.rs`'s pipeline wiring.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use kodabi_core::distill::{route_distilled, DistillError};
use kodabi_llm::{ClaudeConfig, ClaudeRunner};
use tauri::{AppHandle, Emitter, Manager};

use crate::routing_env::routing_config_from_env;
use crate::settings_cmds::SettingsState;
use crate::transcribe::knowledge_base_dir;

/// Event the frontend subscribes to for distill progress.
pub const DISTILL_STATE_EVENT: &str = "distill:state";

/// Payload for [`DISTILL_STATE_EVENT`]. Tagged on `status` so the frontend
/// can switch on that alone; `path`/`reason`/`message` only accompany their
/// matching variant (mirrors `transcription:state`). `Skipped` is terminal
/// like `Saved`/`Error` but benign: nothing distillable (a silent capture),
/// so no note — and no error to alarm anyone with.
///
/// `RoutingFallback` is the one *non-terminal* benign variant besides
/// `Distilling`: project discovery failed outright (the vault is unreadable), so
/// no project could be scored and the note files to Inbox, but the distill
/// itself succeeds. Emitted between `Distilling` and the terminal event; the
/// frontend logs it and leaves its terminal-state label machine untouched. (A
/// single project's malformed `_glossary.yml` does *not* land here: it is
/// contained to that project and merely logged, since routing still runs.)
#[derive(Clone, serde::Serialize)]
///
/// Every terminal variant carries `session_path`, so a list of retryable
/// sessions can attribute each outcome to the right row — a run that ends while
/// another session's retry is pending must not be mistaken for that retry
/// finishing. The transient indicator ignores the field.
#[serde(tag = "status", rename_all = "snake_case")]
enum DistillStateEvent {
    Distilling,
    RoutingFallback {
        message: String,
    },
    Saved {
        path: String,
        session_path: String,
    },
    Skipped {
        reason: String,
        session_path: String,
    },
    Error {
        message: String,
        session_path: String,
    },
}

/// Serializes distill runs so back-to-back sessions never hold two headless
/// Claude subprocesses at once. Independent of `transcribe.rs`'s lock on
/// purpose: a distill (a subprocess call) must not block the next meeting's
/// transcription (a model load), or vice versa.
static DISTILL_LOCK: Mutex<()> = Mutex::new(());

/// Sessions queued for or currently being distilled.
///
/// This is the one gate on distilling a session twice, and it guards both
/// directions:
///
/// - **Read** — [`list_failed_sessions`] subtracts it from what it reports. A
///   session on disk with no note yet is exactly what a *failed* distill looks
///   like, so without this the seconds between a capture landing and its
///   automatic distill finishing would surface the meeting as needing attention.
/// - **Write** — [`spawn_distill`] refuses a session already claimed. The read
///   filter alone can't be enough: a list fetched before a run started is stale
///   the moment it starts, and nothing announces a distill *beginning*, so a
///   click on that stale row would otherwise queue a second run. Two runs mean
///   two headless Claude calls and two suffix-disambiguated notes for one
///   meeting, which nothing downstream dedupes.
///
/// A set, not a list: a second claim is now rejected rather than stacked, so
/// there is never more than one claim per path to release.
///
/// Shell state on purpose: it describes runs this process has in hand, which is
/// not something the vault on disk can answer.
static DISTILLS_IN_FLIGHT: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

fn in_flight() -> MutexGuard<'static, BTreeSet<PathBuf>> {
    DISTILLS_IN_FLIGHT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Claims `session_path` for a distill run. `false` when a run already holds
/// it, in which case the caller must not start another.
fn claim_in_flight(session_path: &Path) -> bool {
    in_flight().insert(session_path.to_path_buf())
}

/// Releases the claim on `session_path`. A no-op when nothing held it.
fn clear_in_flight(session_path: &Path) {
    in_flight().remove(session_path);
}

/// Queues the raw session at `session_path` for distillation. Validation is
/// synchronous (a bad path fails the IPC call directly); the distill itself
/// runs on a background thread and reports through [`DISTILL_STATE_EVENT`].
///
/// **Manual retry/backfill only.** Every freshly transcribed session is
/// already distilled automatically — [`crate::transcribe`] chains
/// [`spawn_distill`] right after emitting its `Saved` event — and nothing
/// dedupes a session distilled twice: a second run spends a second headless
/// Claude call and writes a second, suffix-disambiguated note. Call this only
/// for a session whose distill failed or was skipped (or, in a mock-engine
/// build, never auto-ran).
#[tauri::command]
pub fn distill_session(app: AppHandle, session_path: String) -> Result<(), String> {
    let kb = knowledge_base_dir(&app)?;
    let path = validate_session_path(&kb, &session_path)?;
    if !spawn_distill(&app, path) {
        // A stale row the user clicked after a run for it already started.
        // Refusing is what keeps one meeting from becoming two notes.
        return Err("That session is already being distilled.".to_string());
    }
    Ok(())
}

/// One session needing attention, in the shape the frontend lists. `path` is
/// absolute — the exact value [`distill_session`] takes back for a retry.
/// Mirrors `kodabi_core::sessions::FailedSession`.
#[derive(serde::Serialize)]
pub struct FailedSessionDto {
    path: String,
    file_name: String,
    slug: Option<String>,
    captured_at: String,
}

/// Lists captured sessions that never became a note: a distill that failed, or
/// one the app died in the middle of. Runs currently queued or in progress are
/// excluded, so a meeting only appears once there is genuinely nothing coming
/// for it. Silent captures never appear at all.
#[tauri::command]
pub async fn list_failed_sessions(app: AppHandle) -> Result<Vec<FailedSessionDto>, String> {
    let kb = knowledge_base_dir(&app)?;
    let sessions = kodabi_core::sessions::list_failed_sessions(&kb).map_err(|e| e.to_string())?;
    // Take the claim list once for the whole filter rather than per session.
    let queued = in_flight();
    Ok(sessions
        .into_iter()
        .filter(|session| !queued.contains(&session.path))
        .map(|session| FailedSessionDto {
            path: session.path.display().to_string(),
            file_name: session.file_name,
            slug: session.slug,
            captured_at: session.captured_at,
        })
        .collect())
}

/// Spawns the distill on a background thread and returns immediately, so
/// neither the IPC call above nor the tail of the transcription pipeline
/// (which chains here after `Saved`) ever blocks on the headless Claude
/// call. Concurrent runs are serialized on [`DISTILL_LOCK`].
///
/// Returns whether a run was actually queued: `false` means [`DISTILLS_IN_FLIGHT`]
/// already holds this session, so a second run would duplicate its note.
#[must_use]
pub(crate) fn spawn_distill(app: &AppHandle, session_path: PathBuf) -> bool {
    // Claim the session before the thread starts, not inside it: the claim has
    // to cover the queued window too, or a list refetched between this call and
    // the run reaching the front of the lock would offer a retry for a session
    // already on its way.
    if !claim_in_flight(&session_path) {
        return false;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let _guard = DISTILL_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Announce `Distilling` only once this run actually starts (after the
        // lock is held), for the same reason `transcribe.rs` does: a queued
        // run must not overwrite the previous run's final state early.
        let _ = app.emit(DISTILL_STATE_EVENT, DistillStateEvent::Distilling);
        // `catch_unwind` so even a panic inside the distill yields a terminal
        // event: an unwinding thread would otherwise die between the
        // `Distilling` emit above and the emit below, leaving subscribers
        // stuck on "distilling" forever (and poisoning the lock).
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&app, &session_path)));
        // Release the claim before anything is announced below: both emits
        // prompt a refetch of the needs-attention list, and that refetch has to
        // see this run finished — otherwise a session that just failed would be
        // filtered out of the very list meant to surface it.
        clear_in_flight(&session_path);
        let event = match outcome {
            Ok(Ok(path)) => {
                // The session distilled into a note — enforce a
                // discard-after-distill retention policy now, on this thread,
                // still holding DISTILL_LOCK so nothing is reading the file.
                // Best-effort: a failed delete never turns a successful distill
                // into an error the user sees.
                apply_retention_after_distill(&app, &session_path);
                // The distill wrote its note straight through kodabi-core
                // rather than the note commands, so this is the only in-app
                // announcement of that write; without it the new note (and the
                // cleared needs-attention row) would wait on the file watcher.
                let _ = app.emit(crate::events::VAULT_CHANGED_EVENT, ());
                DistillStateEvent::Saved {
                    path: path.display().to_string(),
                    session_path: session_path.display().to_string(),
                }
            }
            Ok(Err(DistillFailure::EmptyTranscript { reason })) => DistillStateEvent::Skipped {
                reason,
                session_path: session_path.display().to_string(),
            },
            Ok(Err(DistillFailure::Other(message))) => {
                eprintln!("distill pipeline failed: {message}");
                DistillStateEvent::Error {
                    message,
                    session_path: session_path.display().to_string(),
                }
            }
            Err(panic) => {
                let message = format!("distill panicked: {}", panic_message(panic.as_ref()));
                eprintln!("{message}");
                DistillStateEvent::Error {
                    message,
                    session_path: session_path.display().to_string(),
                }
            }
        };
        let _ = app.emit(DISTILL_STATE_EVENT, event);
    });
    true
}

/// Applies the retention policy to a just-distilled session: under
/// [`kodabi_core::settings::RetentionPolicy::DiscardAfterDistill`] the raw
/// `.jsonl` is deleted now that its note exists. A no-op under every other
/// policy. Missing settings state (very early startup) or a delete failure is
/// logged and swallowed — the distill already succeeded.
fn apply_retention_after_distill(app: &AppHandle, session_path: &Path) {
    let Some(state) = app.try_state::<SettingsState>() else {
        return;
    };
    let policy = state.snapshot().retention;
    if let Err(err) = kodabi_core::retention::apply_post_distill(policy, session_path) {
        eprintln!(
            "retention: failed to discard distilled session {}: {err}",
            session_path.display()
        );
    }
}

/// [`run`]'s failure split: an empty transcript is a benign skip — a silent
/// capture is not an error — while everything else is a real failure worth
/// surfacing as one.
enum DistillFailure {
    EmptyTranscript { reason: String },
    Other(String),
}

/// Best-effort extraction of a panic's payload message; panics carry
/// `&str`/`String` payloads in practice (`panic!` with a literal or format).
fn panic_message(panic: &(dyn std::any::Any + Send)) -> &str {
    panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown panic payload")
}

/// Resolves the KB root, builds the distill-configured runner, and runs the
/// pure `kodabi-core` pipeline. Routing goes through the confidence-split
/// router (`route_distilled`) with the threshold resolved from the environment
/// at this boundary; if signals can't load it fails soft to Inbox and emits a
/// [`DistillStateEvent::RoutingFallback`] warning rather than losing the note.
/// Errors collapse to a message string — the house IPC/event convention —
/// except the empty-transcript case, which stays typed so [`spawn_distill`]
/// can report it as a skip rather than a failure.
fn run(app: &AppHandle, session_path: &Path) -> Result<PathBuf, DistillFailure> {
    let kb = knowledge_base_dir(app).map_err(DistillFailure::Other)?;
    let runner = ClaudeRunner::new(ClaudeConfig::distill_from_env());
    let config = routing_config_from_env();
    kodabi_core::distill::distill_session(&runner, &kb, session_path, &|output, body| {
        let (routing, diagnostics) = route_distilled(&kb, output, body, &config);
        // Discovery failed outright: the note fell back to Inbox. Warn the UI so
        // the misrouting is visible, not silent.
        if let Some(err) = &diagnostics.discovery_failure {
            let message =
                format!("routing signals failed to load; filing this note to Inbox: {err}");
            eprintln!("distill: {message}");
            let _ = app.emit(
                DISTILL_STATE_EVENT,
                DistillStateEvent::RoutingFallback { message },
            );
        }
        // A single project's glossary is broken but routing still ran (contained
        // to that project). Log it so the user can fix the file; the note landed
        // wherever the surviving signals sent it, so this is not a fallback.
        for failure in &diagnostics.glossary_failures {
            eprintln!(
                "distill: project \"{}\" has an unreadable glossary; it routes on its name only until fixed: {}",
                failure.project, failure.error
            );
        }
        routing
    })
    .map(|distilled| distilled.path)
    .map_err(|err| match err {
        DistillError::EmptyTranscript => DistillFailure::EmptyTranscript {
            reason: err.to_string(),
        },
        other => DistillFailure::Other(other.to_string()),
    })
}

/// Validates an IPC-supplied session path: the absolute path of a `.jsonl`
/// file directly inside `<kb>/sessions` — the shape the `transcription:state`
/// `Saved` event reports, which is where a frontend caller gets it. Purely
/// lexical (no filesystem access): traversal segments and out-of-tree paths
/// are rejected here; a missing file surfaces later as the distill's own
/// read error.
fn validate_session_path(kb: &Path, raw: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(raw);
    if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
        return Err(format!("not a .jsonl session file: {raw}"));
    }
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(format!(
            "session path must not contain '.' or '..' segments: {raw}"
        ));
    }
    let sessions = kb.join("sessions");
    if path.parent() != Some(sessions.as_path()) {
        return Err(format!(
            "session path must be directly inside {}",
            sessions.display()
        ));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_fallback_event_serializes_with_a_snake_case_status_tag() {
        // The frontend's `useDistillState` switches on this exact wire shape;
        // lock it so a rename can't silently break the pass-through filter.
        let value = serde_json::to_value(DistillStateEvent::RoutingFallback {
            message: "boom".into(),
        })
        .unwrap();

        assert_eq!(
            value,
            serde_json::json!({ "status": "routing_fallback", "message": "boom" })
        );
    }

    #[test]
    fn every_terminal_event_carries_its_session_path() {
        // The needs-attention list attributes each outcome to a row by
        // `session_path`. Without it on *every* terminal variant, a `saved` for
        // one session reads as "the retry I'm waiting on finished" and re-arms
        // the Retry button for a run that is still going.
        let session_path = "kb/sessions/s.jsonl";
        let cases = [
            (
                DistillStateEvent::Saved {
                    path: "kb/Inbox/note.md".into(),
                    session_path: session_path.into(),
                },
                serde_json::json!({
                    "status": "saved",
                    "path": "kb/Inbox/note.md",
                    "session_path": session_path,
                }),
            ),
            (
                DistillStateEvent::Skipped {
                    reason: "transcript is empty".into(),
                    session_path: session_path.into(),
                },
                serde_json::json!({
                    "status": "skipped",
                    "reason": "transcript is empty",
                    "session_path": session_path,
                }),
            ),
            (
                DistillStateEvent::Error {
                    message: "claude exited 1".into(),
                    session_path: session_path.into(),
                },
                serde_json::json!({
                    "status": "error",
                    "message": "claude exited 1",
                    "session_path": session_path,
                }),
            ),
        ];

        for (event, expected) in cases {
            assert_eq!(serde_json::to_value(event).unwrap(), expected);
        }
    }

    #[test]
    fn a_session_can_only_be_claimed_once_at_a_time() {
        // The claim is the only thing standing between a stale Retry row and a
        // second headless Claude call writing a duplicate note.
        let path = PathBuf::from("kb/sessions/in-flight-test.jsonl");

        assert!(claim_in_flight(&path), "first claim should be granted");
        assert!(
            !claim_in_flight(&path),
            "a session already being distilled must not be claimed again"
        );

        clear_in_flight(&path);
        assert!(
            claim_in_flight(&path),
            "the path is claimable again once its run released it"
        );

        clear_in_flight(&path);
        // Releasing something never claimed is a no-op, not a panic.
        clear_in_flight(&path);
        assert!(!in_flight().contains(&path));
    }

    #[test]
    fn accepts_a_jsonl_directly_inside_the_sessions_dir() {
        let kb = PathBuf::from("kb");
        let good = kb.join("sessions").join("s.jsonl");

        let validated = validate_session_path(&kb, good.to_str().unwrap()).unwrap();

        assert_eq!(validated, good);
    }

    #[test]
    fn rejects_a_non_jsonl_file() {
        let kb = PathBuf::from("kb");
        let bad = kb.join("sessions").join("s.md");

        assert!(validate_session_path(&kb, bad.to_str().unwrap()).is_err());
    }

    #[test]
    fn rejects_traversal_segments() {
        let kb = PathBuf::from("kb");
        let sneaky = kb.join("sessions").join("..").join("secrets.jsonl");

        assert!(validate_session_path(&kb, sneaky.to_str().unwrap()).is_err());
    }

    #[test]
    fn rejects_a_path_outside_or_nested_below_the_sessions_dir() {
        let kb = PathBuf::from("kb");
        let outside = PathBuf::from("elsewhere").join("s.jsonl");
        let nested = kb.join("sessions").join("sub").join("s.jsonl");

        assert!(validate_session_path(&kb, outside.to_str().unwrap()).is_err());
        assert!(validate_session_path(&kb, nested.to_str().unwrap()).is_err());
    }
}
