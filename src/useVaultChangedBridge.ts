import { useTauriEvent } from "./useTauriEvent";
import { VAULT_CHANGED_EVENT } from "./events";
import { notifyVaultChanged } from "./useVaultQuery";

/**
 * Bridges the backend's app-wide `vault:changed` broadcast into this window's
 * per-webview `notifyVaultChanged` DOM bus, so a write from another window (the
 * quick-capture box) refreshes this window's note lists. The in-process bus
 * can't cross webviews; a Tauri event can. Mount once, at the shell root — it
 * keeps working while the main window is hidden to the tray (hidden ≠
 * destroyed).
 */
export function useVaultChangedBridge(): void {
  useTauriEvent(VAULT_CHANGED_EVENT, notifyVaultChanged);
}
