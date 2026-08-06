import { useMemo, type ReactNode } from "react";
import { ModelStatusContext } from "../../useModelStatus";
import { useModelDownload } from "../../useModelDownload";

/**
 * The one subscription to `models:state`, held above every consumer so the
 * Settings card, the first-run nudge and the capture indicators all read the
 * same state and none of them misses an event that landed before it mounted.
 */
export function ModelStatusProvider({ children }: { children: ReactNode }) {
  const { state, start, cancel } = useModelDownload();
  const value = useMemo(() => ({ state, start, cancel }), [state, start, cancel]);
  return <ModelStatusContext value={value}>{children}</ModelStatusContext>;
}
