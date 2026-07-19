import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { DISTILL_STATE_EVENT } from "./events";
import { isCaptureActive, type CapturePhase } from "./useCaptureState";

/**
 * How a distill run ended. Every variant carries `session_path` — the session
 * the outcome belongs to — so a list of retryable sessions can pin each outcome
 * to the right row; without it, a `saved` for one session is indistinguishable
 * from the retry a row is waiting on. This hook ignores the field.
 */
export type DistillOutcome =
  | { status: "saved"; path: string; session_path: string }
  | { status: "skipped"; reason: string; session_path: string }
  | { status: "error"; message: string; session_path: string };

/** The label state machine: the wire statuses plus a local `idle` that no
 * event ever carries. */
export type DistillState =
  | { status: "idle" }
  | { status: "distilling" }
  | DistillOutcome;

/**
 * The wire payload (mirrors `distill_cmds::DistillStateEvent`): the run's
 * progress plus one non-terminal warning the label state machine deliberately
 * never adopts. `routing_fallback` means the routing signals failed to load and
 * the note filed to Inbox anyway; it arrives mid-distill, so treating it as
 * label state would clobber "Distilling…", and it is logged and dropped instead
 * (like `skipped`, but not even terminal).
 *
 * Exported because more than one surface subscribes to `distill:state`: the
 * shape is the contract with Rust, and restating it per subscriber is how the
 * two drift apart.
 */
export type DistillEvent =
  | { status: "distilling" }
  | DistillOutcome
  | { status: "routing_fallback"; message: string };

/**
 * Subscribes to the backend's end-of-meeting distill progress: `distilling`
 * while the headless pass runs, then `saved`, `skipped` (nothing distillable —
 * e.g. a silent capture), or `error`. The distill pass fails hard (no note is
 * written on failure), so surfacing its terminal state is the only signal the
 * user gets that a meeting note did or did not land.
 *
 * Mirrors `useTranscriptionState`'s lifecycle exactly, for the same reasons:
 * resets to `idle` when a new capture begins, and drops events that arrive
 * while a capture is engaged — a distill can run for minutes, so its terminal
 * event may land mid-way through the next recording and would otherwise show
 * a stale label for the wrong meeting. "Engaged" spans every non-idle phase,
 * not just `listening`: a capture that is starting or degraded is still the
 * current meeting, and a stale event landing in one of those windows belongs
 * to the previous one just the same.
 *
 * A `routing_fallback` warning is non-fatal and never becomes label state: it
 * is logged and bypasses the capture-phase guard entirely (a log line, unlike
 * a label, cannot go stale).
 */
export function useDistillState(capturePhase: CapturePhase): DistillState {
  const [state, setState] = useState<DistillState>({ status: "idle" });
  const capturePhaseRef = useRef(capturePhase);
  capturePhaseRef.current = capturePhase;

  useEffect(() => {
    if (isCaptureActive(capturePhase)) setState({ status: "idle" });
  }, [capturePhase]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;

    listen<DistillEvent>(DISTILL_STATE_EVENT, (event) => {
      // Respect teardown for every branch: an event delivered in the async gap
      // after cleanup must not log or set state.
      if (!active) return;
      if (event.payload.status === "routing_fallback") {
        console.warn(
          `distill routing fell back to Inbox: ${event.payload.message}`,
        );
        return;
      }
      if (!isCaptureActive(capturePhaseRef.current)) setState(event.payload);
    }).then((fn) => {
      if (active) {
        unlisten = fn;
      } else {
        fn();
      }
    });

    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  return state;
}
