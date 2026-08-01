import { useEffect } from "react";

/**
 * Keep a row visible as a virtual highlight walks past a scrolling list's edge.
 * The element is found by id rather than a ref because that is how `Select`,
 * the aria-activedescendant widget this exists for, addresses its rows; pass
 * `null` to scroll nothing (a collapsed list). `ChatView` uses it for the same
 * mechanic with a different meaning: hold the live end of the log in view.
 *
 * `refreshKey` is not used by the scroll itself — it exists so a caller whose
 * *content* can change under a stable element id (the chat log growing while
 * the end marker stays put) still re-scrolls. Callers whose id fully encodes
 * their state can omit it.
 */
export function useScrollIntoView(elementId: string | null, refreshKey?: unknown): void {
  useEffect(() => {
    if (elementId === null) return;
    void refreshKey;
    document.getElementById(elementId)?.scrollIntoView({ block: "nearest" });
  }, [elementId, refreshKey]);
}
