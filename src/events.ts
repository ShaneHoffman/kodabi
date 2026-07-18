/*
 * Tauri event names, mirrored from `src-tauri/src/quick_capture.rs`. Kept in
 * one place on the frontend so a listener and its emitter can't drift apart
 * across files — a rename here (or in the Rust constants it mirrors) is a single
 * edit, not a hunt through every webview. The Rust ↔ TS pair is the only
 * unavoidable duplication (no shared literal spans the FFI); these comments are
 * the contract that keeps them in sync.
 */

/** App-wide broadcast after a cross-window vault write (e.g. quick capture), so
 * every open window can refetch. Mirrors `quick_capture::VAULT_CHANGED_EVENT`. */
export const VAULT_CHANGED_EVENT = "vault:changed";

/** Sent to the quick-capture window when it comes forward, so its UI can refocus
 * and reset. Mirrors `quick_capture`'s `SHOWN_EVENT`. */
export const QUICK_CAPTURE_SHOWN_EVENT = "quick-capture:shown";
