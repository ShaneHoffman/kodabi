import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { CapturePhase } from "./useCaptureState";

export type DistillState =
  | { status: "idle" }
  | { status: "distilling" }
  | { status: "saved"; path: string }
  | { status: "skipped"; reason: string }
  | { status: "error"; message: string };

/**
 * The wire payload, which is `DistillState` plus one non-terminal warning the
 * label state machine deliberately never adopts: `routing_fallback` means the
 * routing signals failed to load and the note filed to Inbox anyway. It arrives
 * mid-distill, so treating it as label state would clobber "Distilling…"; it is
 * logged and dropped instead (like `skipped`, but not even terminal).
 */
type DistillEvent =
  | DistillState
  | { status: "routing_fallback"; message: string };

const DISTILL_STATE_EVENT = "distill:state";

/**
 * Subscribes to the backend's end-of-meeting distill progress: `distilling`
 * while the headless pass runs, then `saved`, `skipped` (nothing distillable —
 * e.g. a silent capture), or `error`. The distill pass fails hard (no note is
 * written on failure), so surfacing its terminal state is the only signal the
 * user gets that a meeting note did or did not land.
 *
 * Mirrors `useTranscriptionState`'s lifecycle exactly, for the same reasons:
 * resets to `idle` when a new capture begins, and drops events that arrive
 * while a capture is live — a distill can run for minutes, so its terminal
 * event may land mid-way through the next recording and would otherwise show
 * a stale label for the wrong meeting.
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
    if (capturePhase === "listening") setState({ status: "idle" });
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
      if (capturePhaseRef.current !== "listening") setState(event.payload);
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
