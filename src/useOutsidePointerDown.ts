import { useEffect, useRef, type RefObject } from "react";

/**
 * Dismiss a transient surface when a pointer press lands outside it. The
 * listener is only attached while `active`, so a closed popup costs nothing and
 * can't fire a stale dismiss. `onOutsidePress` is read through a ref, so an
 * inline closure never re-attaches the listener.
 *
 * Listens on `pointerdown` (not `click`) because the press is the intent: the
 * surface should collapse as the gesture starts, before focus moves.
 */
export function useOutsidePointerDown(
  active: boolean,
  containerRef: RefObject<HTMLElement | null>,
  onOutsidePress: () => void,
): void {
  const onOutsidePressRef = useRef(onOutsidePress);
  onOutsidePressRef.current = onOutsidePress;

  useEffect(() => {
    if (!active) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!containerRef.current?.contains(event.target as Node)) {
        onOutsidePressRef.current();
      }
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [active, containerRef]);
}
