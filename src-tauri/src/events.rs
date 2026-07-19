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
