//! The quick-capture window: a global hotkey pops a small, chrome-less text box
//! whose contents route through the note pipeline exactly like a meeting note
//! (`docs/FOUNDING_DOC.md` §5). Inherently Tauri-coupled (AppHandle, window
//! show/hide, global shortcut, IPC), so the window control and the thin command
//! wrappers live here in the shell; the pure compose (route + write) is
//! [`kodabi_core::capture::quick_capture`].
//!
//! Naming note: the `capture` in `capture_control.rs` is *audio* capture — a
//! different feature. Everything here is prefixed `quick_capture` /
//! `quick-capture` so the two never blur together.

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};

use kodabi_core::capture::quick_capture;
use kodabi_core::note::{self, Note};
use tauri::{AppHandle, Emitter, Manager, Runtime, Window};
use tauri_plugin_global_shortcut::Shortcut;

use crate::routing_env::routing_config_from_env;
use crate::transcribe::knowledge_base_dir;

/// Label of the statically-configured quick-capture window
/// (`src-tauri/tauri.conf.json`). Pre-created hidden so a hotkey press shows it
/// instantly, with no webview cold start.
pub const WINDOW_LABEL: &str = "quick-capture";

/// Default global hotkey that pops the quick-capture window. OS-global — fires
/// even while Kodabi is unfocused. Deliberately clear of `Ctrl+Shift+K` (the
/// audio-capture toggle) and `Ctrl+K` (the in-app command palette). Not yet
/// user-configurable (there is no settings store yet).
pub const DEFAULT_QUICK_CAPTURE_SHORTCUT: &str = "Ctrl+Alt+Space";

/// Emitted app-wide after a successful quick-capture write so any open window
/// (the main window's note lists) can refetch — the frontend's own
/// `notifyVaultChanged` DOM bus is per-webview and can't cross windows.
pub const VAULT_CHANGED_EVENT: &str = "vault:changed";

/// Emitted to the capture window when it is shown, so its UI can refocus the
/// textarea and clear any stale flash/error left from a prior capture.
const SHOWN_EVENT: &str = "quick-capture:shown";

/// The routed note echoed back to the capture UI so it can flash where the note
/// landed before dismissing. `project: null` is the Inbox sentinel (matching the
/// `NoteSummary` projection), and `path` is KB-relative with forward slashes.
#[derive(serde::Serialize)]
pub struct QuickCaptureOutcome {
    id: String,
    path: String,
    project: Option<String>,
    confidence: f64,
}

/// Parse [`DEFAULT_QUICK_CAPTURE_SHORTCUT`] into a registerable [`Shortcut`].
pub fn default_quick_capture_shortcut() -> Shortcut {
    Shortcut::from_str(DEFAULT_QUICK_CAPTURE_SHORTCUT)
        .expect("DEFAULT_QUICK_CAPTURE_SHORTCUT is a valid accelerator")
}

/// Guards the quick-capture window's dismiss-on-blur against a Windows
/// foreground-stealing race. [`show_window`]'s `show()` + `set_focus()` can be
/// denied — or granted and immediately revoked — when Kodabi isn't the
/// foreground process, and the resulting spurious `Focused(false)` would hide
/// the window the instant it appeared (the hotkey looking like a no-op). So
/// blur-dismiss is *armed* only once the window genuinely gains focus
/// (`Focused(true)`); a blur before that is treated as the race and ignored.
/// Managed as Tauri state; the window-event handler in `lib.rs` drives it.
#[derive(Default)]
pub struct DismissArmed(AtomicBool);

impl DismissArmed {
    /// A real focus gain: the next blur is a genuine dismiss.
    fn arm(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Cancel any pending arm (on show, before focus is resolved).
    fn disarm(&self) {
        self.0.store(false, Ordering::SeqCst);
    }

    /// Consume the armed state, reporting whether a blur should dismiss.
    fn take(&self) -> bool {
        self.0.swap(false, Ordering::SeqCst)
    }
}

/// The quick-capture window genuinely gained focus (`Focused(true)`): arm
/// blur-dismiss so a later focus loss actually hides it.
pub fn arm_dismiss<R: Runtime>(window: &Window<R>) {
    window.state::<DismissArmed>().arm();
}

/// The quick-capture window lost focus (`Focused(false)`): hide it, but only if
/// it had truly gained focus since the last show — otherwise this is the
/// show/focus race, not the user clicking away, and hiding would swallow the
/// hotkey.
pub fn dismiss_on_focus_lost<R: Runtime>(window: &Window<R>) {
    if window.state::<DismissArmed>().take() {
        let _ = window.hide();
    }
}

/// Hotkey semantics: hidden → show + focus (start capturing), visible → hide
/// (dismiss). A no-op if the window isn't built yet (very early startup).
pub fn toggle_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window(WINDOW_LABEL) else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
    } else {
        show_window(app);
    }
}

/// Show and focus the capture window, and tell its UI it just came forward.
/// The tray menu item and the command-palette action both call this.
pub fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        // Disarm blur-dismiss until the window genuinely gains focus, so the
        // show/focus race can't hide it the instant it appears (see
        // [`DismissArmed`]). `Focused(true)` re-arms it.
        app.state::<DismissArmed>().disarm();
        let _ = window.show();
        let _ = window.set_focus();
        let _ = app.emit_to(WINDOW_LABEL, SHOWN_EVENT, ());
    }
}

/// Show the capture window (the command-palette / tray entry point).
#[tauri::command]
pub fn show_quick_capture(app: AppHandle) {
    show_window(&app);
}

/// Hide the capture window. Called by the frontend on Escape, on blur, and
/// after the post-submit flash — the draft survives because the webview stays
/// alive (hidden, not destroyed).
#[tauri::command]
pub fn hide_quick_capture(app: AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.hide();
    }
}

/// Route `text` through the note pipeline and write it. Returns where it landed
/// so the UI can flash the destination; does **not** hide the window (the
/// frontend hides after the flash). `async` like the note commands so the disk
/// work runs off the webview's main thread.
#[tauri::command]
pub async fn quick_capture_submit(
    app: AppHandle,
    text: String,
) -> Result<QuickCaptureOutcome, String> {
    submit_impl(&app, &text)
}

fn submit_impl(app: &AppHandle, text: &str) -> Result<QuickCaptureOutcome, String> {
    let kb = knowledge_base_dir(app)?;
    // Local date (not UTC): a jotted thought is dated in the user's own day, and
    // this matches the frontend's `todayIsoDate` convention. Date-only — there
    // is no meaningful clock time for a quick capture.
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();

    let captured = quick_capture(&kb, text, &date, &routing_config_from_env())
        .map_err(|err| err.to_string())?;

    // A broken glossary is contained to its own project (routing still ran); log
    // it so the user can fix the file. The capture itself succeeded.
    for failure in &captured.glossary_failures {
        eprintln!(
            "quick-capture: project \"{}\" has an unreadable glossary; it routes on its name only until fixed: {}",
            failure.project, failure.error
        );
    }

    // Broadcast to every window so the main window's lists refresh even while it
    // is hidden to the tray.
    let _ = app.emit(VAULT_CHANGED_EVENT, ());

    let rel = captured.path.strip_prefix(&kb).unwrap_or(&captured.path);
    Ok(outcome(&captured.note, rel))
}

/// Projects a written [`Note`] to the capture outcome: Inbox sentinel →
/// `project: null`, KB-relative path normalized to forward slashes. A
/// quick-captured note is always `Routing::Routed`, so `confidence` is present;
/// `unwrap_or(0.0)` is a defensive floor, never hit in practice.
fn outcome(note: &Note, rel_path: &std::path::Path) -> QuickCaptureOutcome {
    let project = note.routing.project();
    QuickCaptureOutcome {
        id: note.id.as_str().to_string(),
        path: rel_path.to_string_lossy().replace('\\', "/"),
        project: (project != note::INBOX).then(|| project.to_string()),
        confidence: note.routing.confidence().unwrap_or(0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kodabi_core::note::{NoteId, NoteType, Routing, Source};
    use std::path::Path;

    #[test]
    fn dismiss_arms_only_after_a_real_focus() {
        let armed = DismissArmed::default();
        // Fresh (or just-shown, disarmed): a blur before any focus is the
        // show/focus race — ignored.
        assert!(!armed.take());
        // A genuine `Focused(true)` arms exactly one dismiss.
        armed.arm();
        assert!(armed.take());
        assert!(!armed.take());
        // Disarm (on show) cancels a stale arm from a prior focus cycle.
        armed.arm();
        armed.disarm();
        assert!(!armed.take());
    }

    #[test]
    fn default_quick_capture_shortcut_parses() {
        // A typo in the constant should fail this test, not a runtime `.expect`.
        let _ = default_quick_capture_shortcut();
    }

    #[test]
    fn outcome_maps_inbox_to_null_and_normalizes_the_path() {
        let note = Note::new(
            NoteId::parse("n_a1b2c3").unwrap(),
            NoteType::Note,
            Routing::Routed {
                project: note::INBOX.to_string(),
                confidence: 0.38,
            },
            "2026-07-18",
            vec![],
            Source::parse("quick-capture").unwrap(),
            "a stray thought",
        )
        .unwrap();

        let dto = outcome(&note, Path::new("Inbox\\a-stray-thought.md"));
        assert_eq!(dto.id, "n_a1b2c3");
        assert_eq!(dto.project, None);
        assert_eq!(dto.confidence, 0.38);
        assert_eq!(dto.path, "Inbox/a-stray-thought.md");
    }

    #[test]
    fn outcome_keeps_a_real_project_and_its_confidence() {
        let note = Note::new(
            NoteId::parse("n_d4e5f6").unwrap(),
            NoteType::Note,
            Routing::Routed {
                project: "Briarwood Golf".to_string(),
                confidence: 0.5,
            },
            "2026-07-18",
            vec![],
            Source::parse("quick-capture").unwrap(),
            "irrigation bid",
        )
        .unwrap();

        let dto = outcome(&note, Path::new("Briarwood Golf/irrigation-bid.md"));
        assert_eq!(dto.project.as_deref(), Some("Briarwood Golf"));
        assert_eq!(dto.confidence, 0.5);
    }
}
