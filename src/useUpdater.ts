import { useCallback, useEffect, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { opaqueFailure } from "./errorCopy";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type DownloadEvent, type Update } from "@tauri-apps/plugin-updater";

/** Bytes moved so far and, once the server has said, how many there are in all.
 *
 * `total` is nullable because it is: the updater reports a content length only
 * if the response carried one, and a progress bar that invents a denominator
 * is worse than a byte count that admits it does not know. */
export type DownloadProgress = {
  receivedBytes: number;
  totalBytes: number | null;
};

/**
 * Where the update flow has got to. One phase at a time, and every transition
 * out of `available` is something the user clicked: nothing downloads or
 * installs on its own (the ticket's "no silent forced updates", and the same
 * rule the 760 MB model download follows).
 *
 * `error` carries the step it failed at, because the three failures are not
 * the same news: a failed check is usually the network, a failed download is
 * worth retrying, and a failed install has already torn down the terminal.
 */
export type UpdaterPhase =
  | { status: "idle" }
  | { status: "checking" }
  | { status: "upToDate" }
  | { status: "available"; version: string; notes: string | null }
  | { status: "downloading"; version: string; progress: DownloadProgress }
  | { status: "readyToInstall"; version: string }
  | { status: "installing"; version: string }
  | { status: "error"; step: "check" | "download" | "install"; message: string };

export type UpdaterState = {
  /** This build's version, for the Settings row. Null until the read lands. */
  appVersion: string | null;
  phase: UpdaterPhase;
};

/**
 * The release check and the download/install lifecycle of
 * `@tauri-apps/plugin-updater`, plus this build's own version.
 *
 * Two external systems would normally mean two bridge hooks, but they are one
 * here: `getVersion()` is a one-shot read on the same mount, it is what the
 * "am I updated" row in Settings answers with, and splitting it would give the
 * Settings card two sources for one question.
 *
 * The `Update` object is held in a ref rather than in state. It is a live
 * resource handle with methods on it, not data — putting it in state would
 * invite a render to read a handle that a later check has already replaced.
 *
 * `checkOnMount` is a parameter rather than an `import.meta.env.DEV` read in
 * here, so tests drive it directly; `UpdaterProvider` is what decides that a
 * dev session never nags.
 */
export function useUpdater(options: { checkOnMount: boolean }): {
  state: UpdaterState;
  check: () => Promise<void>;
  download: () => Promise<void>;
  install: () => Promise<void>;
} {
  const [state, setState] = useState<UpdaterState>({
    appVersion: null,
    phase: { status: "idle" },
  });
  const updateRef = useRef<Update | null>(null);
  // Read during render so the mount effect below can stay `[]`: flipping this
  // option after mount is not a thing any caller does, and a dependency on it
  // would re-run the startup check on every provider re-render.
  const checkOnMountRef = useRef(options.checkOnMount);

  useEffect(() => {
    let active = true;

    void getVersion()
      .then((version) => {
        if (active) setState((current) => ({ ...current, appVersion: version }));
      })
      // A version we could not read leaves the Settings row saying so. It is
      // not an update failure and must not render as one.
      .catch(() => {});

    if (checkOnMountRef.current) {
      setState((current) => ({ ...current, phase: { status: "checking" } }));
      void check()
        .then((update) => {
          if (!active) return;
          updateRef.current = update;
          setState((current) => ({ ...current, phase: phaseForCheck(update) }));
        })
        // Swallowed, unlike the manual check below. This one runs unbidden at
        // every launch, and a machine that is offline or behind a proxy would
        // otherwise be told at startup, every startup, that something failed
        // that it never asked for. The Settings card is where a user who wants
        // an answer goes, and that path does report the error.
        .catch(() => {
          if (active) setState((current) => ({ ...current, phase: { status: "idle" } }));
        });
    }

    return () => {
      active = false;
    };
  }, []);

  /** The manual check: this one surfaces its failure, because someone asked. */
  const runCheck = useCallback(async () => {
    setState((current) => ({ ...current, phase: { status: "checking" } }));
    try {
      const update = await check();
      updateRef.current = update;
      setState((current) => ({ ...current, phase: phaseForCheck(update) }));
    } catch (error) {
      setState((current) => ({
        ...current,
        phase: {
          status: "error",
          step: "check",
          // `opaqueFailure`, not `backendCopy`: these come from
          // `tauri-plugin-updater`, whose errors serialize to strings that are
          // mostly transparent wrappers over reqwest/io ("dns error", "os error
          // 5"). A string from here is as raw as an exception.
          message: opaqueFailure(
            error,
            "Couldn't check for updates. Kodabi will try again next launch.",
          ),
        },
      }));
    }
  }, []);

  const download = useCallback(async () => {
    const update = updateRef.current;
    if (!update) return;
    const version = update.version;
    setState((current) => ({
      ...current,
      phase: {
        status: "downloading",
        version,
        progress: { receivedBytes: 0, totalBytes: null },
      },
    }));
    try {
      await update.download((event) => {
        setState((current) => ({ ...current, phase: foldDownloadEvent(current.phase, event) }));
      });
      // Not inferred from the `Finished` event: `download()` resolving is the
      // only thing that means the bytes are on disk and verified.
      setState((current) => ({ ...current, phase: { status: "readyToInstall", version } }));
    } catch (error) {
      setState((current) => ({
        ...current,
        phase: {
          status: "error",
          step: "download",
          message: opaqueFailure(
            error,
            "Couldn't download the update. Your current version is untouched; try again.",
          ),
        },
      }));
    }
  }, []);

  const install = useCallback(async () => {
    const update = updateRef.current;
    if (!update) return;
    setState((current) => ({
      ...current,
      phase: { status: "installing", version: update.version },
    }));
    try {
      // Before `install()`, never after: the plugin exits this process from
      // inside that call, so this is the last moment the chat and terminal
      // child trees can be reaped. An orphaned kodabi-mcp.exe locks the very
      // binary NSIS is about to replace. See `src-tauri/src/updater_cmds.rs`.
      await invoke("updater_prepare_install");
      await update.install();
      // Unreachable on Windows: `install()` exits the process, and the NSIS
      // installer restarts the app itself (`restart_after_install` defaults
      // on). Kept because it is the documented tail of the flow and the one
      // line that would matter if Kodabi ever ran anywhere else.
      await relaunch();
    } catch (error) {
      setState((current) => ({
        ...current,
        phase: {
          status: "error",
          step: "install",
          message: opaqueFailure(
            error,
            "Couldn't install the update. Your current version is untouched; try again.",
          ),
        },
      }));
    }
  }, []);

  return { state, check: runCheck, download, install };
}

/** `check()` resolves to null when there is nothing newer. */
function phaseForCheck(update: Update | null): UpdaterPhase {
  if (!update) return { status: "upToDate" };
  return {
    status: "available",
    version: update.version,
    // The release notes are whatever the release body says, which for a
    // `--generate-notes` release is a changelog nobody wrote for this dialog.
    // Carried, not shown, until something has a use for it.
    notes: update.body ?? null,
  };
}

/**
 * Folds one download event into the current phase.
 *
 * Exported for its own test: this is the only part of the flow with arithmetic
 * in it, and it is the part a fake `Update` in a hook test exercises least
 * directly.
 *
 * `Finished` deliberately does not advance the phase — the awaited
 * `download()` does that, so the two cannot disagree about whether the file
 * is actually on disk.
 */
export function foldDownloadEvent(phase: UpdaterPhase, event: DownloadEvent): UpdaterPhase {
  if (phase.status !== "downloading") return phase;
  switch (event.event) {
    case "Started":
      return {
        ...phase,
        progress: { receivedBytes: 0, totalBytes: event.data.contentLength ?? null },
      };
    case "Progress":
      return {
        ...phase,
        progress: {
          ...phase.progress,
          receivedBytes: phase.progress.receivedBytes + event.data.chunkLength,
        },
      };
    case "Finished":
      return phase;
  }
}
