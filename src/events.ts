/*
 * Tauri event names, mirrored from `src-tauri/src/events.rs` (and
 * `quick_capture.rs` for the window-scoped ones). Kept in one place on the
 * frontend so a listener and its emitter can't drift apart across files — a
 * rename here (or in the Rust constants it mirrors) is a single edit, not a hunt
 * through every webview. The Rust ↔ TS pair is the only unavoidable duplication
 * (no shared literal spans the FFI); these comments are the contract that keeps
 * them in sync.
 */

/** App-wide broadcast after any backend-observed vault mutation: a quick-capture
 * write, and the index worker after every reconcile or rebuild the file watcher
 * drives. Every open window refetches its disk-backed lists. Mirrors
 * `events::VAULT_CHANGED_EVENT`. */
export const VAULT_CHANGED_EVENT = "vault:changed";

/** Progress of a full index rebuild (`rebuild_index`), as a tagged-status
 * payload: `{ status: "rebuilding" }`, `{ status: "ready", notes }`, or
 * `{ status: "error", message }`. Mirrors `events::INDEX_STATE_EVENT`. */
export const INDEX_STATE_EVENT = "index:state";

/** Sent to the quick-capture window when it comes forward, so its UI can refocus
 * and reset. Mirrors `quick_capture`'s `SHOWN_EVENT`. */
export const QUICK_CAPTURE_SHOWN_EVENT = "quick-capture:shown";
