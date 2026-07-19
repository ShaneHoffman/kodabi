import { useCallback, useEffect, useRef, useState } from "react";

/*
 * Vault change signal — a per-webview fan-out: something changed on disk, so
 * every disk-reading hook refetches. Its sole writer is `useVaultChangedBridge`,
 * which relays the backend `vault:changed` event; that event is now driven by
 * the Phase 2 file watcher, which observes every note write (in-app, external
 * editor, or git) and broadcasts once its debounce settles. Call sites don't
 * announce writes themselves anymore — the watcher does.
 */

const VAULT_CHANGED_EVENT = "kodabi:vault-changed";

export function notifyVaultChanged(): void {
  window.dispatchEvent(new Event(VAULT_CHANGED_EVENT));
}

/** Subscribes `fn` to vault changes; returns the cleanup. */
export function onVaultChanged(fn: () => void): () => void {
  window.addEventListener(VAULT_CHANGED_EVENT, fn);
  return () => window.removeEventListener(VAULT_CHANGED_EVENT, fn);
}

export type VaultQuery<T> = {
  data: T | null;
  loading: boolean;
  error: string | null;
  /** Applies externally obtained data (e.g. a save's echo). Invalidates any
   * in-flight fetch so a stale response can't overwrite it. */
  setData: (data: T) => void;
};

/**
 * The one fetch-from-vault state machine, shared by every disk-reading hook:
 * fetch on mount, refetch on every vault change, surface errors. Responses
 * are sequenced — only the most recently issued request (or `setData` call)
 * may land — because overlapping IPC calls resolve in no guaranteed order,
 * and a stale response must not overwrite newer state.
 *
 * `fetcher` identity drives refetching: wrap it in `useCallback` keyed on its
 * inputs, so a changed input restarts the query and a re-render doesn't.
 */
export function useVaultQuery<T>(fetcher: () => Promise<T>): VaultQuery<T> {
  const [state, setState] = useState<{
    data: T | null;
    loading: boolean;
    error: string | null;
  }>({ data: null, loading: true, error: null });
  const seq = useRef(0);

  useEffect(() => {
    let active = true;
    const run = () => {
      const ticket = ++seq.current;
      fetcher()
        .then((data) => {
          if (active && ticket === seq.current) {
            setState({ data, loading: false, error: null });
          }
        })
        .catch((err: unknown) => {
          if (active && ticket === seq.current) {
            setState({ data: null, loading: false, error: String(err) });
          }
        });
    };
    setState({ data: null, loading: true, error: null });
    run();
    const unsubscribe = onVaultChanged(run);
    return () => {
      active = false;
      unsubscribe();
    };
  }, [fetcher]);

  const setData = useCallback((data: T) => {
    seq.current += 1;
    setState({ data, loading: false, error: null });
  }, []);

  return { ...state, setData };
}
