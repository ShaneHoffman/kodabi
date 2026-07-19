import { invoke } from "@tauri-apps/api/core";

/*
 * IPC wrapper for the capture overlay pill, mirroring the Rust command in
 * `src-tauri/src/overlay.rs`. Showing and hiding the window stays entirely
 * backend-driven — it hangs off the same capture-state funnel as the tray, so
 * the pill can never disagree with what is actually being recorded — and the
 * frontend imports no `@tauri-apps/api/window`. Dragging is the one exception,
 * and needs no IPC: it rides Tauri's injected `data-tauri-drag-region`.
 */

/**
 * Hide the pill for the capture running right now.
 *
 * Session-scoped, not a setting: the backend clears the dismissal when capture
 * returns to idle, so the next capture is announced again. The persistent
 * choice lives in `settings.overlay` (see `setCaptureOverlay` in
 * `useSettings.ts`).
 */
export function dismissCaptureOverlay(): Promise<void> {
  return invoke("dismiss_capture_overlay");
}
