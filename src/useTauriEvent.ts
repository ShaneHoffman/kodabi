import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";

/**
 * Subscribe to a backend Tauri event for the lifetime of the component. Handles
 * the two-part gotcha the raw `listen` API forces on every call site: the
 * subscription resolves asynchronously (so an unmount before it resolves must
 * still unlisten), and the handler must see fresh props/state without churning
 * the subscription. The listener is registered once per `event`; `handler` is
 * read through a ref, so passing a fresh inline closure each render is fine and
 * never re-subscribes.
 *
 * Extracted from the near-identical effects that `QuickCapture` and
 * `useVaultChangedBridge` both hand-rolled — one place to get the mount/unmount
 * race right.
 */
export function useTauriEvent<T>(
  event: string,
  handler: (payload: T) => void,
): void {
  const handlerRef = useRef(handler);
  handlerRef.current = handler;

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;

    listen<T>(event, (e) => {
      if (active) handlerRef.current(e.payload);
    }).then((fn) => {
      if (active) unlisten = fn;
      else fn();
    });

    return () => {
      active = false;
      unlisten?.();
    };
  }, [event]);
}
