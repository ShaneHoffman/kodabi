import { useTauriEvent } from "./useTauriEvent";
import { LEDGER_CHANGED_EVENT } from "./events";
import { notifyVaultChanged } from "./useVaultQuery";

/**
 * Bridges the backend's `ledger:changed` broadcast onto this window's
 * `notifyVaultChanged` DOM bus, so every ledger-reading surface refetches after
 * a mutation the vault's own watcher will not announce (a snooze, an untrack, a
 * change to a meeting's tracking). A waive emits this too, alongside the
 * `vault:changed` its dated line earns; the relay makes the pair one refetch.
 *
 * Relaying onto the vault bus rather than carrying its own subscription is
 * deliberate: it reuses `useVaultQuery`'s response sequencing, so a slow reply
 * can never overwrite a newer one. Mount once, at the shell root. The
 * Commitments view held this locally while it was the only consumer; the note
 * view's enrollment panel is the second, and two copies would refetch twice for
 * one mutation.
 */
export function useLedgerChangedBridge(): void {
  useTauriEvent(LEDGER_CHANGED_EVENT, notifyVaultChanged);
}
