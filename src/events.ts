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

/** Progress of the first-run model download (`download_models`), as a
 * tagged-status payload: `downloading` (per-file and overall bytes),
 * `verifying`, `retrying`, then one of `ready`, `cancelled`, `error`. Mirrors
 * `events::MODELS_STATE_EVENT`. */
export const MODELS_STATE_EVENT = "models:state";

/** The retention sweep deleted expired raw sessions, so any surface listing
 * sessions refetches rather than offering a retry for a file that is gone.
 * Distinct from `vault:changed`: a prune touches no note. Mirrors
 * `events::SESSIONS_CHANGED_EVENT`. */
export const SESSIONS_CHANGED_EVENT = "sessions:changed";

/** A commitment-ledger mutation a person made: a close, waive, snooze, reopen,
 * untrack, an answered evidence claim, a manual track, or a change to a
 * meeting's tracking mode. Distinct from `vault:changed` for the same
 * reason `sessions:changed` is: waiving or snoozing touches no note. A ticked
 * checkbox emits both, because it really does write Markdown. Mirrors
 * `events::LEDGER_CHANGED_EVENT`. */
export const LEDGER_CHANGED_EVENT = "ledger:changed";

/** Post-capture transcription progress, as a tagged-status payload: `queued`
 * (parked behind another run's `TRANSCRIBE_LOCK`), `transcribing` (carrying
 * recording-normalized seconds, re-emitted per audio chunk), then `saved` or
 * `error`. Mirrors `transcribe::TRANSCRIPTION_STATE_EVENT`. */
export const TRANSCRIPTION_STATE_EVENT = "transcription:state";

/** End-of-meeting distill progress, as a tagged-status payload: `distilling`,
 * `routing_fallback`, `saved`, `skipped`, or `error`. Mirrors
 * `distill_cmds::DISTILL_STATE_EVENT`. */
export const DISTILL_STATE_EVENT = "distill:state";

/** Chat-distill progress, same tagged-status shape as `distill:state` but its
 * own channel, so a chat finishing cannot overwrite a meeting distill's label.
 * Mirrors `chat_distill_cmds::CHAT_DISTILL_STATE_EVENT`.
 *
 * No subscriber yet, and deliberately so: a distilled chat surfaces the way a
 * quick capture does, through `vault:changed` refreshing the Inbox and project
 * lists. Named here because this file is the complete event registry. */
export const CHAT_DISTILL_STATE_EVENT = "chat-distill:state";

/** Emitted when a capture is attempted before recording consent is
 * acknowledged, so the frontend opens the consent nudge. Mirrors
 * `capture_control::CONSENT_REQUIRED_EVENT`. */
export const CONSENT_REQUIRED_EVENT = "consent:required";

/** App settings changed over IPC, carrying the new `Settings`, so a view already
 * mounted when the change lands refreshes without a reload. Mirrors
 * `settings_cmds::SETTINGS_CHANGED_EVENT`. */
export const SETTINGS_CHANGED_EVENT = "settings:changed";

/** Sent to the quick-capture window when it comes forward, so its UI can refocus
 * and reset. Mirrors `quick_capture`'s `SHOWN_EVENT`. */
export const QUICK_CAPTURE_SHOWN_EVENT = "quick-capture:shown";

/** A batch of raw terminal (PTY) output, base64-encoded — a per-chunk UTF-8
 * decode would corrupt multibyte sequences split across reads, so the bytes
 * cross the boundary encoded and xterm's own decoder reassembles them. Mirrors
 * `terminal_cmds::TERMINAL_OUTPUT_EVENT`. */
export const TERMINAL_OUTPUT_EVENT = "terminal:output";

/** The hosted `claude` process exited (a deliberate restart or app-exit reap is
 * silent), carrying its exit code if known, so the terminal can offer a restart.
 * Mirrors `terminal_cmds::TERMINAL_EXIT_EVENT`. */
export const TERMINAL_EXIT_EVENT = "terminal:exit";

/** Every chat session lifecycle event — streamed text deltas, completed
 * assistant blocks, tool-use summaries, permission requests/resolutions, turn
 * ends, and process exit — as a tagged payload carrying the session's
 * `chat_id`, so a stale session's stragglers can be dropped. Mirrors
 * `chat_cmds::CHAT_EVENT`. */
export const CHAT_EVENT = "chat:event";
