mod audio_cmds;
mod capture_control;
mod capture_watchdog;
mod distill_cmds;
mod events;
mod index_cmds;
mod index_state;
mod note_cmds;
mod overlay;
mod quick_capture;
mod retention;
mod routing_env;
mod session_cmds;
mod settings_cmds;
mod transcribe;
mod tray_promotion;

use kodabi_core::device::DeviceId;
use tauri::Manager;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

/// The app's pre-rename bundle identifier. `app_config_dir()` resolves to
/// `<config-root>/<identifier>`, so device config from a Kodama-era install
/// lives under this sibling directory — see the migration in [`run`]'s setup.
const LEGACY_CONFIG_DIR: &str = "com.kodama.app";

#[tauri::command]
fn device_id(state: tauri::State<'_, DeviceId>) -> String {
    state.as_str().to_string()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let toggle_shortcut = capture_control::default_toggle_shortcut();
    let quick_capture_shortcut = quick_capture::default_quick_capture_shortcut();

    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    if event.state != ShortcutState::Pressed {
                        return;
                    }
                    if *shortcut == toggle_shortcut {
                        capture_control::toggle_capture(app);
                    } else if *shortcut == quick_capture_shortcut {
                        quick_capture::toggle_window(app);
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_notification::init())
        .setup(move |app| {
            let config_dir = app.path().app_config_dir()?;
            let device_config = config_dir.join("device.toml");
            // One-time migration for the Kodama → Kodabi rename: the bundle
            // identifier changed (com.kodama.app → com.kodabi.app), which moves
            // `app_config_dir()`. Adopt an existing device identity from the
            // legacy location so the rename doesn't silently reset it.
            // `app_config_dir()` is `<config-root>/<identifier>`, so the legacy
            // dir is the sibling named after the old identifier. Best-effort: a
            // failure here just falls through to `load_or_create` minting a
            // fresh id, exactly as before this migration.
            if let Some(config_root) = config_dir.parent() {
                let legacy_config = config_root.join(LEGACY_CONFIG_DIR).join("device.toml");
                if let Err(err) =
                    kodabi_core::device::migrate_legacy_config(&legacy_config, &device_config)
                {
                    eprintln!("failed to migrate legacy device config: {err}");
                }
            }
            let device_id = kodabi_core::device::load_or_create(&device_config)?;
            app.manage(device_id);

            // Load machine-local app settings (consent + retention policy) from
            // the same config dir, and manage them so the capture gate and the
            // retention schedule can read the current policy. A corrupt file
            // self-heals to defaults (consent unacknowledged) rather than
            // bricking startup.
            let settings_config = config_dir.join("settings.toml");
            let settings = kodabi_core::settings::load_or_create(&settings_config)?;
            app.manage(settings_cmds::SettingsState::new(settings_config, settings));

            // Open the note index (best-effort — a cache, never a launch
            // blocker) so the note commands can keep it in sync on write/edit.
            app.manage(index_state::IndexState::initialize(app.handle()));

            // Build the tray (which manages `CaptureController`) BEFORE
            // registering the shortcut, so a hotkey firing in the first
            // moments of launch can't reach the toggle before the controller
            // it depends on exists.
            capture_control::build_tray(app.handle())?;

            // Park the capture pill before anything can show it. Done here,
            // not on first show: the monitor query blocks on the main thread,
            // and first show happens under the capture toggle lock.
            overlay::place_initially(app.handle());

            // Windows drops every new tray icon into the hidden overflow, and
            // a mark behind a chevron can't be read at a glance. Lift it onto
            // the taskbar unless the user has already said otherwise. Runs in
            // the background: Explorer writes the entry it needs in response
            // to the icon being added, so it can't be there yet.
            match std::env::current_exe() {
                Ok(executable) => tray_promotion::promote_in_background(executable),
                Err(err) => eprintln!("could not resolve this executable's path: {err}"),
            }

            // Keep the indicators honest about state changes no toggle drives:
            // a device lost mid-meeting leaves the capture thread retrying
            // silently, which would otherwise show as "listening" forever.
            // Started after the tray, whose `CaptureController` it reads.
            capture_watchdog::start(app.handle());

            // A clashing OS-global shortcut must not prevent launch — the
            // tray toggle still works even if the hotkey couldn't bind. The two
            // shortcuts register independently: one clashing must not sink the
            // other, and the tray items (capture toggle, Quick capture) remain
            // as fallbacks for whichever failed.
            if let Err(err) = app.global_shortcut().register(toggle_shortcut) {
                eprintln!("failed to register global capture-toggle shortcut: {err}");
            }
            if let Err(err) = app.global_shortcut().register(quick_capture_shortcut) {
                eprintln!("failed to register global quick-capture shortcut: {err}");
            }

            // Start the retention schedule (an immediate sweep, then periodic)
            // now that the settings state it reads is managed.
            retention::start_schedule(app.handle());

            // Recover any capture session orphaned by a crash or kill
            // mid-meeting (transcribe + distill what was spilled to disk), and
            // sweep away un-recoverable leftovers. Detached, so it never blocks
            // launch; a clean shutdown leaves nothing to recover.
            transcribe::spawn_recovery(app.handle());
            Ok(())
        })
        .manage(audio_cmds::CaptureState::default())
        .manage(quick_capture::DismissArmed::default())
        // Managed at builder level so it exists before the first capture-state
        // broadcast can reach `overlay::sync`.
        .manage(overlay::OverlayController::default())
        .invoke_handler(tauri::generate_handler![
            device_id,
            audio_cmds::start_capture,
            audio_cmds::stop_capture,
            audio_cmds::capture_status,
            audio_cmds::capture_phase,
            audio_cmds::run_mic_test,
            distill_cmds::distill_session,
            distill_cmds::list_failed_sessions,
            session_cmds::read_session_artifacts,
            session_cmds::reveal_session_audio,
            note_cmds::write_note,
            note_cmds::list_notes,
            note_cmds::read_note,
            note_cmds::save_note,
            note_cmds::file_note_to_project,
            note_cmds::list_projects,
            index_cmds::rebuild_index,
            quick_capture::show_quick_capture,
            quick_capture::hide_quick_capture,
            quick_capture::quick_capture_submit,
            overlay::dismiss_capture_overlay,
            settings_cmds::get_settings,
            settings_cmds::set_retention_policy,
            settings_cmds::set_capture_overlay,
            settings_cmds::set_appearance,
            settings_cmds::acknowledge_consent,
        ])
        .on_window_event(|window, event| match event {
            // Hide instead of exit: the tray + global hotkey must stay
            // functional after the window closes. Full exit is only via the
            // tray's Quit item. Applies to every window, including quick-capture
            // (so Alt+F4 dismisses it rather than destroying its webview).
            tauri::WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
            }
            // The quick-capture window genuinely has focus now, so a later blur
            // is a real dismiss — not the show/focus race. Arm dismiss-on-blur.
            tauri::WindowEvent::Focused(true) if window.label() == quick_capture::WINDOW_LABEL => {
                quick_capture::arm_dismiss(window);
            }
            // The quick-capture window is a transient overlay: losing focus
            // (clicking elsewhere, Alt+Tab) dismisses it, but only once it had
            // truly gained focus (else the spurious blur from a denied
            // `set_focus` would hide it on arrival). The draft survives — the
            // webview is only hidden, never destroyed.
            tauri::WindowEvent::Focused(false) if window.label() == quick_capture::WINDOW_LABEL => {
                quick_capture::dismiss_on_focus_lost(window);
            }
            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// Proves src-tauri actually links kodabi-core (the data-layer dependency),
// exercised by `cargo test`. No runtime feature is added.
#[cfg(test)]
mod tests {
    #[test]
    fn depends_on_core() {
        assert!(!kodabi_core::version().is_empty());
    }
}
