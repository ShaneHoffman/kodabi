import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { MODELS_STATE_EVENT } from "./events";
import { useModelDownload } from "./useModelDownload";
import {
  emitFromBackend,
  invokedCommands,
  listenerCount,
  onCommand,
  resetTauriMocks,
} from "./test/tauri";

vi.mock("@tauri-apps/api/core", () => import("./test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("./test/tauri"));

const MISSING = {
  ready: false,
  bytesRequired: 762_000_000,
  bytesPresent: 0,
  sets: [],
  downloading: false,
  modelsDir: "C:\\app\\.models",
};

const READY = { ...MISSING, ready: true, bytesRequired: 0 };

/** Render and wait for the seed read to land. */
async function renderSeeded(status: unknown = MISSING) {
  onCommand("model_status", () => status);
  const view = renderHook(() => useModelDownload());
  await waitFor(() => expect(view.result.current.state.status).not.toBe("unknown"));
  return view;
}

describe("useModelDownload", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("seeds from the backend rather than waiting for an event that may never come", async () => {
    const { result } = await renderSeeded();
    expect(result.current.state).toEqual({
      status: "missing",
      bytesRequired: 762_000_000,
    });
    expect(invokedCommands()).toContain("model_status");
  });

  it("stays unknown when the status read fails, rather than claiming a failure", async () => {
    // Not seeded: `invoke` rejects. A status we could not read is not evidence
    // that anything is wrong, and `unknown` is what stops a launch-time scare.
    const { result } = renderHook(() => useModelDownload());
    await act(async () => {
      await Promise.resolve();
    });
    expect(result.current.state.status).toBe("unknown");
  });

  it("follows a download through to ready", async () => {
    const { result } = await renderSeeded();

    act(() => {
      emitFromBackend(MODELS_STATE_EVENT, {
        status: "downloading",
        file: "parakeet-tdt-0.6b-v2-int8/encoder.int8.onnx",
        file_index: 0,
        file_count: 5,
        file_received: 100,
        file_total: 652,
        overall_received: 100,
        overall_total: 762,
      });
    });
    expect(result.current.state).toEqual({
      status: "downloading",
      progress: {
        file: "parakeet-tdt-0.6b-v2-int8/encoder.int8.onnx",
        fileIndex: 0,
        fileCount: 5,
        received: 100,
        total: 762,
        verifying: false,
      },
    });

    onCommand("model_status", () => READY);
    act(() => {
      emitFromBackend(MODELS_STATE_EVENT, { status: "ready" });
    });
    await waitFor(() => expect(result.current.state.status).toBe("ready"));
  });

  it("keeps the byte figures while a finished file is being verified", async () => {
    const { result } = await renderSeeded();
    act(() => {
      emitFromBackend(MODELS_STATE_EVENT, {
        status: "downloading",
        file: "a.onnx",
        file_index: 0,
        file_count: 1,
        file_received: 762,
        file_total: 762,
        overall_received: 762,
        overall_total: 762,
      });
      emitFromBackend(MODELS_STATE_EVENT, { status: "verifying", file: "a.onnx" });
    });

    expect(result.current.state).toMatchObject({
      status: "downloading",
      progress: { received: 762, total: 762, verifying: true },
    });
  });

  it("stays in the download for a retry, which usually just resumes", async () => {
    const { result } = await renderSeeded();
    act(() => {
      emitFromBackend(MODELS_STATE_EVENT, {
        status: "retrying",
        file: "a.onnx",
        attempt: 1,
        max_attempts: 4,
        message: "connection reset",
      });
    });
    expect(result.current.state.status).toBe("downloading");
  });

  it("surfaces a download failure, which is a different thing from a failed read", async () => {
    const { result } = await renderSeeded();
    act(() => {
      emitFromBackend(MODELS_STATE_EVENT, {
        status: "error",
        message: "could not download encoder.int8.onnx: no route to host",
      });
    });
    expect(result.current.state).toEqual({
      status: "error",
      message: "could not download encoder.int8.onnx: no route to host",
    });
  });

  it("shows the download immediately on start, before any event arrives", async () => {
    const { result } = await renderSeeded();
    onCommand("download_models", () => null);

    await act(async () => {
      await result.current.start();
    });

    expect(result.current.state).toEqual({ status: "downloading", progress: null });
    expect(invokedCommands()).toContain("download_models");
  });

  it("reports a command-level failure to start, which emits no event of its own", async () => {
    const { result } = await renderSeeded();
    onCommand("download_models", () => {
      throw "the models directory is unavailable";
    });

    await act(async () => {
      await result.current.start();
    });

    expect(result.current.state).toEqual({
      status: "error",
      message: "the models directory is unavailable",
    });
  });

  it("re-reads the truth after a cancel rather than guessing what survived", async () => {
    const { result } = await renderSeeded();
    onCommand("cancel_model_download", () => null);
    // Half of it landed and is resumable, which only the backend knows.
    onCommand("model_status", () => ({
      ...MISSING,
      bytesRequired: 762_000_000,
      bytesPresent: 300_000_000,
    }));

    await act(async () => {
      await result.current.cancel();
    });
    act(() => {
      emitFromBackend(MODELS_STATE_EVENT, { status: "cancelled" });
    });

    await waitFor(() =>
      expect(result.current.state).toEqual({
        status: "missing",
        bytesRequired: 462_000_000,
      }),
    );
    expect(invokedCommands()).toContain("cancel_model_download");
  });

  it("unsubscribes on unmount", async () => {
    const { unmount } = await renderSeeded();
    expect(listenerCount(MODELS_STATE_EVENT)).toBe(1);
    unmount();
    await waitFor(() => expect(listenerCount(MODELS_STATE_EVENT)).toBe(0));
  });
});
