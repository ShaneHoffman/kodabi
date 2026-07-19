import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** Mirrors `CapturePhase` in `src-tauri/src/audio_cmds.rs`. */
export type CapturePhase = "idle" | "starting" | "listening" | "degraded";

/** Mirrors `SourceState` in `src-tauri/src/audio_cmds.rs`. */
export type CaptureSourceState = "off" | "live" | "stalled" | "failed";

/** Mirrors `CaptureSources` in `src-tauri/src/audio_cmds.rs`. */
export type CaptureSources = {
  loopback: CaptureSourceState;
  microphone: CaptureSourceState;
};

/** Mirrors `CaptureStateEvent` in `src-tauri/src/audio_cmds.rs`. */
export type CaptureStateEvent = {
  phase: CapturePhase;
  sources: CaptureSources;
};

const CAPTURE_STATE_EVENT = "capture:state";

const IDLE_STATE: CaptureStateEvent = {
  phase: "idle",
  sources: { loopback: "off", microphone: "off" },
};

/**
 * Whether capture is engaged at all — a start is in flight, or a session is
 * running (fully or degraded). The complement of "nothing is happening", which
 * is what the transcription/distill hooks key their staleness rules off.
 */
export function isCaptureActive(phase: CapturePhase): boolean {
  return phase !== "idle";
}

/**
 * Subscribes to the backend's capture state: an invoke on mount seeds the
 * current state (so a transition that happened before the listener attached
 * isn't missed), then `capture:state` events keep it live.
 *
 * The backend pushes on every change, including ones no toggle drove — a
 * device lost mid-session degrades the state within a few seconds — so what
 * this returns can always be shown as-is without polling to double-check.
 */
export function useCaptureState(): CaptureStateEvent {
  const [captureState, setCaptureState] = useState<CaptureStateEvent>(IDLE_STATE);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;

    invoke<CaptureStateEvent>("capture_phase")
      .then((event) => {
        if (active) setCaptureState(event);
      })
      .catch(() => {});

    listen<CaptureStateEvent>(CAPTURE_STATE_EVENT, (event) => {
      if (active) setCaptureState(event.payload);
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

  return captureState;
}
