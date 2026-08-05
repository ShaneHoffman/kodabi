import { useSyncExternalStore } from "react";
import { REDUCE_MOTION_ATTRIBUTE } from "./reduceMotion";

/**
 * Whether the app should hold still, read from BOTH channels the preference
 * has — the OS `prefers-reduced-motion` query and the in-app override
 * `reduceMotion.ts` reflects onto `<html>` (docs/DESIGN_SYSTEM.md §4).
 *
 * It exists because the `motion` package cannot answer this. Its own
 * `useReducedMotion()` reads the media query and nothing else, so a user who
 * turned the app's own switch on would keep every spring and slide that
 * package drives — the one place the preference is expressed in-app is the one
 * place it would not be honoured. CSS already reads both, through the
 * `motion-reduce:` variant redefined in `index.css`; this is that same union,
 * for the animations JavaScript drives.
 *
 * NOT an effect and not a bridge hook: `useSyncExternalStore` is React's own
 * answer for reading a value that lives outside React, and the subscription it
 * takes is the store's, not a lifecycle's (.claude/rules/no-use-effect.md
 * governs `useEffect`). The snapshot is a boolean — a primitive, so there is
 * nothing to memoize and no tearing to guard against.
 */

const REDUCED_QUERY = "(prefers-reduced-motion: reduce)";

function subscribe(onStoreChange: () => void): () => void {
  // The attribute covers both ways the in-app switch moves: the Settings
  // toggle in this window, and another window's change arriving as a `storage`
  // event — `reduceMotion.ts` funnels both through `reflectReduceMotion`, so
  // watching the attribute catches them without a second listener.
  const observer = new MutationObserver(onStoreChange);
  observer.observe(document.documentElement, {
    attributes: true,
    attributeFilter: [REDUCE_MOTION_ATTRIBUTE],
  });

  // The OS preference never touches the attribute — it is honoured directly in
  // CSS — so it needs its own listener or a mid-session change would go unseen.
  const query = window.matchMedia(REDUCED_QUERY);
  query.addEventListener("change", onStoreChange);

  return () => {
    observer.disconnect();
    query.removeEventListener("change", onStoreChange);
  };
}

function getSnapshot(): boolean {
  return (
    document.documentElement.getAttribute(REDUCE_MOTION_ATTRIBUTE) === "on" ||
    window.matchMedia(REDUCED_QUERY).matches
  );
}

/** Read the preference. Re-renders the caller when either channel changes. */
export function useReduceMotion(): boolean {
  return useSyncExternalStore(subscribe, getSnapshot);
}
