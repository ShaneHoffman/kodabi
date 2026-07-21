import { captureLabel } from "../captureLabel";
import type { CaptureStateEvent } from "../useCaptureState";
import { useCaptureState } from "../useCaptureState";
import { useDebouncedValue } from "../useDebouncedValue";
import { useDistillState } from "../useDistillState";
import { useTranscriptionState } from "../useTranscriptionState";
import { CaptureStatusLine } from "./CaptureStatusLine";
import "./ListeningIndicator.css";

function transcriptionLabel(
  state: ReturnType<typeof useTranscriptionState>,
): string | null {
  switch (state.status) {
    case "transcribing":
      return "Transcribing…";
    case "saved":
      return "Saved";
    case "error":
      return "Transcription failed";
    case "idle":
      return null;
  }
}

function distillLabel(state: ReturnType<typeof useDistillState>): string | null {
  switch (state.status) {
    case "distilling":
      return "Distilling…";
    case "saved":
      return "Note saved";
    case "error":
      return "Distill failed";
    // A skipped distill (nothing distillable — e.g. a silent capture) is not
    // worth a status line; only real progress and real failures surface.
    case "skipped":
    case "idle":
      return null;
  }
}

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
 * The persistent on-air surface in the sidebar foot: one dot, always in the
 * same place, always the same size, whatever capture is doing.
 *
 * The state reads through the dot's FILL and the label's VALUE, never through
 * a change of shape — the foot must not reflow when a capture starts, or the
 * Settings and Commands rows jump under the pointer. Idle is an ink dot beside
 * a mono `IDLE`; live is the one reserved green plus a breathing glow beside a
 * full-ink label. Those are the only two silhouettes.
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
  const transcription = useTranscriptionState(captureState.phase);
  const transcriptionText = transcriptionLabel(transcription);
  const distill = useDistillState(captureState.phase);
  const distillText = distillLabel(distill);
  const live = captureLabel(captureState).live;
  const sources = sourceLine(captureState);

  return (
    <div className={`listening${live ? " listening--live" : ""} flex flex-col gap-2xs`}>
      <div className={`flex items-center ${live ? "gap-sm" : "gap-xs"}`}>
        <span className="listening__dot">
          {live && <span className="listening__glow" aria-hidden="true" />}
          <span
            className={`listening__core${live ? " listening__core--live" : ""}`}
          />
        </span>
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
      {transcriptionText && <CaptureStatusLine>{transcriptionText}</CaptureStatusLine>}
      {distillText && <CaptureStatusLine>{distillText}</CaptureStatusLine>}
    </div>
  );
}
