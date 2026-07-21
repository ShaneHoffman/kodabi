import { invoke } from "@tauri-apps/api/core";

/*
 * The typed callers for the two capture commands, mirroring
 * `src-tauri/src/audio_cmds.rs` (`start_capture` / `stop_capture`). Both were
 * registered and reachable from the tray and the global shortcut long before
 * anything in the frontend called them; quick capture's Record affordance is
 * the first UI that does, which is what these close the parity gap for
 * (.claude/rules/tauri-command-parity.md, layer 4).
 *
 * Neither returns anything useful: the authoritative state arrives as a
 * `capture:state` event, which `useCaptureState` already listens for. So these
 * are fire-and-report — the caller shows the error, and the phase change it
 * was asking for shows up on its own.
 */

/** Begin a capture. Resolves once the backend has accepted the request; the
 * phase reaching `listening` arrives separately on `capture:state`. */
export function startCapture(): Promise<void> {
  return invoke<void>("start_capture");
}

/** End the running capture. Transcription and distillation follow on their own
 * events; this only closes the recording. */
export function stopCapture(): Promise<void> {
  return invoke<void>("stop_capture");
}
