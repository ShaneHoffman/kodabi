import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { MODELS_STATE_EVENT } from "./events";
import {
  aggregateModelStatus,
  cancelModelDownload,
  fetchModelStatus,
  progressFromEvent,
  startModelDownload,
  type ModelDownloadState,
  type ModelsStateEvent,
} from "./models";

/**
 * The backend's model-provisioning state: an invoke on mount seeds what is
 * already on disk, then `models:state` events keep it live.
 *
 * Seeded rather than purely push-driven, unlike `useConsentNudge`: "the models
 * are missing" is a fact that exists before anything happens, and a download
 * may already be running when a view mounts. This is the `useCaptureState`
 * shape.
 */
export function useModelDownload(): {
  state: ModelDownloadState;
  start: () => Promise<void>;
  cancel: () => Promise<void>;
} {
  const [state, setState] = useState<ModelDownloadState>({ status: "unknown" });

  useEffect(() => {
    let active = true;

    const seed = () =>
      fetchModelStatus()
        .then((status) => {
          if (active) setState(aggregateModelStatus(status));
          // To download without asking, start it here when the seeded status is
          // `missing`. Deliberately not done: 760 MB is the user's call.
        })
        // A status read that fails says nothing about whether the models are
        // there, so the state stays `unknown` rather than becoming an error.
        // Claiming transcription is broken because we could not ask would put a
        // warning in front of the user on the strength of no evidence; `error`
        // is reserved for a download that actually failed.
        .catch(() => {});

    void seed();

    let unlisten: (() => void) | undefined;
    listen<ModelsStateEvent>(MODELS_STATE_EVENT, (event) => {
      if (!active) return;
      setState((current) => next(current, event.payload));
      // A terminal event changes what is on disk, so re-read rather than infer:
      // only the backend knows whether a cancelled run left a resumable part.
      if (event.payload.status === "ready" || event.payload.status === "cancelled") {
        void seed();
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

  const start = useCallback(async () => {
    // Optimistic: the command returns as soon as the worker is queued, and the
    // first progress event can trail it by a whole connection setup.
    setState({ status: "downloading", progress: null });
    try {
      await startModelDownload();
    } catch (error) {
      setState({ status: "error", message: String(error) });
    }
  }, []);

  const cancel = useCallback(async () => {
    try {
      await cancelModelDownload();
    } catch (error) {
      setState({ status: "error", message: String(error) });
    }
  }, []);

  return { state, start, cancel };
}

/** Folds one backend event into the current state. */
function next(current: ModelDownloadState, event: ModelsStateEvent): ModelDownloadState {
  switch (event.status) {
    case "downloading":
      return { status: "downloading", progress: progressFromEvent(event) };
    case "verifying": {
      // Keep the byte figures the last report carried; only the phase changed.
      const progress = current.status === "downloading" ? current.progress : null;
      return {
        status: "downloading",
        progress: progress ? { ...progress, verifying: true } : null,
      };
    }
    case "retrying":
      // A retry is still a download in progress; the message is logged backend
      // side and would only alarm here, since resuming usually just works.
      return current.status === "downloading" ? current : { status: "downloading", progress: null };
    case "ready":
      return { status: "ready", envOverridden: false };
    case "cancelled":
      // The seed that follows replaces this with the true remaining figure.
      return { status: "missing", bytesRequired: 0 };
    case "error":
      return { status: "error", message: event.message };
  }
}
