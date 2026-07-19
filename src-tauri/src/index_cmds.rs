//! Thin Tauri commands over the note index. The work lives on the background
//! worker (`index_state`); these wrappers only enqueue it.

use tauri::{AppHandle, Manager};

use crate::index_state::IndexState;

/// Drops and repopulates the note index from every file on disk. Returns as soon
/// as the job is queued; rebuild progress arrives on the `index:state` event.
#[tauri::command]
pub async fn rebuild_index(app: AppHandle) -> Result<(), String> {
    if app.state::<IndexState>().request_rebuild() {
        Ok(())
    } else {
        Err("the note index is unavailable this session".to_string())
    }
}
