import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { notifyVaultChanged } from "./useVaultQuery";

/** Backend event broadcast after a cross-window vault write (e.g. quick
 * capture). Mirrors `quick_capture::VAULT_CHANGED_EVENT`. */
const VAULT_CHANGED_EVENT = "vault:changed";

/**
 * Bridges the backend's app-wide `vault:changed` broadcast into this window's
 * per-webview `notifyVaultChanged` DOM bus, so a write from another window (the
 * quick-capture box) refreshes this window's note lists. The in-process bus
 * can't cross webviews; a Tauri event can. Mount once, at the shell root — it
 * keeps working while the main window is hidden to the tray (hidden ≠
 * destroyed).
 */
export function useVaultChangedBridge(): void {
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;

    listen(VAULT_CHANGED_EVENT, () => {
      if (active) notifyVaultChanged();
    }).then((fn) => {
      if (active) unlisten = fn;
      else fn();
    });

    return () => {
      active = false;
      unlisten?.();
    };
  }, []);
}
