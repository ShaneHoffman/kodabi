import { useEffect, useRef } from "react";

/**
 * Own a dialog's focus hand-off: focus the element the dialog opens on, and on
 * close hand focus back to wherever it came from. The restore is guarded on
 * `isConnected` because the dialog's own action may have unmounted the opener
 * (a palette command that navigates away) — focusing a detached element would
 * silently drop focus to `<body>`.
 *
 * `findInitialFocus` runs once on mount and is read through a ref, so passing a
 * fresh inline closure each render is fine. Extracted from the identical
 * effects `CommandPalette` and `ConsentNudge` both hand-rolled; both have since
 * moved off it — the dialogs now get this from base-ui's `Dialog`, and the
 * rebuilt palette gets it from base-ui directly. Callerless, alongside
 * `Overlay`, pending its own removal ticket.
 */
export function useDialogFocus(findInitialFocus: () => HTMLElement | null): void {
  const findInitialFocusRef = useRef(findInitialFocus);
  findInitialFocusRef.current = findInitialFocus;

  useEffect(() => {
    const previous = document.activeElement;
    findInitialFocusRef.current()?.focus();
    return () => {
      if (previous instanceof HTMLElement && previous.isConnected) {
        previous.focus();
      }
    };
  }, []);
}
