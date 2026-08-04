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
   * What is wrong, when something is (`captureLabel().detail`). Shown only
   * while NOT live, because that is the case the headline cannot carry on its
   * own: a start whose every source failed reads as a bare "Idle", which is
   * indistinguishable from never having pressed anything. Live, the headline
   * already names what is recording ("Mic only") and the clock has the slot.
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
      <SpiritMark mode={mode} size="14px" halo="11px" />
      {/* The live region is the label and its detail. The clock stays OUT of
          it: a time announced every second is the whole content of the region
          changing once a second, forever. The detail is the opposite case —
          it changes only when the capture's health does, and it is the half a
          screen-reader user cannot infer from a mark. */}
      <span
        role="status"
        className={clsx(
          "font-ui text-[11.5px] font-semibold tracking-[0.1em] uppercase",
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
                "IdleCapture failed to start". */}{" "}
            <span className="ml-1 font-normal tracking-normal normal-case text-ink-faint">
              {detail}
            </span>
          </>
        )}
      </span>
      {live && elapsedSeconds != null && (
        <span className="font-data text-[11.5px] text-ink tabular-nums">
          {formatElapsed(elapsedSeconds)}
        </span>
      )}
    </div>
  );
}
