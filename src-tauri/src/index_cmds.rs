//! Thin Tauri commands over the note index — writes enqueue work on the
//! background worker (`index_state`), reads borrow its index handle. Neither
//! owns any logic: `kodabi-core` does.

use kodabi_core::index::{SearchOptions, SearchParams, SearchResults, SnippetMarks};
use tauri::{AppHandle, Manager};

use crate::index_state::IndexState;

/// What `search_notes` wraps a matched term in, mirrored by `MARK_OPEN`/
/// `MARK_CLOSE` in `src/useSearch.ts`, which splits the snippet on them to
/// render `<mark>`.
///
/// Private-use codepoints rather than the contract's Markdown `**`: the indexed
/// text is the note's raw Markdown body, so a note that writes `**firm**` would
/// otherwise be indistinguishable from a match. These cannot occur in text a
/// user typed. (The MCP tool keeps `**`, which is what a model wants to read.)
const SNIPPET_MARK_OPEN: &str = "\u{E000}";
const SNIPPET_MARK_CLOSE: &str = "\u{E001}";

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

/// Searches the note index — the same hybrid FTS5 + vector search the MCP
/// `search_notes` tool runs, with the presentation the search field needs:
/// parseable match delimiters, and a final term treated as a prefix because the
/// user is still typing it.
///
/// Runs on a blocking thread: the index worker holds the lock across a
/// whole-vault reconcile or rebuild, so this can wait seconds and must not
/// block the IPC thread while it does.
#[tauri::command]
pub async fn search_notes(app: AppHandle, params: SearchParams) -> Result<SearchResults, String> {
    let Some(handle) = app.state::<IndexState>().search_handle() else {
        return Err("the note index is unavailable this session".to_string());
    };
    let options = SearchOptions {
        marks: SnippetMarks {
            open: SNIPPET_MARK_OPEN,
            close: SNIPPET_MARK_CLOSE,
        },
        prefix_last_term: true,
    };
    tauri::async_runtime::spawn_blocking(move || handle.search(&params, options))
        .await
        .map_err(|err| err.to_string())?
        .map_err(|err| err.to_string())
}
