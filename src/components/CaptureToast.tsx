import { useState } from "react";
import { useCaptureState } from "../useCaptureState";
import { useDistillState } from "../useDistillState";
import { useTimeout } from "../useTimeout";
import { useTranscriptionState } from "../useTranscriptionState";
import "./CaptureToast.css";

/** How long a success stays up. Long enough to read a three-word sentence,
 * short enough that it is gone before you look for the thing it announced. */
const DWELL_MS = 3500;

type Notice = {
  /** Identity, not content: when this changes the dwell timer restarts, so a
   * second outcome never inherits the remains of the first one's clock. */
  id: string;
  text: string;
  /** A failure stays until dismissed and announces assertively; everything
   * else fades on its own and announces politely. */
  failed: boolean;
};

/**
 * What the pipeline is doing, as one line, derived from the two hooks that
 * watch it. Distill is checked first because it runs after transcription: once
 * a distill has anything to say, the transcription result is already old news.
 *
 * Only real progress and real failures earn a toast. A `skipped` distill
 * (nothing distillable — a silent capture) is a benign non-event, and popping
 * a panel to report that nothing happened is worse than staying quiet.
 */
function noticeFor(
  transcription: ReturnType<typeof useTranscriptionState>,
  distill: ReturnType<typeof useDistillState>,
): Notice | null {
  switch (distill.status) {
    case "distilling":
      return { id: "distill:running", text: "Distilling the meeting…", failed: false };
    case "saved":
      return { id: "distill:saved", text: "Note saved.", failed: false };
    case "error":
      return {
        id: "distill:error",
        // Where to go next, not just what broke: the session is already listed
        // under Needs attention, and a failure the user cannot act on is a
        // failure they will just have to worry about.
        text: "Couldn't distill that capture. It's under Needs attention.",
        failed: true,
      };
    case "skipped":
    case "idle":
      break;
  }

  switch (transcription.status) {
    case "transcribing":
      return { id: "transcribe:running", text: "Transcribing…", failed: false };
    case "error":
      return {
        id: "transcribe:error",
        text: "Couldn't transcribe that capture. It's under Needs attention.",
        failed: true,
      };
    // A saved transcript is a step, not a result — the note is the result, and
    // distill is about to say so. Announcing both makes one capture pop two
    // toasts for the same piece of work.
    case "saved":
    case "idle":
      break;
  }

  return null;
}

/**
 * The distill pipeline's outcomes, in front of the app for a moment.
 *
 * These used to render as extra `CaptureStatusLine`s stacked under the sidebar
 * foot's IDLE dot, which broke the one promise that surface makes: it is a
 * fixed silhouette that never reflows, so the Settings and Commands rows below
 * it never move under the pointer. A bare `SAVED` sitting under `IDLE` also
 * said nothing about *what* was saved, and stayed there long after it was
 * true.
 *
 * A failure does not auto-dismiss and carries `role="alert"`: it arrives
 * asynchronously and the user may not be looking (docs/DESIGN_SYSTEM.md §6).
 * It is safe to dismiss because it is not the only record — the session stays
 * listed under Needs attention until it is retried, which is what the copy
 * points at.
 */
export function CaptureToast() {
  const captureState = useCaptureState();
  const transcription = useTranscriptionState(captureState.phase);
  const distill = useDistillState(captureState.phase);
  const notice = noticeFor(transcription, distill);

  // Which notice the user has already seen out. Compared during render rather
  // than cleared in an effect: a new notice id is a new thing to say, so the
  // dismissal of the previous one must not silence it.
  const [dismissedId, setDismissedId] = useState<string | null>(null);
  const showing = notice && notice.id !== dismissedId ? notice : null;

  useTimeout(
    () => setDismissedId(showing?.id ?? null),
    showing && !showing.failed ? DWELL_MS : null,
  );

  if (!showing) return null;

  return (
    <div
      className="toast"
      role={showing.failed ? "alert" : "status"}
      data-testid="capture-toast"
    >
      <p className="text-label text-text">{showing.text}</p>
      {showing.failed && (
        <button
          type="button"
          aria-label="Dismiss"
          onClick={() => setDismissedId(showing.id)}
          className="toast__dismiss ui-focus-ring text-label text-text-faint"
        >
          ×
        </button>
      )}
    </div>
  );
}
