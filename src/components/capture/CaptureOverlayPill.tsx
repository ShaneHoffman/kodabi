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
 *
 * ## SHARED GEOMETRY — change this and ListenPill together
 *
 * This pill and the in-window one are the same control seen in two places, so
 * they carry identical innards: the mark at `14px` core / `11px` halo with its
 * `mr-1`, the label at `11.5px` semibold `tracking-[0.1em]`, the clock in
 * `font-data` at `11.5px` `tabular-nums`, `gap-3` between them, and the
 * `leading-none` + `translate-y-px` pair that centres the glyphs. ListenPill's
 * CENTRING note carries the measurements behind that pair; they apply here
 * unchanged, because the fonts and sizes are now the same.
 *
 * Four things stay deliberately different, and none of them is drift:
 *
 * 1. **The label never takes the green** (see its own note below) — the whole
 *    point of the divergence, and the reason the two are not one component.
 * 2. **`glass-pill` and `px-5`,** because this is a floating window over the
 *    desktop rather than an element inside the app's chrome.
 * 3. **`grow` + `text-center` on the label,** which only make sense once the
 *    box is wider than most states have content for (see the note below).
 * 4. **Height comes from the window, not the content** — 46px here against
 *    ListenPill's 28px. Reasoning at the pill element below.
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
    // ("System audio only" beside the longest clock, see below), so every
    // shorter state has spare width, and growing the label keeps the mark
    // pinned to the left edge and the clock pinned to the right rather than
    // floating the whole group away from both.
    //
    // The window is 252x46 (src-tauri/tauri.conf.json, mirrored in
    // tauri.e2e.conf.json). 252 is the widest state's intrinsic width measured
    // rather than estimated: the pill rendered against the built stylesheet in
    // headless Edge with `width: max-content` — which is the only way to see
    // it, since `grow` below hides every pixel of slack while the pill is
    // stretched to the window — reporting "System audio only" beside a
    // six-character clock. Six characters is the true worst case: formatElapsed
    // (src/useElapsed.ts) is `m:ss` with unbounded minutes and no hours field,
    // so `999:59` covers 16.6 hours, and `tabular-nums` on Cascadia Mono makes
    // every clock of that length exactly as wide. Measured 251.39px, rounded up.
    // The old 248 was measured before the mark grew its `mr-1` and predated
    // this label size, and it was genuinely too narrow: the widest state
    // truncated to an ellipsis in the shipped window.
    //
    // The height is the one dimension NOT derived from the content: 46px around
    // a 14px mark is far more than ListenPill's 28px, and deliberately so. This
    // window floats over other people's applications with no frame, and it is
    // the only thing the user can grab to move it, so it is sized as a target
    // and a presence rather than trimmed to its text.
    <div
      data-tauri-drag-region="deep"
      data-testid="capture-overlay-pill"
      className="glass-pill flex h-screen w-screen cursor-grab items-center gap-3 px-5 select-none active:cursor-grabbing"
    >
      {/* The margin is optical, not decorative. `.spirit-mark`'s layout box is
          the core alone (`--mark-size`), while the listening aura overflows it
          by `--halo-spread` in every direction — so the flex gap left of the
          label is filled by glow while the gap right of it, against the clock,
          stays empty, and the label reads glued to the mark.

          4px, and the value is measured rather than taste: differencing an
          aura-on against an aura-off render puts the glow's optical mass
          ~3.8px past the core's edge (it fades out entirely by ~10px). So 4px
          of clearance lands the *perceived* space left of the label at
          gap-3 + 4px - 3.8px ≈ 12.2px, matching the 12px of gap-3 on the
          clock's side. Static rather than listening-only, because the aura is
          the one state that wants it and a conditional margin would jolt the
          label sideways on every transition into and out of it. */}
      <SpiritMark
        mode={markMode(captureState)}
        size="14px"
        halo="11px"
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
        className="min-w-0 grow translate-y-px truncate text-center font-ui text-[11.5px] leading-none font-semibold tracking-[0.1em] text-ink-dim uppercase"
      >
        {label.text}
      </span>
      <span className="flex-none translate-y-px font-data text-[11.5px] leading-none text-ink tabular-nums">
        {formatElapsed(elapsedSeconds)}
      </span>
    </div>
  );
}
