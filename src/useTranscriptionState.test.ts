import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TRANSCRIPTION_STATE_EVENT } from "./events";
import type { CapturePhase } from "./useCaptureState";
import { useTranscriptionState } from "./useTranscriptionState";
import { emitFromBackend, listenerCount, resetTauriMocks } from "./test/tauri";

vi.mock("@tauri-apps/api/core", () => import("./test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("./test/tauri"));

/** Deliver a `transcription:state` payload the way the backend would. */
function emitTranscription(payload: unknown): void {
  act(() => {
    emitFromBackend(TRANSCRIPTION_STATE_EVENT, payload);
  });
}

/** Let the `listen(...).then(...)` subscription settle. */
async function flush(): Promise<void> {
  await act(async () => {});
}

describe("useTranscriptionState", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("starts idle", () => {
    const { result } = renderHook(() => useTranscriptionState("idle"));

    expect(result.current).toEqual({ status: "idle" });
  });

  it("carries the progress figures through unchanged", () => {
    const { result } = renderHook(() => useTranscriptionState("idle"));

    emitTranscription({ status: "transcribing", seconds_processed: 750, total_seconds: 3484 });

    expect(result.current).toEqual({
      status: "transcribing",
      seconds_processed: 750,
      total_seconds: 3484,
    });
  });

  it("replaces state wholesale on each tick, so the latest figures win", () => {
    const { result } = renderHook(() => useTranscriptionState("idle"));

    emitTranscription({ status: "transcribing", seconds_processed: 10, total_seconds: 3484 });
    emitTranscription({ status: "transcribing", seconds_processed: 20, total_seconds: 3484 });

    expect(result.current).toEqual({
      status: "transcribing",
      seconds_processed: 20,
      total_seconds: 3484,
    });
  });

  it("surfaces a saved outcome", () => {
    const { result } = renderHook(() => useTranscriptionState("idle"));

    emitTranscription({ status: "saved", path: "sessions/team-sync.jsonl" });

    expect(result.current).toEqual({ status: "saved", path: "sessions/team-sync.jsonl" });
  });

  it("surfaces an error outcome with its message", () => {
    const { result } = renderHook(() => useTranscriptionState("idle"));

    emitTranscription({ status: "error", message: "the model is unreachable" });

    expect(result.current).toEqual({ status: "error", message: "the model is unreachable" });
  });

  it("accepts queued and transcribing even while a capture is listening", () => {
    // Both can beat the stop's own `capture:state` broadcast — `queued` fires
    // before the worker has even blocked on the lock — and a lock-queued
    // predecessor's events are true whatever the current phase reads.
    const { result, rerender } = renderHook(
      ({ phase }: { phase: CapturePhase }) => useTranscriptionState(phase),
      { initialProps: { phase: "listening" satisfies CapturePhase as CapturePhase } },
    );

    emitTranscription({ status: "queued" });
    expect(result.current).toEqual({ status: "queued" });

    emitTranscription({ status: "transcribing", seconds_processed: 0, total_seconds: 60 });
    expect(result.current).toEqual({
      status: "transcribing",
      seconds_processed: 0,
      total_seconds: 60,
    });

    rerender({ phase: "idle" });
    expect(result.current.status).toBe("transcribing");
  });

  it("drops a terminal event that lands while a capture is listening", () => {
    const { result, rerender } = renderHook(
      ({ phase }: { phase: CapturePhase }) => useTranscriptionState(phase),
      { initialProps: { phase: "listening" satisfies CapturePhase as CapturePhase } },
    );

    // The previous meeting's cleanup finishing mid-way through the next
    // recording: adopting it would label the new capture with the old outcome.
    emitTranscription({ status: "saved", path: "sessions/last-week.jsonl" });
    expect(result.current).toEqual({ status: "idle" });

    // Dropped, not deferred.
    rerender({ phase: "idle" });
    expect(result.current).toEqual({ status: "idle" });
  });

  it("resets to idle when a new capture starts listening", () => {
    const { result, rerender } = renderHook(
      ({ phase }: { phase: CapturePhase }) => useTranscriptionState(phase),
      { initialProps: { phase: "idle" satisfies CapturePhase as CapturePhase } },
    );

    emitTranscription({ status: "saved", path: "sessions/team-sync.jsonl" });
    expect(result.current.status).toBe("saved");

    rerender({ phase: "listening" });

    expect(result.current).toEqual({ status: "idle" });
  });

  it("unlistens when unmounted before the subscription resolves", async () => {
    const { unmount } = renderHook(() => useTranscriptionState("idle"));

    unmount();
    await flush();

    expect(listenerCount(TRANSCRIPTION_STATE_EVENT)).toBe(0);
  });
});
