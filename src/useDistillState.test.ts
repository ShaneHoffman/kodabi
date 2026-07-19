import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DISTILL_STATE_EVENT } from "./events";
import type { CapturePhase } from "./useCaptureState";
import { useDistillState } from "./useDistillState";
import { emit, listenerCount, resetTauriMocks } from "./test/tauri";

vi.mock("@tauri-apps/api/core", () => import("./test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("./test/tauri"));

const SESSION = "sessions/2026-07-01T10-00-00Z-team-sync.jsonl";

/** Deliver a `distill:state` payload the way the backend would. */
function emitDistill(payload: unknown): void {
  act(() => {
    emit(DISTILL_STATE_EVENT, payload);
  });
}

/** Let the `listen(...).then(...)` subscription settle. */
async function flush(): Promise<void> {
  await act(async () => {});
}

describe("useDistillState", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("starts idle", () => {
    const { result } = renderHook(() => useDistillState("idle"));

    expect(result.current).toEqual({ status: "idle" });
  });

  it("surfaces a saved outcome", () => {
    const { result } = renderHook(() => useDistillState("idle"));

    const saved = {
      status: "saved",
      path: "Projects/paradise-golf/team-sync.md",
      session_path: SESSION,
    };
    emitDistill(saved);

    expect(result.current).toEqual(saved);
  });

  it("surfaces a skipped outcome with its reason", () => {
    const { result } = renderHook(() => useDistillState("idle"));

    emitDistill({
      status: "skipped",
      reason: "nothing distillable",
      session_path: SESSION,
    });

    expect(result.current).toEqual({
      status: "skipped",
      reason: "nothing distillable",
      session_path: SESSION,
    });
  });

  it("surfaces an error outcome with its message", () => {
    const { result } = renderHook(() => useDistillState("idle"));

    emitDistill({
      status: "error",
      message: "the model is unreachable",
      session_path: SESSION,
    });

    expect(result.current).toEqual({
      status: "error",
      message: "the model is unreachable",
      session_path: SESSION,
    });
  });

  it("resets to idle when a new capture starts listening", () => {
    const { result, rerender } = renderHook(
      ({ phase }: { phase: CapturePhase }) => useDistillState(phase),
      { initialProps: { phase: "idle" satisfies CapturePhase as CapturePhase } },
    );

    emitDistill({ status: "saved", path: "Inbox/stray.md", session_path: SESSION });
    expect(result.current.status).toBe("saved");

    rerender({ phase: "listening" });

    expect(result.current).toEqual({ status: "idle" });
  });

  it("drops a terminal event that lands while a capture is listening", () => {
    const { result, rerender } = renderHook(
      ({ phase }: { phase: CapturePhase }) => useDistillState(phase),
      { initialProps: { phase: "listening" satisfies CapturePhase as CapturePhase } },
    );

    // The previous meeting's distill finishing mid-way through the next
    // recording: adopting it would label the new capture with the old outcome.
    emitDistill({
      status: "saved",
      path: "Projects/paradise-golf/last-week.md",
      session_path: SESSION,
    });
    expect(result.current).toEqual({ status: "idle" });

    // Still dropped, not merely deferred: the event is gone, not queued.
    rerender({ phase: "idle" });
    expect(result.current).toEqual({ status: "idle" });
  });

  it("logs a routing_fallback warning without adopting it as label state", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const { result } = renderHook(() => useDistillState("idle"));

    emitDistill({
      status: "routing_fallback",
      message: "routing signals unavailable",
      session_path: SESSION,
    });

    expect(result.current).toEqual({ status: "idle" });
    expect(warn).toHaveBeenCalledTimes(1);
    expect(warn.mock.calls[0][0]).toContain("routing signals unavailable");
  });

  it("unlistens when unmounted before the subscription resolves", async () => {
    const { unmount } = renderHook(() => useDistillState("idle"));

    unmount();
    await flush();

    expect(listenerCount(DISTILL_STATE_EVENT)).toBe(0);
  });

  it("ignores an event delivered after unmount", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const { unmount } = renderHook(() => useDistillState("idle"));
    await flush();

    unmount();
    emitDistill({ status: "saved", path: "Inbox/late.md", session_path: SESSION });

    // Nothing to assert on state (the hook is gone); what must hold is that no
    // late branch ran — no warning, no act-outside-render error.
    expect(warn).not.toHaveBeenCalled();
  });
});
