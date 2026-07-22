import { useMemo, type ReactNode } from "react";
import { CapturePipelineContext } from "../useCapturePipeline";
import { useCaptureState } from "../useCaptureState";
import { useDistillState } from "../useDistillState";
import { useTranscriptionState } from "../useTranscriptionState";

/**
 * The one subscription to the capture/transcription/distill events, held
 * above every consumer so a view that mounts mid-pipeline (navigating to the
 * Inbox after a capture has already stopped) still sees the current stage —
 * the underlying hooks are per-consumer and miss anything emitted before they
 * mount.
 */
export function CapturePipelineProvider({ children }: { children: ReactNode }) {
  const capture = useCaptureState();
  const transcription = useTranscriptionState(capture.phase);
  const distill = useDistillState(capture.phase);
  const value = useMemo(
    () => ({ capture, transcription, distill }),
    [capture, transcription, distill],
  );

  return <CapturePipelineContext value={value}>{children}</CapturePipelineContext>;
}
