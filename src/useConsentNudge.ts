import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { CONSENT_REQUIRED_EVENT } from "./events";

/**
 * Opens the consent nudge when the backend blocks a capture for want of
 * acknowledged consent. The event is push-only (the backend emits it on a
 * gated capture attempt), so there's nothing to seed on mount — a fresh listen
 * is enough.
 */
export function useConsentNudge(): {
  open: boolean;
  closeNudge: () => void;
} {
  const [open, setOpen] = useState(false);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;

    listen(CONSENT_REQUIRED_EVENT, () => {
      if (active) setOpen(true);
    }).then((fn) => {
      if (active) {
        unlisten = fn;
      } else {
        fn();
      }
    });

    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  return { open, closeNudge: () => setOpen(false) };
}
