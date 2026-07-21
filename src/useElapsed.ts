import { useEffect, useState } from "react";

/**
 * Seconds elapsed since `startedAt`, ticking once a second. Pass `null` to
 * stop the clock and reset it.
 *
 * A blessed bridge hook (.claude/rules/no-use-effect.md): it owns exactly one
 * external system — an interval — and clears it on every change and on
 * unmount. Nothing else in the app may call setInterval.
 *
 * It exists because the capture backend reports a *phase*, not a duration, and
 * the listening surfaces need a duration: a recording indicator that cannot
 * say how long it has been recording is asking the user to trust it about the
 * one thing they would want to check. Timing from the moment the phase went
 * live is accurate to within a tick, which is the resolution a `0:07` shows
 * anyway.
 */
export function useElapsed(startedAt: number | null): number {
  const [seconds, setSeconds] = useState(0);

  useEffect(() => {
    if (startedAt === null) {
      setSeconds(0);
      return;
    }
    // Set once immediately so the first second is not blank, then tick.
    const update = () => setSeconds(Math.floor((Date.now() - startedAt) / 1000));
    update();
    const id = window.setInterval(update, 1000);
    return () => window.clearInterval(id);
  }, [startedAt]);

  return seconds;
}

/** `m:ss`, the form a recording timer takes everywhere. Hours are deliberately
 * not handled: a capture that runs past 59 minutes reads `73:20`, which is
 * still correct and still sorts by eye. */
export function formatElapsed(seconds: number): string {
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${String(seconds % 60).padStart(2, "0")}`;
}
