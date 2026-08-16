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

/// The index failing to open is a whole-session condition, not a per-query one:
/// nothing the reader does brings it back before a restart, and the notes
/// themselves are files on disk that search never had custody of.
const INDEX_UNAVAILABLE: &str =
    "Search isn't available this session. Your notes are safe; restart Kodabi to bring it back.";

/// Drops and repopulates the note index from every file on disk. Returns as soon
/// as the job is queued; rebuild progress arrives on the `index:state` event.
#[tauri::command]
pub async fn rebuild_index(app: AppHandle) -> Result<(), String> {
    if app.state::<IndexState>().request_rebuild() {
        Ok(())
    } else {
        Err(INDEX_UNAVAILABLE.to_string())
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
        return Err(INDEX_UNAVAILABLE.to_string());
    };
    let options = SearchOptions {
        marks: SnippetMarks {
            open: SNIPPET_MARK_OPEN,
            close: SNIPPET_MARK_CLOSE,
        },
        prefix_last_term: true,
    };
    // The index's own errors name rusqlite internals and cursor encodings; a
    // failed search costs the reader nothing but the query they can retype.
    const SEARCH_FAILED: &str = "Search hit a problem. Your notes are safe; try the search again.";
    tauri::async_runtime::spawn_blocking(move || handle.search(&params, options))
        .await
        .map_err(|err| crate::user_errors::reported("search_notes", err, SEARCH_FAILED))?
        .map_err(|err| crate::user_errors::reported("search_notes", err, SEARCH_FAILED))
}
