import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { TRANSCRIPTION_STATE_EVENT } from "./events";
import { isCaptureActive, type CapturePhase } from "./useCaptureState";

/**
 * Mirrors `TranscriptionStateEvent` in `src-tauri/src/transcribe.rs` (tagged on
 * `status`, fields in snake_case — `rename_all` there renames the variants, not
 * the fields). `idle` is the frontend's own: the backend never emits it, it is
 * what this hook holds before a run and after a reset.
 *
 * `transcribing` recurs, once per audio chunk, carrying how far into the
 * recording the engines have got. Both figures are seconds of the recording:
 * the pipeline transcribes the two channels one after the other, and the
 * backend divides that work total back down so `total_seconds` is the meeting's
 * own length. `seconds_processed` reaches `total_seconds` while the run is
 * still going, which is the cleanup stage (a headless Claude call) reporting
 * itself as a full bar rather than a stalled one.
 */
export type TranscriptionState =
  | { status: "idle" }
  | { status: "queued" }
  | { status: "transcribing"; seconds_processed: number; total_seconds: number }
  | { status: "saved"; path: string }
  | { status: "error"; message: string };

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
 * `queued` and `transcribing` are exempt: a run's opening event can beat the
 * stop's own `capture:state` broadcast (`queued` is emitted before the worker
 * even blocks on `TRANSCRIBE_LOCK` in `transcribe.rs`, and the phase read here
 * lags a render behind besides), and the ones that can land mid-capture — a
 * lock-queued predecessor's — are still literally true.
 * "Engaged" is every non-idle phase, not just `listening`: a starting or
 * degraded capture is still the current meeting.
 *
 * Each event replaces the state wholesale, which is why the backend re-sends
 * `total_seconds` with every progress tick rather than only with the first.
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
      const live = event.payload.status === "queued" || event.payload.status === "transcribing";
      if (live || !isCaptureActive(capturePhaseRef.current)) {
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
