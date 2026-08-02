import { captureLabel, markMode } from "../../captureLabel";
import { dismissCaptureOverlay } from "../../captureOverlay";
import { isCaptureActive, useCaptureState } from "../../useCaptureState";
import { useDebouncedValue } from "../../useDebouncedValue";
import { CaptureStatusLine } from "./CaptureStatusLine";
import { SpiritMark } from "./SpiritMark";
// eslint-disable-next-line no-restricted-syntax -- pre-Grove; the capture windows' Grove ticket deletes it
import "./CaptureOverlayPill.css";

/**
 * The always-on-top capture pill: the same mark-plus-status pairing as the
 * transport bar's ListenPill, but in its own frameless window that floats over
 * full-screen apps.
 *
 * It exists for the one case the in-window indicator and the tray both miss:
 * presenting or screen-sharing hides the taskbar, the main window is usually
 * closed to tray, and WASAPI loopback never lights the Windows microphone
 * indicator — so without this a running capture can be completely invisible.
 *
 * Visibility is the backend's call (`src-tauri/src/overlay.rs` shows and hides
 * the window off the capture-state funnel). Rendering nothing while idle is a
 * second, independent guard on the same invariant, not the primary mechanism.
 */
export function CaptureOverlayPill() {
  const captureState = useCaptureState();
  // Same split as the sidebar indicator: the mark reacts instantly, the text
  // follows a debounced state so a flapping source doesn't strobe the label.
  const label = captureLabel(useDebouncedValue(captureState, 400));

  // Belt and braces. The window should already be hidden by the time capture
  // reads idle; if it somehow isn't, an empty pill is far better than one
  // claiming to be recording.
  if (!isCaptureActive(captureState.phase)) return null;

  return (
    // The window is deliberately larger than the pill: the pill's drop shadow
    // needs transparent room to fade into, or it is clipped flat at the window
    // bounds and reads as a rectangle around a rounded pill. This padded frame
    // is that room.
    //
    // `deep` (not the bare attribute) so a press anywhere — pill or the
    // transparent frame, which swallows clicks regardless — drags the window.
    // Tauri's drag script excludes <button> subtrees on its own, which is what
    // keeps the dismiss control clickable.
    <div
      data-tauri-drag-region="deep"
      data-testid="capture-overlay-root"
      className="capture-overlay-pill__frame flex h-screen items-center p-sm"
    >
      <div
        data-testid="capture-overlay-pill"
        className="capture-overlay-pill flex h-full w-full items-center gap-2xs rounded-full bg-surface px-xs"
      >
        <SpiritMark mode={markMode(captureState)} size="0.85rem" halo="0.8rem" />
        <div className="capture-overlay-pill__label grow">
          <CaptureStatusLine live={label.live}>{label.text}</CaptureStatusLine>
        </div>
        <button
          type="button"
          aria-label="Hide capture pill"
          data-testid="capture-overlay-dismiss"
          className="capture-overlay-pill__dismiss ui-focus-ring text-text-soft"
          onClick={() => void dismissCaptureOverlay()}
        >
          {/* Decorative: the accessible name is on the button. */}
          <span aria-hidden="true">✕</span>
        </button>
      </div>
    </div>
  );
}
