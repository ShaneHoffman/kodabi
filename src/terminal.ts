import { invoke } from "@tauri-apps/api/core";

/**
 * Typed callers for the embedded terminal's Tauri commands. Wire types mirror
 * the serde DTOs in `src-tauri/src/terminal_cmds.rs`; the invoke strings equal
 * the Rust command names exactly (`.claude/rules/tauri-command-parity.md`).
 */

/** Mirrors `TerminalSnapshot` in `src-tauri/src/terminal_cmds.rs`. */
export type TerminalSnapshot = {
  running: boolean;
  /** Base64 of the raw PTY scrollback, replayed into a freshly mounted xterm. */
  scrollback: string;
  cols: number;
  rows: number;
};

/** Mirrors `OutputPayload` in `src-tauri/src/terminal_cmds.rs`. Base64 raw bytes. */
export type TerminalOutputEvent = {
  data: string;
};

/** Mirrors `ExitPayload` in `src-tauri/src/terminal_cmds.rs`. `code` is null when
 * the exit status could not be read. */
export type TerminalExitEvent = {
  code: number | null;
};

/** Ensures a live terminal session and returns a snapshot to hydrate xterm.
 * Idempotent: reuses the running session so a view switch does not restart. */
export function openTerminal(): Promise<TerminalSnapshot> {
  return invoke<TerminalSnapshot>("terminal_open");
}

/** Sends keyboard input (xterm's `onData` string) to the PTY. */
export function writeTerminal(data: string): Promise<void> {
  return invoke<void>("terminal_write", { data });
}

/** Resizes the PTY grid to match the xterm viewport. */
export function resizeTerminal(cols: number, rows: number): Promise<void> {
  return invoke<void>("terminal_resize", { cols, rows });
}

/** Reaps the current session and spawns a fresh one (the "Restart" action). */
export function restartTerminal(): Promise<TerminalSnapshot> {
  return invoke<TerminalSnapshot>("terminal_restart");
}
