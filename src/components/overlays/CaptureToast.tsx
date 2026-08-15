import { useState } from "react";
import {
  CLAUDE_INSTALL_URL,
  isClaudeMissingMessage,
} from "../../claudeMissing";
import type { ModelDownloadState } from "../../models";
import { useCapturePipeline } from "../../useCapturePipeline";
import { isTranscriptionReady, useModelStatus } from "../../useModelStatus";

type Notice = {
  /** Identity, not content: it keys the dismissal record, so a second
   * failure of the same kind never inherits the fact that the first one was
   * already dismissed. It names a KIND of outcome rather than a run, which is
   * why `dismissedId` is cleared whenever the pipeline goes quiet. */
  id: string;
  text: string;
};

/**
 * A failure worth interrupting for, derived from the two hooks that watch
 * the pipeline. Distill is checked first because it runs after transcription
 * — a distill failure supersedes a transcription result that already
 * succeeded on the way to it.
 *
 * Progress and success are not notices here: they render live, in the
 * Inbox, as the placeholder row that becomes the routed note
 * (`InboxView.tsx`). This toast exists only for the failures the Inbox
 * placeholder can't announce on its own, because a failed capture never
 * lands a row there — it lands under Needs attention instead.
 */
function noticeFor(
  pipeline: ReturnType<typeof useCapturePipeline>,
  models: ModelDownloadState,
): Notice | null {
  if (pipeline.distill.status === "error") {
    // A missing `claude` is the one distill failure with a fix, and on a fresh
    // install it is the likeliest one: name the prerequisite instead of sending
    // the user to a Needs attention row whose Retry can only fail again.
    if (isClaudeMissingMessage(pipeline.distill.message)) {
      return {
        id: "distill:claude-missing",
        text: `Couldn't distill that capture: Kodabi's AI features run through Claude Code, and Claude Code isn't installed. Install the claude CLI from ${CLAUDE_INSTALL_URL}, then retry it from Needs attention.`,
      };
    }
    return {
      id: "distill:error",
      // Where to go next, not just what broke: the session is already listed
      // under Needs attention, and a failure the user cannot act on is a
      // failure they will just have to worry about.
      text: "Couldn't distill that capture. It's under Needs attention.",
    };
  }
  if (pipeline.transcription.status === "error") {
    // The cause is knowable here and the remedy is different, so say so. A
    // transcription that failed for want of models is not a lost recording:
    // the audio is kept and retried on a later launch, and the only thing the
    // user has to do is finish the download.
    if (!isTranscriptionReady(models)) {
      return {
        id: "transcribe:models",
        text: "Couldn't transcribe yet: the models aren't downloaded. The recording is safe and will be transcribed later.",
      };
    }
    return {
      id: "transcribe:error",
      text: "Couldn't transcribe that capture. It's under Needs attention.",
    };
  }
  return null;
}

/**
 * The distill pipeline's failures, in front of the app for a moment.
 *
 * Everything else the pipeline does used to announce here too — "Transcribing…",
 * "Distilling the meeting…", "Note saved." — but those are progress, and
 * progress now lives where the result of it will land: the Inbox placeholder
 * row. A failure is different: it can happen on any screen, the note it
 * would have produced never appears anywhere for the placeholder to become,
 * and the user may not be looking, so it stays a toast.
 *
 * It does not auto-dismiss and carries `role="alert"`: it arrives
 * asynchronously and the user may not be looking (docs/DESIGN_SYSTEM.md §6).
 * It is safe to dismiss because it is not the only record — the session stays
 * listed under Needs attention until it is retried, which is what the copy
 * points at.
 */
export function CaptureToast() {
  const pipeline = useCapturePipeline();
  const { state: models } = useModelStatus();
  const notice = noticeFor(pipeline, models);

  // Which notice the user has already seen out. Compared during render rather
  // than cleared in an effect: a new notice id is a new thing to say, so the
  // dismissal of the previous one must not silence it.
  const [dismissedId, setDismissedId] = useState<string | null>(null);

  // ...and the ids name a KIND of outcome, not a run, so the record of what has
  // been seen has to end with the run it belongs to. The pipeline going quiet
  // is that boundary: without this, dismissing one capture's failure would
  // swallow the next capture's identical failure entirely. Adjusted during
  // render rather than in an effect (.claude/rules/no-use-effect.md).
  if (notice === null && dismissedId !== null) setDismissedId(null);

  const showing = notice && notice.id !== dismissedId ? notice : null;

  if (!showing) return null;

  return (
    // Unpositioned: the bottom-right corner is owned by the notice stack in
    // `AppShell`, which is what keeps this and the update notice from landing
    // on top of each other when both have something to say.
    //
    // No entrance animation, and nothing for reduced motion to disable:
    // showing the surface IS the transition (FOUNDING_DOC §4). A toast that
    // slides in arrives after the thing it is reporting.
    <div
      className="glass-overlay flex max-w-[236px] items-center gap-3 px-4 py-3"
      role="alert"
      data-testid="capture-toast"
    >
      <p className="font-ui text-[13.5px] leading-snug text-ink">{showing.text}</p>
      <button
        type="button"
        aria-label="Dismiss"
        onClick={() => setDismissedId(showing.id)}
        // The hit target grows by one step in three directions without moving
        // the label, so the pad and its bleed are the same step read twice.
        className="focus-ring -my-1 -mr-1 flex-none rounded-[6px] p-1 font-ui text-[13.5px] text-ink-dim transition-colors duration-140 ease-out-strong hover:bg-wash hover:text-ink"
      >
        ×
      </button>
    </div>
  );
}
