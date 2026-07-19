import { useEffect, useRef } from "react";

/**
 * Run an action once after a delay, cancelled on unmount or when the delay
 * changes. Pass `null` to disarm — that is how a caller expresses "not now"
 * without a conditional hook call, and re-arming (null → a delay) restarts the
 * timer from zero.
 *
 * The imperative sibling of `useDebouncedValue`: that one debounces a value,
 * this one schedules an effect. `callback` is read through a ref, so an inline
 * closure never restarts the timer.
 */
export function useTimeout(callback: () => void, delayMs: number | null): void {
  const callbackRef = useRef(callback);
  callbackRef.current = callback;

  useEffect(() => {
    if (delayMs === null) return;
    const timer = setTimeout(() => callbackRef.current(), delayMs);
    return () => clearTimeout(timer);
  }, [delayMs]);
}
