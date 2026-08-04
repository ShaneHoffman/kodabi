import { clsx } from "clsx";
import { captureLabel, markMode } from "../../captureLabel";
import type { CaptureStateEvent } from "../../useCaptureState";
import { useCaptureState } from "../../useCaptureState";
import { useDebouncedValue } from "../../useDebouncedValue";
import { CaptureStatusLine } from "./CaptureStatusLine";
import { SpiritMark } from "./SpiritMark";

/**
 * What is actually reaching disk, as a phrase. The design's live indicator
 * carries a second line under "Listening", and the reference fills it with a
 * meeting name and a timer — neither of which the capture backend reports, so
 * this says the true thing it does know: which sources are recording. A
 * degraded capture is exactly when that line earns its place.
 */
function sourceLine(state: CaptureStateEvent): string | null {
  const loopback = state.sources.loopback === "live";
  const microphone = state.sources.microphone === "live";
  if (loopback && microphone) return "Mic + system audio";
  if (microphone) return "Mic";
  if (loopback) return "System audio";
  return null;
}

/**
 * The persistent on-air surface in the sidebar foot: one spirit mark, always in
 * the same place, always the same 11px core, whatever capture is doing.
 *
 * The state reads through the mark's FILL and the label's VALUE, never through
 * a change of the core's size — the foot must not reflow when a capture starts,
 * or the Settings and Commands rows jump under the pointer. Idle is an ink mark
 * beside a mono `IDLE`; live is the one reserved green beside a full-ink label,
 * with `markMode` naming the ink in-between states (starting, degraded,
 * reconnecting) so a session that is engaged is never mistaken for one that is
 * not running at all.
 *
 * The core is size-invariant; the live mark's aura is not — it reaches past its
 * own box, which is what the live-only `pt-2xs` below is reserving room for.
 * Removing it reintroduces exactly the reflow this paragraph forbids.
 *
 * It reports CAPTURE only. What transcription and distillation are doing
 * afterwards used to stack up here as extra lines — a bare `SAVED` sitting
 * under `IDLE`, saying nothing about what was saved and long outliving the
 * moment it was true. That progress now lives where its result will land —
 * the Inbox placeholder row (`InboxView.tsx`) — and its failures in
 * `CaptureToast`, which the sidebar foot never has to make room for.
 *
 * The green means precisely one thing: audio is genuinely being recorded. A
 * degraded capture that still has a live source keeps it (it *is* recording,
 * and dropping it would falsely imply privacy); a capture whose sources have
 * all dropped shows no green at all, and the label carries the reconnecting
 * state instead.
 */
export function ListeningIndicator() {
  const captureState = useCaptureState();
  // The dot reacts instantly for immediate visual feedback, but the text
  // label — an aria-live region — follows a debounced state so a flapping VAD
  // doesn't spam screen readers (or flicker the label) on every toggle.
  const label = captureLabel(useDebouncedValue(captureState, 400));
  const live = captureLabel(captureState).live;
  const sources = sourceLine(captureState);

  return (
    // The sidebar insets are still the legacy geometry family: this row sits on
    // the same left edge as the nav rows above it, and it keeps doing so until
    // the sidebar's own Grove ticket. Live gains room above for the mark's aura,
    // which reaches past its own box.
    <div
      className={clsx(
        "flex flex-col gap-2xs px-[var(--sidebar-row-x)] pb-[var(--sidebar-section-gap)]",
        live && "pt-2xs",
      )}
    >
      <div className={clsx("flex items-center", live ? "gap-sm" : "gap-xs")}>
        {/* Optically centred against the quiet label, which is uppercase and so
            has no descenders: matching the boxes leaves the mark sitting
            visibly low against the caps. Live, the label is the mixed-case
            headline step and needs no correction. */}
        <SpiritMark
          mode={markMode(captureState)}
          size="11px"
          halo="11px"
          className={live ? undefined : "-translate-y-px"}
        />
        {live ? (
          <div className="flex min-w-0 flex-1 flex-col gap-3xs">
            <CaptureStatusLine live variant="headline">
              {label.text}
            </CaptureStatusLine>
            {(label.detail ?? sources) && (
              <p className="text-cap text-text-faint">{label.detail ?? sources}</p>
            )}
          </div>
        ) : (
          <CaptureStatusLine>{label.text}</CaptureStatusLine>
        )}
      </div>
      {/* Not live, so the detail has nowhere else to go — a failed start still
          has to say so. */}
      {!live && label.detail && <CaptureStatusLine>{label.detail}</CaptureStatusLine>}
    </div>
  );
}
