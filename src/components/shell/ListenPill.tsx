import { cva } from "class-variance-authority";
import { clsx } from "clsx";
import { SpiritMark, type SpiritMarkMode } from "../capture/SpiritMark";
import { formatElapsed } from "../../useElapsed";

/**
 * The pill's two tones, and the whole of the reason it is a component.
 *
 * `live` is the only green surface in the app's chrome: green means audio is
 * reaching disk, and the pill wears it on exactly the modes the mark does
 * (docs/DESIGN_SYSTEM.md §2). `quiet` is the same box in neutral glass.
 *
 * The transition is the point. Because both tones are one element and only the
 * values differ, going on air is a 300ms morph of fill, edge and label rather
 * than one pill being replaced by another — and the mark's core crossfades to
 * green on the same clock (src/index.css §3), so the pill and the creature
 * inside it arrive together.
 */
const listenPillVariants = cva(
  [
    "inline-flex select-none items-center gap-3 rounded-pill border py-1.5 pr-3.5 pl-3",
    "shadow-[inset_0_1px_0_var(--color-edge-lit)]",
    "transition-[background-color,border-color,color] duration-300 ease-out-strong",
  ],
  {
    variants: {
      tone: {
        live: "border-kodama/30 bg-kodama/10",
        quiet: "border-edge bg-wash",
      },
    },
    defaultVariants: { tone: "quiet" },
  },
);

type Props = {
  mode: SpiritMarkMode;
  /**
   * The state in words. Supplied by the caller rather than derived here,
   * because what a degraded capture should say ("Mic only" vs "System audio
   * only") is not derivable from the mark's mode — src/captureLabel.ts owns
   * that phrasing, and it is the trust-bearing half of this control.
   */
  label: string;
  /**
   * The one line beside the headline: what is wrong when something is
   * (`captureLabel().detail`), or whatever else the caller ranks in behind it.
   * Shown only while NOT live, because that is the case the headline cannot
   * carry on its own: a start whose every source failed reads as a bare "Not
   * listening", which is indistinguishable from never having pressed anything.
   * Live, the headline already names what is recording ("Mic only") and the
   * clock has the slot.
   *
   * Note what "live" is and isn't: it means audio is reaching disk, NOT that a
   * capture is engaged. `starting` and `reconnecting` are engaged and still
   * render this line, so a caller whose copy is only true of an idle pill has
   * to gate on the phase itself — TopBar's chord hint does exactly that.
   */
  detail?: string | null;
  /** Whole seconds recorded, from useElapsed. Shown only while live. */
  elapsedSeconds?: number | null;
  className?: string;
};

/**
 * The shell's listening indicator: the kodama, the state in words, and how
 * long it has been recording.
 *
 * Presentational on purpose — it takes a mode, a label and a number, and holds
 * no capture state of its own. The wiring (useCaptureState → markMode →
 * useElapsed) belongs to the top bar that mounts it, which keeps this
 * renderable in the primitive gallery with no IPC and no running clock.
 *
 * Degraded counts as live. Audio IS being captured on one source, and a pill
 * that dropped its green would tell the user the room had gone private when it
 * had not; the mark's collapsed aura is what says "not full listening".
 * Starting and reconnecting are the opposite case — engaged, recording
 * nothing — so they stay neutral, and the clock stays hidden with them: a
 * timer running over a capture that is writing nothing is a confident lie.
 *
 * ## SHARED GEOMETRY — change this and CaptureOverlayPill together
 *
 * The two capture pills are meant to read as one control, so the mark
 * (`14px` core, `11px` halo, `mr-1`), the label (`11.5px` semibold,
 * `tracking-[0.1em]`, `leading-none`), the clock (`font-data` `11.5px`,
 * `tabular-nums`) and the `gap-3` between them are the same in both. What
 * stays deliberately different is listed in CaptureOverlayPill: that window
 * has its own glass, its own padding, a label that never takes the green, and
 * a height set by the window rather than by this pill's `py-1.5`.
 *
 * ## CENTRING — why the text carries a 1px translate
 *
 * Flex `items-center` centres layout BOXES, and it does that perfectly here:
 * every box in this row measures 0.00px off the pill's centreline. What the
 * eye reads is not the box, though, but the glyphs inside it, and a font's
 * ascent and descent are not symmetric about its cap band — so centred boxes
 * still render text that sits high.
 *
 * Measured against the built stylesheet in headless Edge at DPR 1 (mass
 * centroid of the rendered ink, cross-checked against the cap band derived
 * from the DOM baseline and canvas TextMetrics): the label sat **1.51px**
 * above the centreline and the clock **0.93px** above it. Note that
 * `leading-none` does NOT fix this — half-leading is symmetric, so
 * line-height very nearly cancels out of the ink-vs-box offset; it is here to
 * make the pill a whole 28px (14px mark + `py-1.5` + border) instead of
 * 31.25px, and to stop the row's height depending on which glyphs it holds.
 *
 * `translate-y-px` corrects both, and 1px is not a rounding of taste: Chromium
 * snaps a text translation to whole device pixels, so 0.5px, 0.75px, 1px and
 * 1.25px all rasterise identically and only integers are reachable. 1px lands
 * the label 0.51px low-side of centre and the clock 0.07px past it; 2px
 * overshoots both. This is a static optical correction, not a press or a
 * motion state, so `--press-scale` and the reduced-motion swaps
 * (DESIGN_SYSTEM §2) have no bearing on it — nothing about it ever moves.
 */
export function ListenPill({
  mode,
  label,
  detail,
  elapsedSeconds,
  className,
}: Props) {
  const live = mode === "listening" || mode === "degraded";

  return (
    <div
      className={clsx(
        listenPillVariants({ tone: live ? "live" : "quiet" }),
        className,
      )}
    >
      {/* Clearance for the aura, the same call CaptureOverlayPill makes, at the
          same measured 4px and for the same reason: the mark's layout box is
          its core, the listening glow reaches past it, and the gap against the
          clock has no such filler, so without this the label reads glued to
          the mark. Static, not live-only — a conditional margin would shift
          the label sideways at exactly the moment the clock appears. */}
      <SpiritMark mode={mode} size="14px" halo="11px" className="mr-1" />
      {/* The live region is the label and its detail. The clock stays OUT of
          it: a time announced every second is the whole content of the region
          changing once a second, forever. The detail is the opposite case —
          it changes only when the capture's health does, and it is the half a
          screen-reader user cannot infer from a mark. */}
      <span
        role="status"
        className={clsx(
          "font-ui text-[11.5px] leading-none font-semibold tracking-[0.1em] uppercase",
          // See CENTRING above. `leading-none` makes the pill 28px instead of
          // 31.25px; `translate-y-px` is the optical correction, and it is
          // layout-inert, so it moves the glyphs without re-biasing the
          // `items-center` that positions the box.
          "translate-y-px",
          "transition-colors duration-300 ease-out-strong",
          // Not `text-kodama`: as a sentence the mark's green measures under
          // 4.5:1 on both grounds. `kodama-ink` is the step that exists for
          // green carrying words (DESIGN_SYSTEM §6).
          live ? "text-kodama-ink" : "text-ink-dim",
        )}
      >
        {label}
        {/* Nested rather than a sibling, so it lands in the same region and is
            announced with the state it explains. The resets are what keep it
            reading as a note beside the label instead of a second headline. */}
        {!live && detail && (
          <>
            {/* A real space, not just the margin: the two spans concatenate
                into one accessible name, and without it a screen reader says
                "Not listeningCapture failed to start". */}{" "}
            <span className="ml-1 font-normal tracking-normal normal-case text-ink-dim">
              {detail}
            </span>
          </>
        )}
      </span>
      {live && elapsedSeconds != null && (
        // Same treatment as the label, and the same 1px: Cascadia Mono's digits
        // sit 0.93px high in their box where Bahnschrift's caps sit 1.51px
        // high, and both round to the same whole pixel (see CENTRING).
        <span className="translate-y-px font-data text-[11.5px] leading-none text-ink tabular-nums">
          {formatElapsed(elapsedSeconds)}
        </span>
      )}
    </div>
  );
}
