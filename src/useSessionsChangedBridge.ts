import { useTauriEvent } from "./useTauriEvent";
import { SESSIONS_CHANGED_EVENT } from "./events";
import { notifyVaultChanged } from "./useVaultQuery";

/**
 * Bridges the backend's `sessions:changed` broadcast (the retention sweep
 * deleted expired raw sessions) into this window's disk-refetch bus, so a
 * needs-attention list drops rows whose files the sweep just removed instead of
 * offering a retry that would fail. Reuses the vault-changed bus rather than
 * adding a second one: both mean "re-read what you have from disk", and the
 * sweep runs at most every few hours, so the extra refetch costs nothing.
 * Mount once, at the shell root.
 */
export function useSessionsChangedBridge(): void {
  useTauriEvent(SESSIONS_CHANGED_EVENT, notifyVaultChanged);
}
