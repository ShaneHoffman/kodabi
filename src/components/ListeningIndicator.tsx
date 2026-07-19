import { captureLabel, markMode } from "../captureLabel";
import { useCaptureState } from "../useCaptureState";
import { useDebouncedValue } from "../useDebouncedValue";
import { useDistillState } from "../useDistillState";
import { useTranscriptionState } from "../useTranscriptionState";
import { SpiritMark } from "./SpiritMark";

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
 * The persistent on-air surface: a small SpiritMark plus status text,
 * always visible in the sidebar foot regardless of the active view.
 */
export function ListeningIndicator() {
  const captureState = useCaptureState();
  // The mark reacts instantly for immediate visual feedback, but the text
  // label — an aria-live region — follows a debounced state so a flapping VAD
  // doesn't spam screen readers (or flicker the label) on every toggle.
  const label = captureLabel(useDebouncedValue(captureState, 400));
  const transcription = useTranscriptionState(captureState.phase);
  const transcriptionText = transcriptionLabel(transcription);
  const distill = useDistillState(captureState.phase);
  const distillText = distillLabel(distill);

  return (
    <div className="flex flex-col gap-2xs">
      <div className="flex items-center gap-xs">
        <SpiritMark mode={markMode(captureState)} size="1rem" halo="0.9rem" />
        <p
          role="status"
          className={`text-cap uppercase tracking-wide ${
            label.live ? "text-accent-dot" : "text-text-faint"
          }`}
        >
          {label.text}
        </p>
      </div>
      {label.detail && (
        <p role="status" className="text-cap uppercase tracking-wide text-text-faint">
          {label.detail}
        </p>
      )}
      {transcriptionText && (
        <p role="status" className="text-cap uppercase tracking-wide text-text-faint">
          {transcriptionText}
        </p>
      )}
      {distillText && (
        <p role="status" className="text-cap uppercase tracking-wide text-text-faint">
          {distillText}
        </p>
      )}
    </div>
  );
}
