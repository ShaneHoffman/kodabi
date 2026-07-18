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

use kodabi_core::capture::quick_capture;
use kodabi_core::note::{self, Note};
use kodabi_core::routing::RoutingConfig;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::Shortcut;

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

/// Environment override for the routing threshold, applied at this src-tauri
/// boundary (its documented home — `kodabi_core::routing` takes config as a
/// parameter and never reads the environment itself).
const ROUTING_THRESHOLD_ENV: &str = "KODABI_ROUTING_THRESHOLD";

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

/// Resolve the routing config from the `KODABI_ROUTING_THRESHOLD` environment
/// variable, falling back to the default when it is unset or unparseable. An
/// out-of-range value is accepted here and clamped by
/// `RoutingConfig::effective_threshold` at use, keeping the intent visible.
fn routing_config_from_env() -> RoutingConfig {
    routing_config_from(std::env::var(ROUTING_THRESHOLD_ENV).ok().as_deref())
}

fn routing_config_from(raw: Option<&str>) -> RoutingConfig {
    match raw.and_then(|s| s.trim().parse::<f64>().ok()) {
        Some(threshold) => RoutingConfig { threshold },
        None => RoutingConfig::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kodabi_core::note::{NoteId, NoteType, Routing, Source};
    use kodabi_core::routing::DEFAULT_THRESHOLD;
    use std::path::Path;

    #[test]
    fn default_quick_capture_shortcut_parses() {
        // A typo in the constant should fail this test, not a runtime `.expect`.
        let _ = default_quick_capture_shortcut();
    }

    #[test]
    fn routing_config_reads_a_valid_override_and_falls_back_otherwise() {
        assert_eq!(routing_config_from(Some("0.4")).threshold, 0.4);
        assert_eq!(routing_config_from(Some(" 0.4 ")).threshold, 0.4);
        assert_eq!(routing_config_from(None).threshold, DEFAULT_THRESHOLD);
        assert_eq!(
            routing_config_from(Some("junk")).threshold,
            DEFAULT_THRESHOLD
        );
        assert_eq!(routing_config_from(Some("")).threshold, DEFAULT_THRESHOLD);
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
