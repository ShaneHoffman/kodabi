import { useEffect, useState } from "react";

/**
 * Returns `value` only after it has held steady for `delayMs`. Rapid changes
 * (e.g. a VAD flapping idle↔listening on speech pauses) collapse to the final
 * settled value, so anything driven off this — notably an `aria-live` region —
 * isn't re-triggered on every intermediate flip.
 */
export function useDebouncedValue<T>(value: T, delayMs: number): T {
  const [settled, setSettled] = useState(value);

  useEffect(() => {
    const id = setTimeout(() => setSettled(value), delayMs);
    return () => clearTimeout(id);
  }, [value, delayMs]);

  return settled;
}
