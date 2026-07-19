import type { CaptureStateEvent } from "./useCaptureState";
import type { SpiritMarkMode } from "./components/SpiritMark";

/** What the indicator shows for a capture state. */
export type CaptureLabel = {
  text: string;
  /** Whether audio is actually being recorded — drives the reserved green. */
  live: boolean;
  /** Secondary line naming what is wrong, when something is. */
  detail: string | null;
};

/**
 * The capture state as words. Pure and separate from the component, so the
 * phrasing that carries the trust invariant ("never say Listening while
 * nothing is being recorded") is readable in one place.
 *
 * `live` tracks whether audio is genuinely being captured, not merely whether
 * a session is engaged: a capture whose devices have both dropped out reads as
 * reconnecting, in ink, because nothing is reaching disk.
 */
export function captureLabel(state: CaptureStateEvent): CaptureLabel {
  const loopbackLive = state.sources.loopback === "live";
  const microphoneLive = state.sources.microphone === "live";

  switch (state.phase) {
    case "idle": {
      // A start that failed outright leaves both sources failed. Saying only
      // "Idle" would read as "you never pressed anything".
      const failed =
        state.sources.loopback === "failed" ||
        state.sources.microphone === "failed";
      return {
        text: "Idle",
        live: false,
        detail: failed ? "Capture failed to start" : null,
      };
    }
    case "starting":
      return { text: "Starting…", live: false, detail: null };
    case "listening":
      return { text: "Listening", live: true, detail: null };
    case "degraded": {
      if (!loopbackLive && !microphoneLive) {
        return { text: "Reconnecting", live: false, detail: null };
      }
      // One source is still recording, so this IS on air: keep the green. But
      // the headline must not read "Listening" either, which would imply both
      // sources are being captured. It names what IS recorded; the detail line
      // names what isn't. This matches the tray tooltip, which never says
      // plain "listening" for a degraded capture.
      return microphoneLive
        ? { text: "Mic only", live: true, detail: "System audio unavailable" }
        : { text: "System audio only", live: true, detail: "Mic unavailable" };
    }
  }
}

/** The mark's visual mode for a capture state. */
export function markMode(state: CaptureStateEvent): SpiritMarkMode {
  if (state.phase !== "degraded") return state.phase;
  const anyLive =
    state.sources.loopback === "live" || state.sources.microphone === "live";
  // Degraded with nothing live is not on air, so it must not wear the green.
  // It must not wear the *idle* mark either: that one means "no capture is
  // running", and this session is still engaged and will resume recording
  // with no further press. `reconnecting` is the ink mark, moving.
  return anyLive ? "degraded" : "reconnecting";
}
