import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";

import type {
  CommitmentDirection,
  CommitmentEntry,
  CommitmentState,
  UntrackedVia,
} from "./useCommitments";
import { useVaultQuery } from "./useVaultQuery";

/*
 * The note view's enrollment panel, mirroring the Rust DTOs in
 * `src-tauri/src/ledger_cmds.rs`.
 *
 * Extraction is not tracking: the note always records what was said, and this
 * is the separate question of whether a line became something the ledger
 * follows.
 */

/** Whether one extracted line is in the working set. Mirrors
 * `kodabi_core::ledger::view::ItemTracking`. `not_enrolled` means the meeting's
 * mode never enrolled it; `untracked` means it was enrolled and then removed. */
export type ItemTracking = "tracked" | "untracked" | "not_enrolled";

/** Mirrors `ledger_cmds::NoteCommitmentItemDto`. */
export type NoteCommitmentItem = {
  item_id: string;
  description: string;
  owner: string;
  due_date: string | null;
  done: boolean;
  direction: CommitmentDirection;
  tracking: ItemTracking;
  untracked_via: UntrackedVia | null;
  entry_id: string | null;
  entry_state: CommitmentState | null;
};

/** Mirrors `ledger_cmds::NoteCommitmentsDto`. */
export type NoteCommitmentsPayload = {
  context_only: boolean;
  items: NoteCommitmentItem[];
};

/** Mirrors `ledger_cmds::MeetingTrackingDto`. */
export type MeetingTrackingResult = {
  context_only: boolean;
  untracked: number;
  retracked: number;
};

export function listNoteCommitments(
  noteId: string,
): Promise<NoteCommitmentsPayload> {
  return invoke<NoteCommitmentsPayload>("list_note_commitments", {
    input: { note_id: noteId },
  });
}

/** Sets whether a meeting is tracked in full or for direct asks only, and
 * re-evaluates the entries it already produced. */
export function setMeetingTracking(
  noteId: string,
  contextOnly: boolean,
): Promise<MeetingTrackingResult> {
  return invoke<MeetingTrackingResult>("set_meeting_tracking", {
    input: { note_id: noteId, context_only: contextOnly },
  });
}

/** Tracks one extracted line by hand, whatever the meeting's mode says. */
export function trackCommitmentItem(
  noteId: string,
  itemId: string,
): Promise<CommitmentEntry> {
  return invoke<CommitmentEntry>("track_commitment_item", {
    input: { note_id: noteId, item_id: itemId },
  });
}

/**
 * One note's tracking mode and the enrollment status of each of its lines.
 *
 * Refetches on the shared vault bus, which `useLedgerChangedBridge` relays
 * `ledger:changed` onto, so flipping the mode here or untracking an entry in
 * the Commitments view both land without either surface knowing about the
 * other.
 */
export function useNoteCommitments(noteId: string) {
  const { data, loading, error } = useVaultQuery(
    useCallback(() => listNoteCommitments(noteId), [noteId]),
    "Couldn't read this meeting's tracking. Your note is untouched; reopen it to try again.",
  );

  return {
    contextOnly: data?.context_only ?? false,
    items: data?.items ?? [],
    /** The response object itself, for pruning per-row state during render. */
    response: data,
    loading,
    error,
  };
}
