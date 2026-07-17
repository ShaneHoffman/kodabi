import { useCallback, useEffect, useState } from "react";

const IS_MAC = /Mac|iPhone|iPad/.test(navigator.userAgent);

/** Platform-honest shortcut label — no ⌘ glyph on Windows. */
export const PALETTE_SHORTCUT_LABEL = IS_MAC ? "⌘K" : "Ctrl K";

/**
 * Owns the palette's open state and the global ⌘K / Ctrl-K toggle. Escape
 * belongs to the palette's own keydown handler (mounted only while open),
 * so future overlays each own their dismissal instead of contending with an
 * always-on window listener. The listener is registered once per mount with
 * its cleanup removing the same reference, so StrictMode's
 * mount→unmount→mount ends with exactly one listener. This is the in-app
 * shortcut — unrelated to the Rust global capture hotkey.
 */
export function useCommandPalette(): {
  open: boolean;
  openPalette: () => void;
  closePalette: () => void;
} {
  const [open, setOpen] = useState(false);

  const openPalette = useCallback(() => setOpen(true), []);
  const closePalette = useCallback(() => setOpen(false), []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      // Exactly the advertised chord: alt/shift excluded so Ctrl+Shift+K
      // stays free and AltGr+K (ctrl+alt on Windows) still types its
      // character on layouts that map it. Composition keys stay the IME's.
      if (event.isComposing || event.altKey || event.shiftKey) return;
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setOpen((prev) => !prev);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  return { open, openPalette, closePalette };
}
