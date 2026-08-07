import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { foldDownloadEvent, useUpdater, type UpdaterPhase } from "./useUpdater";
import { invokedCommands, onCommand, resetTauriMocks } from "./test/tauri";
import {
  check,
  failCheck,
  relaunch,
  resetUpdaterMocks,
  setAppVersion,
  setAvailableUpdate,
  type DownloadEventLike,
  type FakeUpdate,
} from "./test/updater";

vi.mock("@tauri-apps/api/core", () => import("./test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("./test/tauri"));
vi.mock("@tauri-apps/plugin-updater", () => import("./test/updater"));
vi.mock("@tauri-apps/plugin-process", () => import("./test/updater"));
vi.mock("@tauri-apps/api/app", () => import("./test/updater"));

/** An update that downloads and installs without complaint. `emits` is what the
 * download reports on its way through. */
function updateThat(
  emits: DownloadEventLike[] = [],
  overrides: Partial<FakeUpdate> = {},
): FakeUpdate {
  return {
    version: "0.2.0",
    body: "Fixed the thing.",
    download: async (onEvent) => {
      for (const event of emits) onEvent(event);
    },
    install: async () => {},
    ...overrides,
  };
}

/** Render with the startup check off, which is every test that is not about
 * the startup check. */
function renderQuiet() {
  return renderHook(() => useUpdater({ checkOnMount: false }));
}

describe("useUpdater", () => {
  beforeEach(() => {
    resetTauriMocks();
    resetUpdaterMocks();
    onCommand("updater_prepare_install", () => null);
  });

  it("seeds this build's version, which is what Settings answers with", async () => {
    setAppVersion("1.4.2");
    const { result } = renderQuiet();
    await waitFor(() => expect(result.current.state.appVersion).toBe("1.4.2"));
  });

  it("does not check on mount when told not to", async () => {
    setAvailableUpdate(updateThat());
    const { result } = renderQuiet();
    await waitFor(() => expect(result.current.state.appVersion).not.toBeNull());
    expect(check).not.toHaveBeenCalled();
    expect(result.current.state.phase).toEqual({ status: "idle" });
  });

  it("checks on mount when asked, and reports a waiting release", async () => {
    setAvailableUpdate(updateThat());
    const { result } = renderHook(() => useUpdater({ checkOnMount: true }));
    await waitFor(() =>
      expect(result.current.state.phase).toEqual({
        status: "available",
        version: "0.2.0",
        notes: "Fixed the thing.",
      }),
    );
  });

  it("stays quiet when the startup check fails, rather than nagging about a check nobody asked for", async () => {
    failCheck("no route to host");
    const { result } = renderHook(() => useUpdater({ checkOnMount: true }));
    await waitFor(() => expect(check).toHaveBeenCalled());
    await waitFor(() => expect(result.current.state.phase).toEqual({ status: "idle" }));
  });

  it("reports up to date when a manual check finds nothing newer", async () => {
    const { result } = renderQuiet();
    await act(async () => {
      await result.current.check();
    });
    expect(result.current.state.phase).toEqual({ status: "upToDate" });
  });

  it("surfaces a manual check failure, unlike the startup one", async () => {
    failCheck("no route to host");
    const { result } = renderQuiet();
    await act(async () => {
      await result.current.check();
    });
    expect(result.current.state.phase).toEqual({
      status: "error",
      step: "check",
      message: "no route to host",
    });
  });

  it("follows a download through to ready", async () => {
    setAvailableUpdate(
      updateThat([
        { event: "Started", data: { contentLength: 20_000_000 } },
        { event: "Progress", data: { chunkLength: 5_000_000 } },
        { event: "Progress", data: { chunkLength: 5_000_000 } },
        { event: "Finished" },
      ]),
    );
    const { result } = renderQuiet();
    await act(async () => {
      await result.current.check();
    });
    await act(async () => {
      await result.current.download();
    });

    expect(result.current.state.phase).toEqual({
      status: "readyToInstall",
      version: "0.2.0",
    });
  });

  it("reports a download failure and leaves the install alone", async () => {
    setAvailableUpdate(
      updateThat([], {
        download: async () => {
          throw "the connection was reset";
        },
      }),
    );
    const { result } = renderQuiet();
    await act(async () => {
      await result.current.check();
    });
    await act(async () => {
      await result.current.download();
    });

    expect(result.current.state.phase).toEqual({
      status: "error",
      step: "download",
      message: "the connection was reset",
    });
  });

  it("reaps the child processes BEFORE handing over to the installer", async () => {
    // The ordering is the whole point: the updater exits this process from
    // inside install(), and a live kodabi-mcp.exe locks the binary NSIS is
    // about to replace. See src-tauri/src/updater_cmds.rs.
    const calls: string[] = [];
    onCommand("updater_prepare_install", () => {
      calls.push("reap");
      return null;
    });
    setAvailableUpdate(
      updateThat([], {
        install: async () => {
          calls.push("install");
        },
      }),
    );

    const { result } = renderQuiet();
    await act(async () => {
      await result.current.check();
    });
    await act(async () => {
      await result.current.install();
    });

    expect(calls).toEqual(["reap", "install"]);
    expect(invokedCommands()).toContain("updater_prepare_install");
    // The cross-platform tail. Windows never gets here, but nothing in the
    // hook may skip it on that assumption.
    expect(relaunch).toHaveBeenCalled();
  });

  it("reports an install failure", async () => {
    setAvailableUpdate(
      updateThat([], {
        install: async () => {
          throw "the installer could not replace kodabi.exe";
        },
      }),
    );
    const { result } = renderQuiet();
    await act(async () => {
      await result.current.check();
    });
    await act(async () => {
      await result.current.install();
    });

    expect(result.current.state.phase).toEqual({
      status: "error",
      step: "install",
      message: "the installer could not replace kodabi.exe",
    });
  });

  it("does nothing when asked to download or install with no update in hand", async () => {
    const { result } = renderQuiet();
    await act(async () => {
      await result.current.download();
      await result.current.install();
    });
    expect(result.current.state.phase).toEqual({ status: "idle" });
    expect(invokedCommands()).not.toContain("updater_prepare_install");
  });
});

describe("foldDownloadEvent", () => {
  const downloading: UpdaterPhase = {
    status: "downloading",
    version: "0.2.0",
    progress: { receivedBytes: 0, totalBytes: null },
  };

  it("takes the total from Started, when the server volunteered one", () => {
    const next = foldDownloadEvent(downloading, {
      event: "Started",
      data: { contentLength: 20_000_000 },
    });
    expect(next).toMatchObject({ progress: { receivedBytes: 0, totalBytes: 20_000_000 } });
  });

  it("leaves the total null when the response carried no content length", () => {
    const next = foldDownloadEvent(downloading, { event: "Started", data: {} });
    expect(next).toMatchObject({ progress: { totalBytes: null } });
  });

  it("accumulates chunks rather than replacing the count", () => {
    const first = foldDownloadEvent(downloading, {
      event: "Progress",
      data: { chunkLength: 300 },
    });
    const second = foldDownloadEvent(first, { event: "Progress", data: { chunkLength: 200 } });
    expect(second).toMatchObject({ progress: { receivedBytes: 500 } });
  });

  it("does not advance the phase on Finished, which the awaited download owns", () => {
    expect(foldDownloadEvent(downloading, { event: "Finished" })).toEqual(downloading);
  });

  it("ignores events that arrive outside a download", () => {
    const idle: UpdaterPhase = { status: "idle" };
    expect(foldDownloadEvent(idle, { event: "Progress", data: { chunkLength: 10 } })).toBe(idle);
  });
});
