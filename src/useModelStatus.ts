import { createContext, useContext } from "react";
import type { ModelDownloadState } from "./models";

/**
 * The model-provisioning state, subscribed once at the shell
 * (`ModelStatusProvider`) and read from here by everything that cares: the
 * first-run nudge, the Settings card, the capture indicators.
 *
 * Shell-held for the same reason the capture pipeline is: `useModelDownload`
 * has one listener per caller, so four independent subscriptions would each
 * miss whatever arrived before they mounted, and a download that finished while
 * the user was on another view would leave stale copies behind.
 *
 * The quick-capture window is its own webview with no providers at all, so it
 * calls `useModelDownload` directly — which is why the hook and this context
 * are separate files.
 */
export type ModelStatus = {
  state: ModelDownloadState;
  start: () => Promise<void>;
  cancel: () => Promise<void>;
};

export const ModelStatusContext = createContext<ModelStatus | undefined>(undefined);

export function useModelStatus(): ModelStatus {
  const ctx = useContext(ModelStatusContext);
  if (!ctx) {
    throw new Error("useModelStatus must be used within <ModelStatusProvider>");
  }
  return ctx;
}

/**
 * Whether transcription can run right now. Used for the honest capture-side
 * copy: a recording made while this is false is kept and transcribed later, so
 * the UI says so rather than letting it surface as a failure after the meeting.
 *
 * `unknown` counts as ready: it means the seed has not landed yet, and warning
 * about a state we have not confirmed would flash a scare on every launch.
 */
export function isTranscriptionReady(state: ModelDownloadState): boolean {
  return state.status === "ready" || state.status === "unknown";
}
