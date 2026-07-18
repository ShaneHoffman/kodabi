import { invoke } from "@tauri-apps/api/core";

/*
 * IPC wrappers for the quick-capture window, mirroring the Rust commands in
 * `src-tauri/src/quick_capture.rs`. Window show/hide stays entirely backend-
 * driven (the frontend imports no `@tauri-apps/api/window`); the UI only asks
 * the backend to show/hide and to file the captured text.
 */

/** Where a quick-captured note landed. `project: null` is the Inbox sentinel
 * (matching `NoteSummary`); `confidence` is always present (routing always
 * scores). */
export type QuickCaptureOutcome = {
  id: string;
  path: string;
  project: string | null;
  confidence: number;
};

/** Show + focus the capture window (the command-palette / tray entry point). */
export function showQuickCaptureWindow(): Promise<void> {
  return invoke("show_quick_capture");
}

/** Hide the capture window (Escape, blur handled backend-side, post-flash). */
export function hideQuickCaptureWindow(): Promise<void> {
  return invoke("hide_quick_capture");
}

/** Route and write the captured text; resolves with where it landed. */
export function submitQuickCapture(text: string): Promise<QuickCaptureOutcome> {
  return invoke<QuickCaptureOutcome>("quick_capture_submit", { text });
}
