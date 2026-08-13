import { getCurrentWindow, type Window } from "@tauri-apps/api/window";

/**
 * The three window controls the TopBar draws, now that the main window carries
 * no native frame (`decorations: false`).
 *
 * Not the command-parity path: these are Tauri's own `core:window` channels,
 * not `#[tauri::command]`s of ours, so there is no Rust wrapper to keep in
 * lockstep — only the permissions in `src-tauri/capabilities/main-window.json`.
 */

/** The current Tauri window, or null when there is none.
 *
 * `getCurrentWindow()` reads `__TAURI_INTERNALS__.metadata` synchronously, so
 * outside Tauri it *throws* rather than returning a handle that rejects later:
 * `preview.html` mocks `invoke` but no metadata, and a plain browser on the dev
 * server has neither. A design preview should render a bar whose controls do
 * nothing, not a blank screen. */
export function currentWindowOrNull(): Window | null {
  try {
    return getCurrentWindow();
  } catch {
    return null;
  }
}

export function minimizeWindow(): void {
  void currentWindowOrNull()
    ?.minimize()
    .catch(() => {});
}

export function toggleMaximizeWindow(): void {
  void currentWindowOrNull()
    ?.toggleMaximize()
    .catch(() => {});
}

/** Hides to the tray rather than quitting: `lib.rs` intercepts `CloseRequested`
 * for every window and calls `prevent_close()`. That is the same thing the
 * native close button did, and Quit still lives in the tray menu. */
export function closeWindow(): void {
  void currentWindowOrNull()
    ?.close()
    .catch(() => {});
}
