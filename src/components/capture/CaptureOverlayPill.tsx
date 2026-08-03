import { clsx } from "clsx";
import { captureLabel, markMode } from "../../captureLabel";
import { isCaptureActive, useCaptureState } from "../../useCaptureState";
import { useDebouncedValue } from "../../useDebouncedValue";
import { formatElapsed, useEngagedElapsed } from "../../useElapsed";
import { SpiritMark } from "./SpiritMark";

/**
 * The always-on-top capture pill: the kodama, the state in words, and how long
 * the recording has been running, in its own frameless window that floats over
 * full-screen apps.
 *
 * It exists for the one case the in-window indicator and the tray both miss:
 * presenting or screen-sharing hides the taskbar, the main window is usually
 * closed to tray, and WASAPI loopback never lights the Windows microphone
 * indicator — so without this a running capture can be completely invisible.
 *
 * **Pure status: it carries no controls.** Stopping is the global shortcut and
 * the main window, and the pill has no dismiss of its own — a recording that
 * can hide itself is a recording that can be forgotten, which is the invariant
 * this window exists to hold. Visibility is the backend's call
 * (`src-tauri/src/overlay.rs` shows and hides the window off the capture-state
 * funnel); rendering nothing while idle is a second, independent guard on the
 * same invariant, not the primary mechanism.
 */
export function CaptureOverlayPill() {
  const captureState = useCaptureState();
  // Same split as the sidebar indicator: the mark reacts instantly, the text
  // follows a debounced state so a flapping source doesn't strobe the label.
  const label = captureLabel(useDebouncedValue(captureState, 400));
  const engaged = isCaptureActive(captureState.phase);
  // Timed from the press, so a mid-session dropout never rewinds the clock.
  // The mark beside it is what reports whether audio is reaching disk.
  const elapsedSeconds = useEngagedElapsed(engaged);

  // Belt and braces. The window should already be hidden by the time capture
  // reads idle; if it somehow isn't, an empty pill is far better than one
  // claiming to be recording.
  if (!engaged) return null;

  return (
    // The window is deliberately larger than the pill: the pill's drop shadow
    // needs transparent room to fade into, or it is clipped flat at the window
    // bounds and reads as a rectangle around a rounded pill. This padded frame
    // is that room.
    //
    // `deep` (not the bare attribute) so a press anywhere — pill or the
    // transparent frame, which swallows clicks regardless — drags the window.
    <div
      data-tauri-drag-region="deep"
      data-testid="capture-overlay-root"
      className="flex h-screen w-screen cursor-grab items-center justify-center p-3 select-none active:cursor-grabbing"
    >
      <div
        data-testid="capture-overlay-pill"
        className="glass-pill flex max-w-full min-w-0 items-center gap-2.5 px-5 py-3"
      >
        <SpiritMark mode={markMode(captureState)} size="13px" halo="10px" />
        {/* The live region is the label alone. Wrapping the clock in it too
            would announce a new time every second, forever. */}
        <span
          role="status"
          className={clsx(
            "min-w-0 grow truncate font-ui text-[11px] font-semibold tracking-[0.12em] uppercase",
            "transition-colors duration-300 ease-out-strong",
            label.live ? "text-kodama-ink" : "text-ink-dim",
          )}
        >
          {label.text}
        </span>
        <span className="flex-none font-data text-[13px] text-ink tabular-nums">
          {formatElapsed(elapsedSeconds)}
        </span>
      </div>
    </div>
  );
}
