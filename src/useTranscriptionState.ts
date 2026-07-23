import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { isCaptureActive, type CapturePhase } from "./useCaptureState";

export type TranscriptionState =
  | { status: "idle" }
  | { status: "transcribing" }
  | { status: "saved"; path: string }
  | { status: "error"; message: string };

const TRANSCRIPTION_STATE_EVENT = "transcription:state";

/**
 * Subscribes to the backend's post-capture transcription progress:
 * `transcribing` while the pipeline runs, then `saved` or `error`. Unlike
 * `useCaptureState`, there is nothing to seed on mount — transcription only
 * ever starts in response to a capture stop that happens while this is
 * mounted, so there is no missed-transition window to cover.
 *
 * Resets to `idle` whenever a new capture begins (`capturePhase` leaves
 * `idle`) so a terminal `saved`/`error` label from the previous meeting
 * doesn't linger on screen through the next recording.
 *
 * The previous meeting's cleanup stage runs a headless Claude subprocess that
 * can take seconds, so its terminal `saved`/`error` event may not land until
 * the next recording is already under way. A terminal event that arrives
 * while a capture is engaged therefore belongs to that prior run and is
 * dropped — transcription only ever starts after a stop, so a genuine
 * `saved`/`error` for the current session never arrives mid-capture.
 * `transcribing` is exempt: a run's opening event can beat the stop's own
 * `capture:state` broadcast (the worker emits the moment it holds
 * `TRANSCRIBE_LOCK` in `transcribe.rs`, and the phase read here lags a
 * render behind besides), and the one other `transcribing` that can land
 * mid-capture — a lock-queued predecessor's — is still literally true.
 * "Engaged" is every non-idle phase, not just `listening`: a starting or
 * degraded capture is still the current meeting.
 */
export function useTranscriptionState(capturePhase: CapturePhase): TranscriptionState {
  const [state, setState] = useState<TranscriptionState>({ status: "idle" });
  const capturePhaseRef = useRef(capturePhase);
  capturePhaseRef.current = capturePhase;

  useEffect(() => {
    if (isCaptureActive(capturePhase)) setState({ status: "idle" });
  }, [capturePhase]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;

    listen<TranscriptionState>(TRANSCRIPTION_STATE_EVENT, (event) => {
      if (!active) return;
      if (event.payload.status === "transcribing" || !isCaptureActive(capturePhaseRef.current)) {
        setState(event.payload);
      }
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
