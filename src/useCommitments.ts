import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";

import { LEDGER_CHANGED_EVENT } from "./events";
import { useTauriEvent } from "./useTauriEvent";
import { notifyVaultChanged, useVaultQuery } from "./useVaultQuery";

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
  | "waived";

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

/** Mirrors `ledger_cmds::CommitmentsDto`. */
export type CommitmentsPayload = {
  entries: Commitment[];
  settled: Commitment[];
};

/** Mirrors `ledger_cmds::CommitmentEntryDto`: a mutation's echo. */
export type CommitmentEntry = {
  entry_id: string;
  state: CommitmentState;
  snoozed_until: string | null;
  closed_via: ClosedVia | null;
  review_reason: string | null;
  updated_at: string;
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

export function listCommitments(
  project: string | null,
): Promise<CommitmentsPayload> {
  return invoke<CommitmentsPayload>("list_commitments", { project });
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

/** Marks a commitment as deliberately not happening. The note is untouched,
 * which is the whole point of the verb. */
export function waiveCommitment(entryId: string): Promise<CommitmentEntry> {
  return invoke<CommitmentEntry>("waive_commitment", {
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
 * mutation writes no note, so it announces itself on its own channel; relaying
 * that onto the same bus reuses `useVaultQuery`'s response sequencing rather
 * than opening a second, unsequenced path to the same state. Composing two
 * blessed hooks is deliberate: this is not a new external system, so it earns no
 * bridge hook of its own (`.claude/rules/no-use-effect.md`). A second concurrent
 * consumer would change that answer, and then the relay moves to the shell.
 */
export function useCommitments(slug: string | null) {
  const { data, loading, error } = useVaultQuery(
    useCallback(() => listCommitments(slug), [slug]),
    "Couldn't load the commitment ledger. Your notes on disk are untouched; reopen this view to try again.",
  );
  useTauriEvent(LEDGER_CHANGED_EVENT, () => notifyVaultChanged());

  return {
    entries: data?.entries ?? [],
    settled: data?.settled ?? [],
    /** The response object itself, for pruning per-row state during render:
     * key on this, never on a derived array, whose identity changes every
     * render. */
    response: data,
    loading,
    error,
  };
}
