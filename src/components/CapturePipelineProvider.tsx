import { useMemo, useState, type ReactNode } from "react";
import { CapturePipelineContext } from "../useCapturePipeline";
import { isCaptureActive, useCaptureState } from "../useCaptureState";
import { useDistillState } from "../useDistillState";
import { useTranscriptionState } from "../useTranscriptionState";

/**
 * The one subscription to the capture/transcription/distill events, held
 * above every consumer so a view that mounts mid-pipeline (navigating to the
 * Inbox after a capture has already stopped) still sees the current stage —
 * the underlying hooks are per-consumer and miss anything emitted before they
 * mount.
 *
 * It also holds the record of which `filed` outcome the Inbox has fully
 * presented (`handledFiledId`): the distill hook keeps reporting `saved`
 * until the next capture starts, and this record is what stops a remounting
 * Inbox from replaying the vanish and the filed toast for it.
 */
export function CapturePipelineProvider({ children }: { children: ReactNode }) {
  const capture = useCaptureState();
  const transcription = useTranscriptionState(capture.phase);
  const distill = useDistillState(capture.phase);

  const [handledFiledId, setHandledFiledId] = useState<string | null>(null);
  // A new capture starting is the run boundary: the record belongs to the
  // outcome that produced it, and the hooks below are about to reset to idle
  // anyway. Adjusted during render (.claude/rules/no-use-effect.md), same as
  // the Inbox's own waiver reset.
  if (isCaptureActive(capture.phase) && handledFiledId !== null) {
    setHandledFiledId(null);
  }

  const value = useMemo(
    () => ({
      capture,
      transcription,
      distill,
      handledFiledId,
      markFiledHandled: setHandledFiledId,
    }),
    [capture, transcription, distill, handledFiledId],
  );

  return <CapturePipelineContext value={value}>{children}</CapturePipelineContext>;
}
