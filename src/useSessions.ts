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
 * The empty list, once. `data ?? []` would mint a new array on every render
 * before the first read lands, and callers compare this array by identity:
 * NeedsAttentionView prunes its row errors when the list changes (React's
 * adjust-state-during-render), and useCommands has it in a memo's deps. A fresh
 * `[]` each render turns the first into an infinite render loop and the second
 * into a memo that never hits.
 */
const NO_SESSIONS: FailedSession[] = [];

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
  return { sessions: data ?? NO_SESSIONS, loading, error };
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

/** One transcript segment of a raw session. Mirrors `TranscriptSegmentDto` in
 * `src-tauri/src/session_cmds.rs` (and the MCP `TranscriptSegment` shape):
 * timestamps are millisecond offsets from session start, `speaker` is always
 * null in v1. */
export type TranscriptSegment = {
  index: number;
  channel: "you" | "them" | "unknown";
  speaker: string | null;
  start_ms: number;
  end_ms: number;
  text: string;
};

/** What a note's session source resolves to. Mirrors `SessionArtifactsDto` in
 * `src-tauri/src/session_cmds.rs`. `transcript_available: false` is the
 * retention-pruned state, not an error; `audio_path` is the absolute path of
 * the retained recording (fed to `convertFileSrc` for playback and handed
 * back to `revealSessionAudio`), or null when none exists. */
export type SessionArtifacts = {
  transcript_available: boolean;
  segments: TranscriptSegment[];
  audio_path: string | null;
};

/**
 * Whether a note's `source:` value names a raw session artifact — the
 * `sessions/<file>.jsonl` form distill writes — rather than a capture keyword
 * (`manual`, `quick-capture`, …). The guard the note view uses to decide the
 * source-pairing section exists at all, so keyword-sourced notes never invoke
 * the artifact read.
 */
export function isSessionSource(source: string): boolean {
  return source.startsWith("sessions/") && source.endsWith(".jsonl");
}

/**
 * The raw transcript and retained recording behind a distilled note's
 * `source:` field. Refetched via `useVaultQuery` on every vault change, which
 * the sessions-changed bridge also feeds — so a retention prune that deletes
 * the artifacts while the note is open flips the view live.
 */
export function useSessionArtifacts(source: string): {
  artifacts: SessionArtifacts | null;
  loading: boolean;
  error: string | null;
} {
  const { data, loading, error } = useVaultQuery(
    useCallback(
      () => invoke<SessionArtifacts>("read_session_artifacts", { source }),
      [source],
    ),
  );
  return { artifacts: data, loading, error };
}

/**
 * Opens Explorer with the retained recording selected. Mirrors
 * `session_cmds::reveal_session_audio`; Tauri exposes its `audio_path`
 * argument as `audioPath` on the wire. Rejects when the file has since been
 * discarded (retention racing the open note view).
 */
export function revealSessionAudio(audioPath: string): Promise<void> {
  return invoke("reveal_session_audio", { audioPath });
}
