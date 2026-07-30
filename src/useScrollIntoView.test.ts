import { renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useScrollIntoView } from "./useScrollIntoView";

/*
 * jsdom implements no layout, so nothing here can assert a measured scroll
 * position — and a test that pretended otherwise would be worse than none.
 * What is falsifiable is the part that actually breaks in practice: that the
 * call lands on the element the id names, carries `block: "nearest"` (a walk
 * one row past the edge must nudge, not recentre), and fires again exactly
 * when the hook's contract says it should.
 *
 * `src/test/setup.ts` installs a guarded no-op `scrollIntoView` on
 * Element.prototype, since jsdom ships none — so the spy below always has a
 * real method to wrap.
 */

function appendRow(id: string): HTMLElement {
  const row = document.createElement("li");
  row.id = id;
  document.body.append(row);
  return row;
}

afterEach(() => {
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

describe("useScrollIntoView", () => {
  it("scrolls the row its id names", () => {
    const first = appendRow("row-0");
    appendRow("row-1");
    const scrollIntoView = vi.spyOn(Element.prototype, "scrollIntoView");

    renderHook(() => useScrollIntoView("row-0"));

    expect(scrollIntoView).toHaveBeenCalledTimes(1);
    expect(scrollIntoView).toHaveBeenCalledWith({ block: "nearest" });
    expect(scrollIntoView.mock.contexts[0]).toBe(first);
  });

  it("scrolls nothing when there is no row to scroll", () => {
    // A collapsed list passes null. Scrolling anything then would yank the page
    // around for a highlight the user cannot see.
    appendRow("row-0");
    const scrollIntoView = vi.spyOn(Element.prototype, "scrollIntoView");

    renderHook(() => useScrollIntoView(null));

    expect(scrollIntoView).not.toHaveBeenCalled();
  });

  it("follows the highlight to a new row", () => {
    const first = appendRow("row-0");
    const second = appendRow("row-1");
    const scrollIntoView = vi.spyOn(Element.prototype, "scrollIntoView");
    const { rerender } = renderHook(
      ({ elementId }: { elementId: string }) => useScrollIntoView(elementId),
      { initialProps: { elementId: "row-0" } },
    );
    expect(scrollIntoView.mock.contexts[0]).toBe(first);

    rerender({ elementId: "row-1" });

    expect(scrollIntoView).toHaveBeenCalledTimes(2);
    expect(scrollIntoView.mock.contexts[1]).toBe(second);
  });

  it("re-scrolls when the list changed under a stable row id", () => {
    // The whole reason `refreshKey` exists: the command palette re-filters
    // while the highlight stays on row 0, so the id is unchanged but the row
    // under it is a different note. Without this the list would scroll once and
    // then never again for the rest of the search.
    const row = appendRow("row-0");
    const scrollIntoView = vi.spyOn(Element.prototype, "scrollIntoView");
    const { rerender } = renderHook(
      ({ refreshKey }: { refreshKey: number }) => useScrollIntoView("row-0", refreshKey),
      { initialProps: { refreshKey: 1 } },
    );
    expect(scrollIntoView).toHaveBeenCalledTimes(1);

    // Same id, same key: nothing changed, so nothing re-scrolls. This is the
    // contrast that makes the assertion below mean something.
    rerender({ refreshKey: 1 });
    expect(scrollIntoView).toHaveBeenCalledTimes(1);

    rerender({ refreshKey: 2 });

    expect(scrollIntoView).toHaveBeenCalledTimes(2);
    expect(scrollIntoView.mock.contexts[1]).toBe(row);
  });

  it("does not throw when the id matches nothing", () => {
    // The row can be absent for a render — a list that opened before its
    // options arrived. The optional chain is what keeps that from throwing.
    const scrollIntoView = vi.spyOn(Element.prototype, "scrollIntoView");

    expect(() => renderHook(() => useScrollIntoView("row-missing"))).not.toThrow();

    expect(scrollIntoView).not.toHaveBeenCalled();
  });
});
