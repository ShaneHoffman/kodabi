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
    // The label's `grow` is what absorbs the slack, not a `justify-center` on
    // this row: the window is sized to the widest state this can report
    // ("System audio only" beside an hours-long clock, see below), so every
    // shorter state has spare width, and growing the label keeps the mark
    // pinned to the left edge and the clock pinned to the right rather than
    // floating the whole group away from both.
    //
    // TODO(width): measured for the pre-mr-1 layout at 248px; re-measure with
    // the mark's mr-1 folded in before shipping.
    <div
      data-tauri-drag-region="deep"
      data-testid="capture-overlay-pill"
      className="glass-pill flex h-screen w-screen cursor-grab items-center gap-2.5 px-5 select-none active:cursor-grabbing"
    >
      {/* The margin is optical, not decorative. `.spirit-mark`'s layout box is
          the core alone (`--mark-size`), while the listening aura overflows it
          by `--halo-spread` in every direction — so the flex gap left of the
          label is filled by glow while the gap right of it, against the clock,
          stays empty, and the label reads glued to the mark.

          4px, and the value is measured rather than taste: differencing an
          aura-on against an aura-off render puts the glow's optical mass
          ~3.8px past the core's edge (it fades out entirely by ~10px). So 4px
          of clearance lands the *perceived* space either side of the label at
          10px each, matching the gap-2.5 on the clock's side. Static rather
          than listening-only, because the aura is the one state that wants it
          and a conditional margin would jolt the label sideways on every
          transition into and out of it. */}
      <SpiritMark
        mode={markMode(captureState)}
        size="13px"
        halo="10px"
        className="mr-1"
      />
      {/* The live region is the label alone. Wrapping the clock in it too
          would announce a new time every second, forever.

          Dim ink in every state, unlike the in-app ListenPill, whose label
          does step up to `kodama-ink` while live. That divergence is the
          point: this window floats over other people's applications, and on
          the desktop the mark is the only green thing while audio is being
          captured (DESIGN_SYSTEM §2). A pill that is half green reads as an
          alert; the signal here is calm and always on.

          `grow` and `text-center`: inert while the pill hugs its content, and
          load-bearing now that the window is wider than most states — see the
          `grow` note above. */}
      <span
        role="status"
        className="min-w-0 grow truncate text-center font-ui text-[11px] font-semibold tracking-[0.12em] text-ink-dim uppercase"
      >
        {label.text}
      </span>
      <span className="flex-none font-data text-[13px] text-ink tabular-nums">
        {formatElapsed(elapsedSeconds)}
      </span>
    </div>
  );
}
