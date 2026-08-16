import { invoke } from "@tauri-apps/api/core";

/*
 * The typed callers for the two capture commands, mirroring
 * `src-tauri/src/audio_cmds.rs` (`start_capture` / `stop_capture`). Both were
 * registered and reachable from the tray and the global shortcut long before
 * anything in the frontend called them; quick capture's Record affordance is
 * the first UI that does, which is what these close the parity gap for
 * (.claude/rules/tauri-command-parity.md, layer 4).
 *
 * Neither response is worth reading: the authoritative state arrives as a
 * `capture:state` event, which `useCaptureState` already listens for. So these
 * are fire-and-report — the caller shows the error, and the phase change it
 * was asking for shows up on its own.
 *
 * Both commands do resolve with a `CaptureStatus` all the same, so the invoke
 * is typed `unknown` and the value dropped here. Typing it `void` would state
 * a wire shape that is simply false, which is the drift
 * .claude/rules/typescript-style.md's "wire types mirror the Rust DTOs" rule
 * exists to stop; the mirror itself belongs to the first caller that wants the
 * value, not to a placeholder nobody reads.
 */

/**
 * The OS-global chord that toggles capture, mirroring `DEFAULT_TOGGLE_SHORTCUT`
 * in `src-tauri/src/capture_control.rs`. The backend registers it at startup and
 * offers no rebinding command, so this is a claim about a real accelerator
 * rather than a suggestion: every surface that teaches the chord reads it from
 * here, and the spelling is the accelerator's own (unspaced), matching how the
 * palette prints `Ctrl+Alt+Space`.
 *
 * It *toggles* — the same press stops a running capture — which is why the
 * surfaces that print it have to stay honest about what a press would do right
 * now.
 */
export const CAPTURE_TOGGLE_SHORTCUT = "Ctrl+Shift+K";

/**
 * Mirrors `ShortcutStatus` in `src-tauri/src/lib.rs`: whether each OS-global
 * shortcut actually bound at startup. Registration is best-effort so a chord
 * another app owns cannot block launch, which means the constant above is a
 * claim the OS is free to have refused.
 *
 * Immutable for the app's lifetime, so there is no event to pair with it.
 */
export type ShortcutStatus = {
  captureToggle: boolean;
  quickCapture: boolean;
};

/** Read whether the startup shortcut registrations landed. */
export function fetchShortcutStatus(): Promise<ShortcutStatus> {
  return invoke<ShortcutStatus>("shortcut_status");
}

/** Begin a capture. Resolves once the backend has accepted the request; the
 * phase reaching `listening` arrives separately on `capture:state`. */
export function startCapture(): Promise<void> {
  return invoke<unknown>("start_capture").then(() => undefined);
}

/** End the running capture. Transcription and distillation follow on their own
 * events; this only closes the recording. */
export function stopCapture(): Promise<void> {
  return invoke<unknown>("stop_capture").then(() => undefined);
}
