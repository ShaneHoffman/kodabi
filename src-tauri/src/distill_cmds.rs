//! The end-of-meeting distill pass's shell wiring: a thin command + the
//! background spawn that [`crate::transcribe`] chains after saving a raw
//! session. All real logic lives in `kodabi_core::distill`; this module only
//! resolves the KB root, builds the headless runner, and reports progress —
//! same shape as `transcribe.rs`'s pipeline wiring.

use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use kodabi_core::distill::{inbox_routing, DistillError};
use kodabi_llm::{ClaudeConfig, ClaudeRunner};
use tauri::{AppHandle, Emitter, Manager};

use crate::settings_cmds::SettingsState;
use crate::transcribe::knowledge_base_dir;

/// Event the frontend subscribes to for distill progress.
pub const DISTILL_STATE_EVENT: &str = "distill:state";

/// Payload for [`DISTILL_STATE_EVENT`]. Tagged on `status` so the frontend
/// can switch on that alone; `path`/`reason`/`message` only accompany their
/// matching variant (mirrors `transcription:state`). `Skipped` is terminal
/// like `Saved`/`Error` but benign: nothing distillable (a silent capture),
/// so no note — and no error to alarm anyone with.
#[derive(Clone, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum DistillStateEvent {
    Distilling,
    Saved { path: String },
    Skipped { reason: String },
    Error { message: String },
}

/// Serializes distill runs so back-to-back sessions never hold two headless
/// Claude subprocesses at once. Independent of `transcribe.rs`'s lock on
/// purpose: a distill (a subprocess call) must not block the next meeting's
/// transcription (a model load), or vice versa.
static DISTILL_LOCK: Mutex<()> = Mutex::new(());

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
    spawn_distill(&app, path);
    Ok(())
}

/// Spawns the distill on a background thread and returns immediately, so
/// neither the IPC call above nor the tail of the transcription pipeline
/// (which chains here after `Saved`) ever blocks on the headless Claude
/// call. Concurrent runs are serialized on [`DISTILL_LOCK`].
pub(crate) fn spawn_distill(app: &AppHandle, session_path: PathBuf) {
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
        let event = match outcome {
            Ok(Ok(path)) => {
                // The session distilled into a note — enforce a
                // discard-after-distill retention policy now, on this thread,
                // still holding DISTILL_LOCK so nothing is reading the file.
                // Best-effort: a failed delete never turns a successful distill
                // into an error the user sees.
                apply_retention_after_distill(&app, &session_path);
                DistillStateEvent::Saved {
                    path: path.display().to_string(),
                }
            }
            Ok(Err(DistillFailure::EmptyTranscript { reason })) => {
                DistillStateEvent::Skipped { reason }
            }
            Ok(Err(DistillFailure::Other(message))) => {
                eprintln!("distill pipeline failed: {message}");
                DistillStateEvent::Error { message }
            }
            Err(panic) => {
                let message = format!("distill panicked: {}", panic_message(panic.as_ref()));
                eprintln!("{message}");
                DistillStateEvent::Error { message }
            }
        };
        let _ = app.emit(DISTILL_STATE_EVENT, event);
    });
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
/// pure `kodabi-core` pipeline. Routing is the pre-#43 Inbox placeholder.
/// Errors collapse to a message string — the house IPC/event convention —
/// except the empty-transcript case, which stays typed so [`spawn_distill`]
/// can report it as a skip rather than a failure.
fn run(app: &AppHandle, session_path: &Path) -> Result<PathBuf, DistillFailure> {
    let kb = knowledge_base_dir(app).map_err(DistillFailure::Other)?;
    let runner = ClaudeRunner::new(ClaudeConfig::distill_from_env());
    kodabi_core::distill::distill_session(&runner, &kb, session_path, &|_| inbox_routing())
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
