import { describe, expect, it, afterEach } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { setMediaMatches } from "./test/media";
import { applyReduceMotion } from "./reduceMotion";
import { useReduceMotion } from "./useReduceMotion";

/*
 * The union is the whole point of this hook, so both channels are tested
 * separately and then together: the `motion` package sees only the media
 * query, and a regression here would silently un-honour the in-app switch.
 */

afterEach(() => {
  applyReduceMotion(false);
});

describe("useReduceMotion", () => {
  it("is off when neither channel asks for it", () => {
    const { result } = renderHook(() => useReduceMotion());
    expect(result.current).toBe(false);
  });

  // Awaited because a MutationObserver delivers its records as a microtask,
  // in the browser as much as here: the switch is read on the next tick, not
  // in the same one that flipped it.
  it("follows the in-app switch, which is what motion's own hook cannot see", async () => {
    const { result } = renderHook(() => useReduceMotion());

    await act(async () => {
      applyReduceMotion(true);
    });
    expect(result.current).toBe(true);

    await act(async () => {
      applyReduceMotion(false);
    });
    expect(result.current).toBe(false);
  });

  it("follows the OS preference", () => {
    const { result } = renderHook(() => useReduceMotion());

    act(() => {
      setMediaMatches("(prefers-reduced-motion: reduce)", true);
    });
    expect(result.current).toBe(true);
  });

  it("stays on while either channel still asks for it", async () => {
    const { result } = renderHook(() => useReduceMotion());

    await act(async () => {
      setMediaMatches("(prefers-reduced-motion: reduce)", true);
      applyReduceMotion(true);
    });
    expect(result.current).toBe(true);

    // The in-app switch can only ever ADD reduction, so turning it off while
    // the OS still asks for it must not un-reduce the app.
    await act(async () => {
      applyReduceMotion(false);
    });
    expect(result.current).toBe(true);
  });
});
