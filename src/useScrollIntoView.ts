import { useEffect } from "react";

/**
 * Keep an active-descendant row visible as a virtual highlight walks past a
 * scrolling list's edge. The element is found by id rather than a ref because
 * that is how the aria-activedescendant widgets in this app already address
 * their rows (`CommandPalette`, `Select`); pass `null` to scroll nothing (a
 * collapsed list).
 *
 * `refreshKey` is not used by the scroll itself — it exists so a caller whose
 * list *content* can change under a stable option id (the palette re-filtering
 * while the highlight stays on row 0) still re-scrolls. Callers whose id fully
 * encodes their state can omit it.
 */
export function useScrollIntoView(elementId: string | null, refreshKey?: unknown): void {
  useEffect(() => {
    if (elementId === null) return;
    void refreshKey;
    document.getElementById(elementId)?.scrollIntoView({ block: "nearest" });
  }, [elementId, refreshKey]);
}
