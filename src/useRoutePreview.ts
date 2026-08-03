import { useEffect, useRef, useState } from "react";

import {
  previewQuickCaptureRoute,
  type QuickCaptureRoutePreview,
} from "./quickCapture";
import { useDebouncedValue } from "./useDebouncedValue";

/**
 * The router's live guess for a draft: where Enter would file it, refreshed as
 * the user types.
 *
 * A blessed bridge hook (.claude/rules/no-use-effect.md): it owns exactly one
 * external system — the `quick_capture_route_preview` IPC round trip — and
 * sequences its responses so a slow early one can't overwrite a newer answer.
 *
 * The guess is a hint, not a status. An empty draft has nothing to guess about
 * and asks nothing; a failed call clears the guess silently rather than putting
 * an error where a project name goes, because the window's actual job (filing
 * the note) is unaffected by the router being briefly unreachable.
 */
export const ROUTE_PREVIEW_DEBOUNCE_MS = 300;

export function useRoutePreview(text: string): QuickCaptureRoutePreview | null {
  const trimmed = text.trim();
  const settledText = useDebouncedValue(trimmed, ROUTE_PREVIEW_DEBOUNCE_MS);
  const [guess, setGuess] = useState<QuickCaptureRoutePreview | null>(null);
  // The `useVaultQuery` ticket: only the latest request may land.
  const seq = useRef(0);

  useEffect(() => {
    const ticket = ++seq.current;
    if (!settledText) {
      setGuess(null);
      return;
    }
    previewQuickCaptureRoute(settledText)
      .then((preview) => {
        if (ticket === seq.current) setGuess(preview);
      })
      .catch(() => {
        if (ticket === seq.current) setGuess(null);
      });
  }, [settledText]);

  // Derived from the *un*-debounced draft: clearing the box drops the guess on
  // the keystroke, rather than leaving a stale project name for another 300ms.
  return trimmed ? guess : null;
}
