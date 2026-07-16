import { useCallback, useEffect, useState } from "react";

const IS_MAC = /Mac|iPhone|iPad/.test(navigator.userAgent);

/** Platform-honest shortcut label — no ⌘ glyph on Windows. */
export const PALETTE_SHORTCUT_LABEL = IS_MAC ? "⌘K" : "Ctrl K";

/**
 * Owns the palette's open state and the global ⌘K / Ctrl-K toggle (Escape
 * closes). The listener is registered once per mount with its cleanup
 * removing the same reference, so StrictMode's mount→unmount→mount ends with
 * exactly one listener. This is the in-app shortcut — unrelated to the Rust
 * global capture hotkey.
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
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setOpen((prev) => !prev);
      } else if (event.key === "Escape") {
        setOpen(false);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  return { open, openPalette, closePalette };
}
