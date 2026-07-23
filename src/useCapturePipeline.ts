import { createContext, useContext } from "react";
import { isCaptureActive, type CaptureStateEvent } from "./useCaptureState";
import type { DistillState } from "./useDistillState";
import type { TranscriptionState } from "./useTranscriptionState";

/**
 * The three capture-adjacent hooks' raw states, gathered once at the shell
 * (`CapturePipelineProvider`) and shared from there. Each is a per-consumer
 * hook with its own listener — subscribing to all three a second time in a
 * routed view would miss any event that arrived before that view mounted, so
 * the shell holds the one subscription and everything else reads it here.
 */
export type CapturePipeline = {
  capture: CaptureStateEvent;
  transcription: TranscriptionState;
  distill: DistillState;
  /** The `filed` stage id the Inbox has fully presented — vanish-and-toast
   * played, or the routed row's fill-in settled. Held here rather than in the
   * Inbox because a distill's terminal `saved` state persists until the next
   * capture starts, while the Inbox's own state dies with every unmount:
   * without a shell-held record, re-entering the Inbox would replay the
   * vanish and the "Filed to <project>" toast for an outcome already seen. */
  handledFiledId: string | null;
  markFiledHandled: (id: string) => void;
  /** True from the moment a running capture stops until the next one starts
   * — or until the Inbox gives up on it (`markStopHandled`): the shell's own
   * record that a pipeline is due. Held because the backend announces
   * `Transcribing` only once its worker thread holds `TRANSCRIBE_LOCK`
   * (`transcribe.rs`), so for a beat after every stop all three states above
   * still read idle. Derived from the `capture:state`
   * listening|degraded → idle transition — the one signal every stop path
   * (tray, global shortcut, QuickCapture window) produces. */
  stopPending: boolean;
  /** Retires a stop the backend never acknowledged. Shell-held for the same
   * reason `handledFiledId` is: the Inbox's grace-timer waiver dies with the
   * view, while `stopPending` would otherwise persist until the next capture
   * — so without this, every later mount would replay the phantom
   * "Transcribing" placeholder for another grace period. A genuine run that
   * starts late (queued behind `TRANSCRIBE_LOCK`) still surfaces afterwards:
   * its own `transcribing` event carries the stage, not this flag. */
  markStopHandled: () => void;
};

export const CapturePipelineContext = createContext<CapturePipeline | undefined>(undefined);

export function useCapturePipeline(): CapturePipeline {
  const ctx = useContext(CapturePipelineContext);
  if (!ctx) {
    throw new Error("useCapturePipeline must be used within <CapturePipelineProvider>");
  }
  return ctx;
}

/**
 * A distill `saved` path is absolute with the OS's own separators
 * (`path.display().to_string()` in `distill_cmds.rs`); a `NoteSummary.path`
 * is KB-relative with forward slashes (`note_cmds.rs` strips the vault root
 * and normalizes). Equality never holds between them, so resolving a saved
 * outcome to a listed note is a suffix match on the normalized form.
 */
export function savedPathMatchesNote(savedPath: string, notePath: string): boolean {
  return savedPath.replace(/\\/g, "/").endsWith(`/${notePath}`);
}

export type SavedDestination = { kind: "inbox" } | { kind: "project"; slug: string };

/**
 * Where a distilled note landed, read back out of the absolute saved path.
 * Project directories mirror a project's slug segment-for-segment
 * (`note::project_dir`), so the longest slug whose segments end the path
 * wins — a nested `clients/acme` must beat its sibling-looking parent
 * `acme`. Falls back to the immediate parent directory name: a note routed
 * to a brand-new project can save before that project's next `list_projects`
 * refetch lands.
 */
export function savedDestination(savedPath: string, projectSlugs: string[]): SavedDestination {
  const normalized = savedPath.replace(/\\/g, "/");
  const segments = normalized.split("/");
  const fileName = segments[segments.length - 1];
  const parent = segments[segments.length - 2] as string | undefined;
  if (parent === "Inbox") return { kind: "inbox" };

  let best: string | null = null;
  for (const slug of projectSlugs) {
    if (normalized.endsWith(`/${slug}/${fileName}`)) {
      if (best === null || slug.length > best.length) best = slug;
    }
  }
  if (best !== null) return { kind: "project", slug: best };

  return { kind: "project", slug: parent ?? "" };
}

/** A stage's identity, so a component can key its timers on the run rather
 * than the delay: two `distilling` stages in a row (a retry) must not
 * inherit the first one's clock. `awaiting-distill` and `distilling` are
 * deliberately different ids, not just different `awaitingDistill` values —
 * a caller that gives up waiting on one (a grace timer covering a dev build
 * where distill never follows transcription) must not also give up on the
 * other if a lock-queued distill run genuinely starts afterward. `stopped`
 * and `transcribing` split the same way: giving up on a stop the backend
 * never acknowledged (a mis-tap under `MIN_TRANSCRIBE_DURATION` emits
 * nothing at all) must not also give up on a genuine run that starts late,
 * queued behind `TRANSCRIBE_LOCK`. */
export type PipelineStage =
  | {
      id: "stopped" | "transcribing";
      kind: "transcribing";
      awaitingTranscribe: boolean;
    }
  | {
      id: "awaiting-distill" | "distilling";
      kind: "distilling";
      awaitingDistill: boolean;
    }
  | { id: `filed:${string}`; kind: "filed"; savedPath: string };

/**
 * The Inbox placeholder's stage, derived from the three raw states. Distill
 * outranks transcription (same reasoning as `CaptureToast`'s `noticeFor`:
 * once distill has anything to say, transcription's result is already old
 * news). A `skipped` or `error` outcome collapses the placeholder — those
 * failures are the failures-only toast's and Needs Attention's job, not a
 * label here.
 */
export function pipelineStage(pipeline: CapturePipeline): PipelineStage | null {
  // The pipeline hooks reset to idle on their own next render once a new
  // capture starts, but this check answers instantly rather than showing a
  // stale terminal stage for the one render in between.
  if (isCaptureActive(pipeline.capture.phase)) return null;

  switch (pipeline.distill.status) {
    case "distilling":
      return { id: "distilling", kind: "distilling", awaitingDistill: false };
    case "saved":
      return {
        id: `filed:${pipeline.distill.session_path}`,
        kind: "filed",
        savedPath: pipeline.distill.path,
      };
    case "error":
    case "skipped":
      return null;
    case "idle":
      break;
  }

  switch (pipeline.transcription.status) {
    case "transcribing":
      return { id: "transcribing", kind: "transcribing", awaitingTranscribe: false };
    // The gap before distill picks up is sub-millisecond in a real build
    // (`transcribe.rs` chains `spawn_distill` right after this event), so
    // showing "distilling" already is more honest than a third, unseen label.
    case "saved":
      return { id: "awaiting-distill", kind: "distilling", awaitingDistill: true };
    case "error":
      return null;
    case "idle":
      // For a beat after every stop, all three states still read idle — the
      // backend's own `Transcribing` waits on `TRANSCRIBE_LOCK` — so the
      // stop the shell witnessed is the only evidence a pipeline is due.
      return pipeline.stopPending
        ? { id: "stopped", kind: "transcribing", awaitingTranscribe: true }
        : null;
  }
}
