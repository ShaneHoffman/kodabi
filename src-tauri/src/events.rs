//! Backend event names emitted to the frontend, shared across modules so a
//! single string is the source of truth on the Rust side (the TypeScript mirror
//! lives in `src/events.ts`).

/// Emitted app-wide after any backend-observed vault mutation: a quick-capture
/// write, and the index worker after every reconcile or rebuild the file watcher
/// drives. Any open window listens (via `useVaultChangedBridge`) and refetches
/// its disk-backed lists — the frontend's own `kodabi:vault-changed` DOM bus is
/// per-webview and can't cross windows.
pub const VAULT_CHANGED_EVENT: &str = "vault:changed";

/// Progress of a full index rebuild (`rebuild_index` command), as a
/// tagged-status payload the Settings UI subscribes to. Mirrors the
/// `transcription:state` shape.
pub const INDEX_STATE_EVENT: &str = "index:state";

/// Progress of the first-run model download (`download_models` command), as a
/// tagged-status payload: `downloading` carries per-file and overall byte
/// counts, then `verifying` / `retrying`, then one of `ready`, `cancelled` or
/// `error`. Mirrors the [`INDEX_STATE_EVENT`] shape.
pub const MODELS_STATE_EVENT: &str = "models:state";

/// Emitted after the retention sweep deleted raw sessions, so any surface
/// listing sessions refetches instead of offering a retry for a file that is
/// gone. Distinct from [`VAULT_CHANGED_EVENT`]: a prune touches no note, so
/// nothing about the vault itself changed. Payload: none.
pub const SESSIONS_CHANGED_EVENT: &str = "sessions:changed";

/// Emitted after a commitment-ledger mutation a person made: a close, waive,
/// snooze, reopen, untrack, an answered evidence claim, a manual track, or a
/// change to a meeting's tracking mode. Distinct from
/// [`VAULT_CHANGED_EVENT`] for the same reason [`SESSIONS_CHANGED_EVENT`] is:
/// snoozing touches no note, so claiming the vault changed would be a lie, and
/// a surface listening for vault writes would refetch for nothing. The
/// mutations that *do* write Markdown emit both — a ticked checkbox, and a
/// waive, which leaves a dated line under the item. Payload: none.
pub const LEDGER_CHANGED_EVENT: &str = "ledger:changed";
