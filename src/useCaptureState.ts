import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type CapturePhase = "idle" | "listening";

type CaptureStateEvent = { phase: CapturePhase };

const CAPTURE_STATE_EVENT = "capture:state";

/**
 * Subscribes to the backend's capture idle/listening state: an invoke on
 * mount seeds the current state (so a transition that happened before the
 * listener attached isn't missed), then `capture:state` events keep it live.
 */
export function useCaptureState(): CapturePhase {
  const [phase, setPhase] = useState<CapturePhase>("idle");

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;

    invoke<CaptureStateEvent>("capture_phase")
      .then((event) => {
        if (active) setPhase(event.phase);
      })
      .catch(() => {});

    listen<CaptureStateEvent>(CAPTURE_STATE_EVENT, (event) => {
      if (active) setPhase(event.payload.phase);
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

  return phase;
}
