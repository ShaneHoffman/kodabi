mod audio_cmds;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(audio_cmds::CaptureState::default())
        .invoke_handler(tauri::generate_handler![
            audio_cmds::start_capture,
            audio_cmds::stop_capture,
            audio_cmds::capture_status,
        ])
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
