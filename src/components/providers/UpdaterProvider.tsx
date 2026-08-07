import { useMemo, type ReactNode } from "react";
import { UpdaterStatusContext } from "../../useUpdaterStatus";
import { useUpdater } from "../../useUpdater";

/**
 * The one update check per launch, held above both consumers so the notice and
 * the Settings card read the same phase and a download started from either is
 * the same download.
 *
 * The startup check is off in dev. A `pnpm tauri dev` build carries whatever
 * version is in tauri.conf.json and would happily discover that the published
 * release is newer than the tree being worked on, so the nag would be wrong
 * and constant. The Settings button still works there, which is what a
 * developer wanting to exercise the flow actually needs.
 */
export function UpdaterProvider({ children }: { children: ReactNode }) {
  const { state, check, download, install } = useUpdater({
    checkOnMount: !import.meta.env.DEV,
  });
  const value = useMemo(
    () => ({ state, check, download, install }),
    [state, check, download, install],
  );
  return <UpdaterStatusContext value={value}>{children}</UpdaterStatusContext>;
}
