import { fireEvent, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { type RefObject } from "react";
import { useOutsidePointerDown } from "./useOutsidePointerDown";

/*
 * A dismiss-on-outside-press hook has two failure modes that only bite in
 * production: it leaks a document listener that fires for a surface which has
 * already closed, or it re-subscribes on every render so the count creeps.
 * Both are invisible to a component test, which is why the hook is covered
 * directly.
 *
 * `RefObject<HTMLElement | null>` is satisfied structurally, so these tests
 * hand the hook a plain object rather than rendering a probe component — the
 * container is real DOM either way, and no probe means no useEffect outside a
 * blessed bridge hook (.claude/rules/no-use-effect.md).
 *
 * The ref object is always hoisted out of the render callback: it is in the
 * effect's dependency list, so a fresh literal per render would re-subscribe
 * and quietly invalidate the re-subscription assertions below.
 */

function mountContainer(): { containerRef: RefObject<HTMLElement | null>; child: HTMLElement } {
  const container = document.createElement("div");
  const child = document.createElement("button");
  container.append(child);
  document.body.append(container);
  return { containerRef: { current: container }, child };
}

/**
 * How many pointerdown listeners the hook has registered on `document`, read off
 * a `vi.spyOn(document, "addEventListener")`. Typed structurally because
 * addEventListener is overloaded, and naming it through `vi.spyOn`'s generics
 * does not satisfy the key constraint.
 */
function pointerdownCount(spy: { mock: { calls: unknown[][] } }): number {
  return spy.mock.calls.filter(([type]) => type === "pointerdown").length;
}

afterEach(() => {
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

describe("useOutsidePointerDown", () => {
  it("dismisses on a press outside the container", () => {
    const { containerRef } = mountContainer();
    const onOutsidePress = vi.fn();
    renderHook(() => useOutsidePointerDown(true, containerRef, onOutsidePress));

    fireEvent.pointerDown(document.body);

    expect(onOutsidePress).toHaveBeenCalledTimes(1);
  });

  it("leaves a press inside the container alone", () => {
    // The surface must survive its own controls being pressed, or a menu would
    // close before the row under the finger could act on the press.
    const { containerRef, child } = mountContainer();
    const onOutsidePress = vi.fn();
    renderHook(() => useOutsidePointerDown(true, containerRef, onOutsidePress));

    fireEvent.pointerDown(child);

    expect(onOutsidePress).not.toHaveBeenCalled();
  });

  it("listens for nothing while it is inactive", () => {
    // A closed popup costs nothing and cannot fire a stale dismiss.
    const { containerRef } = mountContainer();
    const onOutsidePress = vi.fn();
    const addEventListener = vi.spyOn(document, "addEventListener");
    renderHook(() => useOutsidePointerDown(false, containerRef, onOutsidePress));

    fireEvent.pointerDown(document.body);

    expect(onOutsidePress).not.toHaveBeenCalled();
    expect(pointerdownCount(addEventListener)).toBe(0);
  });

  it("stops listening when the surface closes", () => {
    const { containerRef } = mountContainer();
    const onOutsidePress = vi.fn();
    const { rerender } = renderHook(
      ({ active }: { active: boolean }) =>
        useOutsidePointerDown(active, containerRef, onOutsidePress),
      { initialProps: { active: true } },
    );
    fireEvent.pointerDown(document.body);
    expect(onOutsidePress).toHaveBeenCalledTimes(1);

    rerender({ active: false });
    fireEvent.pointerDown(document.body);

    expect(onOutsidePress).toHaveBeenCalledTimes(1);
  });

  it("stops listening when unmounted", () => {
    // A leaked document listener is exactly what the bridge-hook rule exists to
    // prevent, and it would fire against a surface that no longer exists.
    const { containerRef } = mountContainer();
    const onOutsidePress = vi.fn();
    const { unmount } = renderHook(() =>
      useOutsidePointerDown(true, containerRef, onOutsidePress),
    );

    unmount();
    fireEvent.pointerDown(document.body);

    expect(onOutsidePress).not.toHaveBeenCalled();
  });

  it("calls the latest callback without re-subscribing", () => {
    // The callback is read through a ref assigned during render, so a call site
    // can pass a fresh inline closure every render — as Select does — without
    // the listener being torn down and re-attached each time.
    const { containerRef } = mountContainer();
    const firstCallback = vi.fn();
    const secondCallback = vi.fn();
    const addEventListener = vi.spyOn(document, "addEventListener");
    const { rerender } = renderHook(
      ({ onOutsidePress }: { onOutsidePress: () => void }) =>
        useOutsidePointerDown(true, containerRef, onOutsidePress),
      { initialProps: { onOutsidePress: firstCallback } },
    );
    expect(pointerdownCount(addEventListener)).toBe(1);

    rerender({ onOutsidePress: secondCallback });
    fireEvent.pointerDown(document.body);

    expect(pointerdownCount(addEventListener)).toBe(1);
    expect(secondCallback).toHaveBeenCalledTimes(1);
    expect(firstCallback).not.toHaveBeenCalled();
  });

  it("treats a press as outside when the container is gone", () => {
    // `?.contains` yields undefined for a detached ref, which counts as
    // outside. Documenting the branch rather than leaving it to be discovered:
    // a surface whose container has unmounted should dismiss, not wedge open.
    const containerRef: RefObject<HTMLElement | null> = { current: null };
    const onOutsidePress = vi.fn();
    renderHook(() => useOutsidePointerDown(true, containerRef, onOutsidePress));

    fireEvent.pointerDown(document.body);

    expect(onOutsidePress).toHaveBeenCalledTimes(1);
  });
});
