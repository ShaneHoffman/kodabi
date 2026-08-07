import { createContext, useContext } from "react";
import type { UpdaterState } from "./useUpdater";

/**
 * The update state, subscribed once at the shell (`UpdaterProvider`) and read
 * from here by the two surfaces that care: the notice that appears when a
 * release is waiting, and the Settings About card.
 *
 * Shell-held for the same reason `useModelStatus` is: `useUpdater` runs its own
 * startup check per caller, so two independent callers would mean two checks
 * per launch and two disagreeing copies of a download's progress. Here there is
 * one check, one `Update` handle, one answer.
 *
 * Only the main window mounts it. Quick capture and the capture pill are
 * provider-less webviews, and neither has any business restarting the app —
 * which is also why `capabilities/updater.json` names `main` alone.
 */
export type UpdaterStatus = {
  state: UpdaterState;
  check: () => Promise<void>;
  download: () => Promise<void>;
  install: () => Promise<void>;
};

export const UpdaterStatusContext = createContext<UpdaterStatus | undefined>(undefined);

export function useUpdaterStatus(): UpdaterStatus {
  const ctx = useContext(UpdaterStatusContext);
  if (!ctx) {
    throw new Error("useUpdaterStatus must be used within <UpdaterProvider>");
  }
  return ctx;
}
