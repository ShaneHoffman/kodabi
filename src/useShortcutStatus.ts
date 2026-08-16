import { useEffect, useState } from "react";
import { fetchShortcutStatus, type ShortcutStatus } from "./captureControl";

/**
 * Whether the OS-global shortcuts bound at startup: one `shortcut_status` read
 * on mount, and nothing after. The backend records the outcome once in setup
 * and never revises it, so unlike every other backend fact in the app there is
 * no event to keep live — this is the whole external system.
 *
 * `null` until the read lands, and `null` forever if it fails. Both render as
 * today's copy, which is the point: a read that did not answer says nothing
 * about whether the chord works, and putting "unavailable" on screen because we
 * could not ask would scare the user on the strength of no evidence. Same call
 * `useModelDownload` makes for its seed.
 */
export function useShortcutStatus(): ShortcutStatus | null {
  const [status, setStatus] = useState<ShortcutStatus | null>(null);

  useEffect(() => {
    let active = true;

    fetchShortcutStatus()
      .then((value) => {
        if (active) setStatus(value);
      })
      .catch(() => {});

    return () => {
      active = false;
    };
  }, []);

  return status;
}
