import { invoke } from "@tauri-apps/api/core";

/*
 * IPC wrappers for the quick-capture window, mirroring the Rust commands in
 * `src-tauri/src/quick_capture.rs`. Window show/hide stays entirely backend-
 * driven (the frontend imports no `@tauri-apps/api/window`); the UI only asks
 * the backend to show/hide and to file the captured text.
 */

/**
 * The OS-global chord that pops the capture box, mirroring
 * `DEFAULT_QUICK_CAPTURE_SHORTCUT` in `src-tauri/src/quick_capture.rs`. It fires
 * even while Kodabi is unfocused, which is the whole point of the feature.
 *
 * The backend registers it at startup and offers no rebinding command, so this
 * is a claim about a real accelerator rather than a suggestion: every surface
 * that teaches the chord reads it from here, in the accelerator's own unspaced
 * spelling. `CAPTURE_TOGGLE_SHORTCUT` in `captureControl.ts` is the same
 * arrangement for the other global chord.
 */
export const QUICK_CAPTURE_SHORTCUT = "Ctrl+Alt+Space";

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

/** The router's guess for a draft, mirroring `QuickCaptureRoutePreview` in
 * `src-tauri/src/quick_capture.rs`. `project: null` is the Inbox sentinel. */
export type QuickCaptureRoutePreview = {
  project: string | null;
  confidence: number;
};

/** Where the draft *would* file right now. Read-only: writes nothing, so it is
 * safe to call as the user types. */
export function previewQuickCaptureRoute(
  text: string,
): Promise<QuickCaptureRoutePreview> {
  return invoke<QuickCaptureRoutePreview>("quick_capture_route_preview", {
    text,
  });
}
