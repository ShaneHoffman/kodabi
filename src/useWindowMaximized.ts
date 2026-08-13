import { useEffect, useState } from "react";
import { currentWindowOrNull } from "./windowControls";

/**
 * Whether the window is maximized, kept live so the caption button can show
 * Restore instead of Maximize.
 *
 * Seeded once on mount and re-read on every resize. A resize is the only way
 * the answer changes, which is what makes one subscription enough to cover all
 * four doors in: the caption button, a double-click on the drag region, Win+Up,
 * and a drag to the top edge. Outside Tauri there is no window to ask and the
 * bar stays in its un-maximized state (see `currentWindowOrNull`).
 */
export function useWindowMaximized(): boolean {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    const appWindow = currentWindowOrNull();
    if (!appWindow) return;

    let active = true;
    let unlisten: (() => void) | undefined;

    const sync = () => {
      appWindow
        .isMaximized()
        .then((value) => {
          if (active) setMaximized(value);
        })
        .catch(() => {});
    };

    sync();

    // Same mount/unmount race as `useTauriEvent`: the unlisten arrives a tick
    // after the subscription, so an unmount in between has to call it itself.
    appWindow
      .onResized(() => sync())
      .then((fn) => {
        if (active) unlisten = fn;
        else fn();
      })
      .catch(() => {});

    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  return maximized;
}
