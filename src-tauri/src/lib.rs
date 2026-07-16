mod audio_cmds;
mod capture_control;
mod transcribe;

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

    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    if *shortcut == toggle_shortcut && event.state == ShortcutState::Pressed {
                        capture_control::toggle_capture(app);
                    }
                })
                .build(),
        )
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

            // Build the tray (which manages `CaptureController`) BEFORE
            // registering the shortcut, so a hotkey firing in the first
            // moments of launch can't reach the toggle before the controller
            // it depends on exists.
            capture_control::build_tray(app.handle())?;

            // A clashing OS-global shortcut must not prevent launch — the
            // tray toggle still works even if the hotkey couldn't bind.
            if let Err(err) = app.global_shortcut().register(toggle_shortcut) {
                eprintln!("failed to register global capture-toggle shortcut: {err}");
            }
            Ok(())
        })
        .manage(audio_cmds::CaptureState::default())
        .invoke_handler(tauri::generate_handler![
            device_id,
            audio_cmds::start_capture,
            audio_cmds::stop_capture,
            audio_cmds::capture_status,
            audio_cmds::capture_phase,
        ])
        .on_window_event(|window, event| {
            // Hide instead of exit: the tray + global hotkey must stay
            // functional after the window closes. Full exit is only via the
            // tray's Quit item.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
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
