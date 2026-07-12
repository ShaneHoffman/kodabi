use kodama_core::device::DeviceId;
use tauri::Manager;

#[tauri::command]
fn device_id(state: tauri::State<'_, DeviceId>) -> String {
    state.as_str().to_string()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let device_id = kodama_core::device::load_or_create(&config_dir.join("device.toml"))?;
            app.manage(device_id);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![device_id])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// Proves src-tauri actually links kodama-core (the data-layer dependency),
// exercised by `cargo test`. No runtime feature is added.
#[cfg(test)]
mod tests {
    #[test]
    fn depends_on_core() {
        assert!(!kodama_core::version().is_empty());
    }
}
