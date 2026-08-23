import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { NoteCategory } from "./useNotes";
import { useVaultQuery } from "./useVaultQuery";

/*
 * The commitment wire shapes, mirroring the Rust DTOs in
 * `src-tauri/src/ledger_cmds.rs`.
 *
 * A commitment is two stores joined: the ledger owns identity and the states a
 * checkbox cannot spell, while `item` carries the note's current line and the
 * checkbox that owns done/not-done. `item` is null when the source line was
 * edited away, which is exactly when the ledger's cached `owner`/`description`
 * are what a row renders instead.
 */

/** Where an entry sits in its lifecycle. Mirrors `ledger::EntryState`; the two
 * states this view never asks for (`superseded`) are absent by construction. */
export type CommitmentState =
  | "open"
  | "needs_review"
  | "snoozed"
  | "closed"
  | "waived"
  | "untracked";

/** How an entry left the working set. Mirrors `ledger::UntrackedVia`: `manual`
 * is a person's own untrack, `override` one a meeting's own tracking mode
 * applied, and `category` one its meeting kind applied when it was
 * recategorized. */
export type UntrackedVia = "manual" | "override" | "category";

/** Which way a commitment points. Mirrors `ledger::Direction`, and it is this
 * view's organizing principle rather than a filter. */
export type CommitmentDirection = "mine" | "theirs" | "unassigned";

/** How a closure was established. Mirrors `ledger::ClosedVia`. */
export type ClosedVia = "manual" | "conversation" | "github";

/** Mirrors `ledger_cmds::CommitmentItemDto`: the live source line. */
export type CommitmentItem = {
  note_id: string;
  item_id: string;
  description: string;
  owner: string;
  due_date: string | null;
  done: boolean;
  status: "open" | "overdue" | "done";
};

/** Mirrors `ledger_cmds::CommitmentSourceDto`: where the line lives. */
export type CommitmentSource = {
  note_id: string;
  title: string;
  project: string | null;
  path: string;
  /** The source meeting's kind, in the kebab-case spelling `NoteCategory`
   * uses; `null` where the note carries none. */
  category: NoteCategory | null;
};

/** Mirrors `ledger_cmds::CommitmentEvidenceDto`: one claim about a commitment. */
export type CommitmentEvidence = {
  evidence_id: string;
  source: ClosedVia;
  reference: string | null;
  confidence: number;
  observed_at: string;
};

/**
 * How long a commitment has gone untouched, derived by the backend against the
 * device's local today and the user's thresholds. Mirrors
 * `kodabi_core::ledger::AgingTier`.
 */
export type CommitmentTier = "fresh" | "aging" | "stale";

/** Mirrors `ledger_cmds::CommitmentDto`. */
export type Commitment = {
  entry_id: string;
  state: CommitmentState;
  direction: CommitmentDirection;
  /** The ledger's cached owner; `item.owner` wins when a live line exists. */
  owner: string;
  /** The ledger's cached description, same rule as `owner`. */
  description: string;
  project: string | null;
  created_at: string;
  updated_at: string;
  last_mention: string;
  /** When an evidence provider last checked this commitment, if one ever has.
   * The other half of the aging anchor. */
  last_evidence_check: string | null;
  tier: CommitmentTier;
  snoozed_until: string | null;
  /** Whether a snooze's day has arrived. Evaluated by the backend at read time;
   * nothing writes when a snooze lapses, so a lapsed entry is still `snoozed`
   * on the wire and belongs back with the live work. */
  snooze_lapsed: boolean;
  closed_via: ClosedVia | null;
  review_reason: string | null;
  item: CommitmentItem | null;
  source: CommitmentSource | null;
  evidence: CommitmentEvidence[];
};

/** Mirrors `ledger_cmds::SettledSummaryDto`: what the settled window cleared.
 *
 * Counted over the whole seven days before the shelf was capped, so the
 * sentence stays true on a week that settled more than the shelf shows. */
export type SettledSummary = {
  /** Closed or waived. Untracked is excluded: leaving the working set is a
   * filing correction, not a win. */
  cleared: number;
  closed_from_conversation: number;
  closed_from_github: number;
};

/** Mirrors `ledger_cmds::CommitmentsDto`. */
export type CommitmentsPayload = {
  entries: Commitment[];
  settled: Commitment[];
  settled_summary: SettledSummary;
  /** When the user last reviewed newly enrolled commitments, RFC 3339 UTC.
   * The triage strip lists entries whose `created_at` sorts above it.
   *
   * `null` only when the ledger could not supply it, in which case the strip
   * stays hidden rather than declaring every commitment new. */
  last_seen: string | null;
};

/** Mirrors `ledger_cmds::BulkMutateDto`: what a batched gesture settled on.
 *
 * Counts, not entries: the `ledger:changed` the command broadcasts brings the
 * full truth, and the strip needs only enough to say how many were declined. */
export type BulkMutateResult = {
  updated: number;
  skipped: number;
};

/** Mirrors `ledger_cmds::CommitmentEntryDto`: a mutation's echo. */
export type CommitmentEntry = {
  entry_id: string;
  state: CommitmentState;
  snoozed_until: string | null;
  closed_via: ClosedVia | null;
  review_reason: string | null;
  updated_at: string;
  untracked_via: UntrackedVia | null;
};

/** Mirrors `ledger_cmds::SetCommitmentDoneDto`. */
export type SetCommitmentDoneResult = {
  entry: CommitmentEntry;
  note_updated: boolean;
};

/** Mirrors `ledger_cmds::ConfirmEvidenceDto`. */
export type ConfirmEvidenceResult = {
  entry: CommitmentEntry;
  note_updated: boolean;
  note_annotated: boolean;
};

/** Mirrors `ledger_cmds::WaiveCommitmentDto`. */
export type WaiveCommitmentResult = {
  entry: CommitmentEntry;
  note_annotated: boolean;
};

export function listCommitments(
  project: string | null,
): Promise<CommitmentsPayload> {
  return invoke<CommitmentsPayload>("list_commitments", { project });
}

/** How many commitments are on the user, outstanding, across the whole vault. */
export function countMyCommitments(): Promise<number> {
  return invoke<number>("count_my_commitments");
}

/** Ticks or unticks the source note's checkbox and records the judgement. */
export function setCommitmentDone(input: {
  entry_id: string;
  note_id: string;
  item_id: string;
  done: boolean;
}): Promise<SetCommitmentDoneResult> {
  return invoke<SetCommitmentDoneResult>("set_commitment_done", { input });
}

/** Hides a commitment until a local `YYYY-MM-DD` day. The note is untouched. */
export function snoozeCommitment(
  entryId: string,
  until: string,
): Promise<CommitmentEntry> {
  return invoke<CommitmentEntry>("snooze_commitment", {
    input: { entry_id: entryId, until },
  });
}

/** Marks a commitment as deliberately not happening, and writes a date-only
 * line under the item saying so. The box is never ticked: waived is not done,
 * and the line is the signal. */
export function waiveCommitment(
  entryId: string,
): Promise<WaiveCommitmentResult> {
  return invoke<WaiveCommitmentResult>("waive_commitment", {
    input: { entry_id: entryId },
  });
}

/** Removes a commitment from the working set: it never should have been in it.
 *
 * The sibling of `waiveCommitment`, and the difference is what each is about.
 * Waiving is about the commitment (it was mine, it stopped mattering);
 * untracking is about the ledger (this was never my business). Which is why
 * only waiving writes to the note: this one leaves it untouched. */
export function untrackCommitment(entryId: string): Promise<CommitmentEntry> {
  return invoke<CommitmentEntry>("untrack_commitment", {
    input: { entry_id: entryId },
  });
}

/** Untracks several commitments as one gesture, for the triage strip's group
 * and selection verbs.
 *
 * One invoke rather than N: the backend applies them in a single worker job and
 * broadcasts one `ledger:changed`, so the view refetches once instead of
 * flickering through a dozen refetches. Semantics are `untrackCommitment`'s,
 * repeated per entry.
 *
 * Best-effort: an entry the ledger refuses (one already settled) lands in
 * `skipped` rather than failing the whole gesture. */
export function untrackCommitments(
  entryIds: string[],
): Promise<BulkMutateResult> {
  return invoke<BulkMutateResult>("untrack_commitments", {
    input: { entry_ids: entryIds },
  });
}

/** Snoozes several commitments as one gesture. Same batching and same
 * best-effort rule as `untrackCommitments`; a needs-review entry is one the
 * ledger declines, because it is still asking a question. */
export function snoozeCommitments(
  entryIds: string[],
  until: string,
): Promise<BulkMutateResult> {
  return invoke<BulkMutateResult>("snooze_commitments", {
    input: { entry_ids: entryIds, until },
  });
}

/** Records that the user has reviewed newly enrolled commitments up to
 * `seenThrough` (RFC 3339 UTC, taken from a row's `created_at`).
 *
 * Emits no `ledger:changed` by design: nothing about a commitment changed, and
 * a refetch would recompute the review batch out from under the hand clearing
 * it. */
export function markCommitmentsSeen(seenThrough: string): Promise<void> {
  return invoke<void>("mark_commitments_seen", {
    input: { seen_through: seenThrough },
  });
}

/** Mirrors `AliasOutcome` in `src-tauri/src/ledger_cmds.rs`: what became of the
 * name a claimed row was filed under.
 *
 * `not_needed` is the design working (a reserved token like Them, or a spelling
 * already known); `failed` is the only one worth a word to the user, because
 * the same misfiling will happen again. */
export type AliasOutcome = "saved" | "not_needed" | "failed";

/** Mirrors `ClaimMineDto` in `src-tauri/src/ledger_cmds.rs`: the re-filed
 * entry, plus what became of the name it was filed under. */
export type ClaimMineResult = {
  entry: CommitmentEntry;
  alias: AliasOutcome;
};

/** One-click human correction: this commitment is mine.
 *
 * Moves the entry to Mine and teaches the owner spelling as one of the user's
 * names, so the next meeting files it right unprompted. The correction loop the
 * routing examples established: every correction is training data. */
export function claimCommitmentMine(entryId: string): Promise<ClaimMineResult> {
  return invoke<ClaimMineResult>("claim_commitment_mine", {
    input: { entry_id: entryId },
  });
}

/** Returns a commitment to open: waking a snooze, taking back a waiver, or
 * undoing a closure an evidence pass made on its own. */
export function reopenCommitment(entryId: string): Promise<CommitmentEntry> {
  return invoke<CommitmentEntry>("reopen_commitment", {
    input: { entry_id: entryId },
  });
}

/** Accepts a parked claim: closes with that claim's provenance, ticks the box,
 * and writes the story into the note. */
export function confirmCommitmentEvidence(
  entryId: string,
  evidenceId: string,
): Promise<ConfirmEvidenceResult> {
  return invoke<ConfirmEvidenceResult>("confirm_commitment_evidence", {
    input: { entry_id: entryId, evidence_id: evidenceId },
  });
}

/** Rejects a parked claim, reopening the entry if that claim closed it. */
export function dismissCommitmentEvidence(
  entryId: string,
  evidenceId: string,
): Promise<CommitmentEntry> {
  return invoke<CommitmentEntry>("dismiss_commitment_evidence", {
    input: { entry_id: entryId, evidence_id: evidenceId },
  });
}

/**
 * The Commitments view's read, for one project or the whole vault.
 *
 * Refetches on two signals. `vault:changed` arrives through the shell's bridge
 * and covers a ticked checkbox, which really does rewrite Markdown. A ledger
 * mutation writes no note, so it announces itself on its own channel, which
 * `useLedgerChangedBridge` relays onto the same bus at the shell root.
 *
 * That relay used to live here, as a local composition of two blessed hooks,
 * with a note saying a second concurrent consumer would move it to the shell.
 * The note view's enrollment panel is that second consumer, so it moved: two
 * copies of the relay would refetch twice for one mutation.
 */
export function useCommitments(slug: string | null) {
  const { data, loading, error } = useVaultQuery(
    useCallback(() => listCommitments(slug), [slug]),
    "Couldn't load the commitment ledger. Your notes on disk are untouched; reopen this view to try again.",
  );

  return {
    entries: data?.entries ?? [],
    settled: data?.settled ?? [],
    settledSummary: data?.settled_summary ?? null,
    lastSeen: data?.last_seen ?? null,
    /** The response object itself, for pruning per-row state during render:
     * key on this, never on a derived array, whose identity changes every
     * render. */
    response: data,
    loading,
    error,
  };
}

/**
 * The dock row's count: the user's own outstanding commitments.
 *
 * Mounted for the whole session, so it rides the same bus every other vault
 * read does. Both signals that can change the number are already relayed onto
 * it at the shell root (`vault:changed` for a ticked box, `ledger:changed` for
 * a judgement), so this needs no subscription of its own.
 *
 * Loading and failure both read as `null`: the row goes quiet rather than
 * apologising in a nav list, and the Commitments view itself is where a broken
 * ledger says so.
 */
export function useMyCommitmentsCount(): number | null {
  const { data } = useVaultQuery(useCallback(() => countMyCommitments(), []));
  return data;
}
