/**
 * Typed callers for the first-run model download. Wire types mirror the serde
 * DTOs in `src-tauri/src/models_cmds.rs`; the invoke strings equal the Rust
 * command names exactly (`.claude/rules/tauri-command-parity.md`).
 *
 * The backend reports each model set separately because they can genuinely
 * differ (a developer overrides one and downloads the other). The UI tells one
 * story, so `aggregateModelStatus` collapses the sets into the single state
 * every surface here renders.
 */
import { invoke } from "@tauri-apps/api/core";

/** Mirrors `SetState` in `crates/kodabi-core/src/models/status.rs`. */
export type ModelSetState = "ready" | "env_overridden" | "missing" | "partial";

/** Mirrors `SetStatus` in `crates/kodabi-core/src/models/status.rs`. */
export type ModelSetStatus = {
  id: string;
  state: ModelSetState;
  bytesTotal: number;
  bytesPresent: number;
  license: string;
};

/** Mirrors `ModelStatusDto` in `src-tauri/src/models_cmds.rs` (which flattens
 * `ModelsStatus` from kodabi-core into the two shell-only fields). */
export type ModelStatusDto = {
  ready: boolean;
  bytesRequired: number;
  bytesPresent: number;
  sets: ModelSetStatus[];
  downloading: boolean;
  modelsDir: string;
};

/** Mirrors `ModelsStateEvent` in `src-tauri/src/models_cmds.rs`. */
export type ModelsStateEvent =
  | {
      status: "downloading";
      file: string;
      file_index: number;
      file_count: number;
      file_received: number;
      file_total: number;
      overall_received: number;
      overall_total: number;
    }
  | { status: "verifying"; file: string }
  | {
      status: "retrying";
      file: string;
      attempt: number;
      max_attempts: number;
      message: string;
    }
  | { status: "ready" }
  | { status: "cancelled" }
  | { status: "error"; message: string };

/** What a download surface is showing right now. `unknown` is the pre-seed
 * state: distinct from `ready` so nothing flashes before the first status
 * lands. */
export type ModelDownloadState =
  | { status: "unknown" }
  | { status: "ready"; envOverridden: boolean }
  | { status: "missing"; bytesRequired: number }
  | { status: "downloading"; progress: DownloadProgress | null }
  | { status: "error"; message: string };

/** The numbers a progress display needs, normalised out of the event union. */
export type DownloadProgress = {
  file: string;
  fileIndex: number;
  fileCount: number;
  received: number;
  total: number;
  /** True while a finished file is being hashed, when byte progress is
   * legitimately frozen and a bar alone would read as a hang. */
  verifying: boolean;
};

export async function fetchModelStatus(): Promise<ModelStatusDto> {
  return invoke<ModelStatusDto>("model_status");
}

export async function startModelDownload(): Promise<void> {
  await invoke("download_models");
}

export async function cancelModelDownload(): Promise<void> {
  await invoke("cancel_model_download");
}

/**
 * Collapses the per-set status into the one state the UI renders.
 *
 * A download in flight outranks everything: it is what the user is watching.
 * Otherwise anything not yet usable means "missing", and the byte figure is
 * what remains to fetch, so a resumed download does not offer to re-download
 * what is already on disk.
 */
export function aggregateModelStatus(status: ModelStatusDto): ModelDownloadState {
  if (status.downloading) {
    return { status: "downloading", progress: null };
  }
  if (!status.ready) {
    return {
      status: "missing",
      bytesRequired: Math.max(0, status.bytesRequired - status.bytesPresent),
    };
  }
  return {
    status: "ready",
    envOverridden: status.sets.some((set) => set.state === "env_overridden"),
  };
}

/** Normalises a progress-bearing event into display numbers, or null for the
 * events that carry none. */
export function progressFromEvent(event: ModelsStateEvent): DownloadProgress | null {
  if (event.status === "downloading") {
    return {
      file: event.file,
      fileIndex: event.file_index,
      fileCount: event.file_count,
      received: event.overall_received,
      total: event.overall_total,
      verifying: false,
    };
  }
  return null;
}

/**
 * Bytes as a human figure, in decimal megabytes because that is what a download
 * is quoted in everywhere else the user will have seen one.
 */
export function formatMegabytes(bytes: number): string {
  const megabytes = Math.max(0, bytes) / 1e6;
  if (megabytes >= 1000) {
    return `${(megabytes / 1000).toFixed(1)} GB`;
  }
  return `${Math.round(megabytes)} MB`;
}
