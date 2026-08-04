import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ROUTE_PREVIEW_DEBOUNCE_MS, useRoutePreview } from "./useRoutePreview";
import type { QuickCaptureRoutePreview } from "./quickCapture";
import { invoke, invokedCommands, onCommand, resetTauriMocks } from "./test/tauri";

vi.mock("@tauri-apps/api/core", () => import("./test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("./test/tauri"));

const COMMAND = "quick_capture_route_preview";

function guess(project: string | null): QuickCaptureRoutePreview {
  return { project, confidence: project ? 0.8 : 0.1 };
}

/** Let the debounce elapse and the invoke resolve. */
async function settle(): Promise<void> {
  await act(async () => {
    vi.advanceTimersByTime(ROUTE_PREVIEW_DEBOUNCE_MS);
  });
}

describe("useRoutePreview", () => {
  beforeEach(() => {
    resetTauriMocks();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("asks the router only once the draft holds steady", async () => {
    onCommand(COMMAND, () => guess("briarwood-golf"));
    // Mounts empty, as the window does on every pop.
    const { result, rerender } = renderHook(({ text }) => useRoutePreview(text), {
      initialProps: { text: "" },
    });

    // Mid-typing: the router is not asked on every keystroke, or a vault scan
    // runs per character.
    rerender({ text: "b" });
    rerender({ text: "bu" });
    rerender({ text: "bunker edge" });
    expect(invokedCommands()).not.toContain(COMMAND);

    await settle();
    expect(invoke).toHaveBeenCalledWith(COMMAND, { text: "bunker edge" });
    expect(invokedCommands().filter((name) => name === COMMAND)).toHaveLength(1);
    expect(result.current).toEqual(guess("briarwood-golf"));
  });

  it("asks nothing for an empty draft, and clears any guess at once", async () => {
    onCommand(COMMAND, () => guess("briarwood-golf"));
    const { result, rerender } = renderHook(({ text }) => useRoutePreview(text), {
      initialProps: { text: "" },
    });
    rerender({ text: "bunker edge" });
    await settle();
    expect(result.current).not.toBeNull();

    // Cleared on the keystroke rather than a debounce later: an empty box has
    // no destination, and a leftover project name would be a claim about
    // nothing.
    rerender({ text: "   " });
    expect(result.current).toBeNull();

    invoke.mockClear();
    await settle();
    expect(invokedCommands()).not.toContain(COMMAND);
  });

  it("never lets a slow early answer overwrite a newer one", async () => {
    // Two in-flight calls settled by hand, oldest resolving last — the ordering
    // IPC gives no guarantee about.
    let resolveFirst!: (value: QuickCaptureRoutePreview) => void;
    let resolveSecond!: (value: QuickCaptureRoutePreview) => void;
    const first = new Promise<QuickCaptureRoutePreview>((resolve) => {
      resolveFirst = resolve;
    });
    const second = new Promise<QuickCaptureRoutePreview>((resolve) => {
      resolveSecond = resolve;
    });

    onCommand(COMMAND, () => first);
    const { result, rerender } = renderHook(({ text }) => useRoutePreview(text), {
      initialProps: { text: "" },
    });
    rerender({ text: "first draft" });
    await settle();

    onCommand(COMMAND, () => second);
    rerender({ text: "second draft" });
    await settle();

    await act(async () => {
      resolveSecond(guess("riverbend-deck"));
    });
    await act(async () => {
      resolveFirst(guess("briarwood-golf"));
    });

    expect(result.current).toEqual(guess("riverbend-deck"));
  });

  it("clears the guess silently when the router is unreachable", async () => {
    onCommand(COMMAND, () => {
      throw new Error("no knowledge base configured");
    });
    const { result, rerender } = renderHook(({ text }) => useRoutePreview(text), {
      initialProps: { text: "" },
    });
    rerender({ text: "bunker edge" });

    await settle();

    // A hint, not a status: the window's actual job — filing the note — is
    // unaffected, so a failure shows no chip rather than an error where a
    // project name goes.
    expect(result.current).toBeNull();
  });
});
