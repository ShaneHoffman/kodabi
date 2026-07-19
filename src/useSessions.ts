import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useVaultQuery } from "./useVaultQuery";

/** One session needing attention. Mirrors `FailedSessionDto` in
 * `src-tauri/src/distill_cmds.rs`. */
export type FailedSession = {
  /** Absolute path to the raw `.jsonl`; the value `retryDistill` takes back. */
  path: string;
  file_name: string;
  /** Slug segment of the session filename, when it follows the scheme. */
  slug: string | null;
  /** Capture instant, RFC 3339 UTC (`Z`). */
  captured_at: string;
};

/**
 * Captured sessions that never became a note: a distill that failed, or one the
 * app died in the middle of. Derived from disk on every read (no failure record
 * is persisted), so the list survives a restart and self-heals when a session is
 * retried or pruned. Refetched via `useVaultQuery` on every vault change, which
 * the sessions-changed bridge also feeds.
 */
export function useFailedSessions(): {
  sessions: FailedSession[];
  loading: boolean;
  error: string | null;
} {
  const { data, loading, error } = useVaultQuery(
    useCallback(() => invoke<FailedSession[]>("list_failed_sessions"), []),
  );
  return { sessions: data ?? [], loading, error };
}

/**
 * Queues a failed session for another distill run. Resolves as soon as the run
 * is *queued*, not when it finishes: progress and the outcome arrive on the
 * `distill:state` event. A rejection here means the run never started, not that
 * a distill failed: either the path isn't a session file, or a run for it is
 * already going (the backend refuses to distill one session twice, since
 * nothing downstream dedupes the two notes that would produce).
 *
 * Mirrors `distill_cmds::distill_session`; Tauri exposes its `session_path`
 * argument as `sessionPath` on the wire.
 */
export function retryDistill(sessionPath: string): Promise<void> {
  return invoke("distill_session", { sessionPath });
}
