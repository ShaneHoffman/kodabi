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
  // Same split as the transport bar's ListenPill: the mark reacts instantly,
  // the text follows a debounced state so a flapping source doesn't strobe the
  // label.
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
    // The pill *is* the window: it fills it edge to edge, and the window is
    // sized to the pill. There is no frame around it, because a transparent
    // webview window is not click-through — every pixel of it takes the mouse,
    // so a margin the user cannot see would still show the grab cursor and eat
    // clicks meant for the application underneath. Flush bounds make the thing
    // you can see and the thing you can grab the same shape. That is also why
    // `glass-pill` carries no drop shadow: with no room to fade into, a shadow
    // clips flat and reads as a dark wall over whatever is behind it.
    //
    // `deep` (not the bare attribute) so a press on the mark, the label or the
    // clock drags the window too, rather than only one landing on this element.
    //
    // The contents are centred rather than spread, because the window is a
    // fixed size and the label is not: it is sized in tauri.conf.json to hold
    // the widest state this can report ("System audio only" beside an
    // hours-long clock), so every shorter state would otherwise leave a gap
    // between the label and the clock wide enough to read as two things rather
    // than one line. Centred, the slack becomes padding.
    //
    // 248px is that widest state measured, not estimated: 13 mark + 125 label
    // + 46 clock + 20 of gaps + 40 of padding + 2 of border, with a couple of
    // px spare. Sizing it any wider buys nothing and costs the common
    // "Listening" state — 66px of label — a visibly slack pill.
    <div
      data-tauri-drag-region="deep"
      data-testid="capture-overlay-pill"
      className="glass-pill flex h-screen w-screen cursor-grab items-center justify-center gap-2.5 px-5 select-none active:cursor-grabbing"
    >
      <SpiritMark mode={markMode(captureState)} size="13px" halo="10px" />
      {/* The live region is the label alone. Wrapping the clock in it too
          would announce a new time every second, forever.

          Faint ink in every state, unlike the in-app ListenPill, whose label
          does step up to `kodama-ink` while live. That divergence is the
          point: this window floats over other people's applications, and on
          the desktop the mark is the only green thing while audio is being
          captured (DESIGN_SYSTEM §2). A pill that is half green reads as an
          alert; the signal here is calm and always on. */}
      <span
        role="status"
        className="min-w-0 truncate font-ui text-[11px] font-semibold tracking-[0.12em] text-ink-dim uppercase"
      >
        {label.text}
      </span>
      <span className="flex-none font-data text-[13px] text-ink tabular-nums">
        {formatElapsed(elapsedSeconds)}
      </span>
    </div>
  );
}
