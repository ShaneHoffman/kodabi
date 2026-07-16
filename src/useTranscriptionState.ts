import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { CapturePhase } from "./useCaptureState";

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
 * Resets to `idle` whenever a new capture begins (`capturePhase` becomes
 * `listening`) so a terminal `saved`/`error` label from the previous meeting
 * doesn't linger on screen through the next recording.
 */
export function useTranscriptionState(capturePhase: CapturePhase): TranscriptionState {
  const [state, setState] = useState<TranscriptionState>({ status: "idle" });

  useEffect(() => {
    if (capturePhase === "listening") setState({ status: "idle" });
  }, [capturePhase]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;

    listen<TranscriptionState>(TRANSCRIPTION_STATE_EVENT, (event) => {
      if (active) setState(event.payload);
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
