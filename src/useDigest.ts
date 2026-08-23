import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";

import { useVaultQuery } from "./useVaultQuery";

/*
 * The daily digest's wire shapes, mirroring `kodabi_core::ledger::digest` as
 * `src-tauri/src/ledger_cmds.rs` returns it.
 *
 * The digest is what *changed* in the ledger since it last ran, not what the
 * ledger currently holds: an item is here because it crossed a boundary, and
 * it appears once. The Commitments view is where the standing picture lives.
 */

/** Why a commitment is in today's digest. Mirrors `ledger::digest::DigestKind`. */
export type DigestKind =
  | "newly_overdue"
  | "parked_in_review"
  | "went_stale"
  | "theirs_quiet";

/** One transition. Mirrors `ledger::digest::DigestItem`. */
export type DigestItem = {
  entry_id: string;
  kind: DigestKind;
  /** The live source line's text, else the ledger's cached description. */
  description: string;
  owner: string;
  project: string;
  note_id: string | null;
  note_title: string | null;
  /** Present on `newly_overdue`. */
  due_date: string | null;
  /** Present on `went_stale` and `theirs_quiet`. */
  last_mention: string | null;
  /** Present on `theirs_quiet`: whole days since that mention. */
  quiet_days: number | null;
  /** Present on `parked_in_review`. */
  review_reason: string | null;
};

/** A day's digest. Mirrors `ledger::digest::Digest`. */
export type DigestPayload = {
  /** The local day this digest describes (`YYYY-MM-DD`). */
  date: string;
  /** The local day it measures from: the previous digest's day. */
  since: string;
  items: DigestItem[];
  /** Transitions that qualified but did not fit the cap. */
  more: number;
};

/**
 * Today's digest, computing one first if the day is due.
 *
 * The command is compute-if-due, so asking is the whole trigger: there is no
 * scheduler, and the marker in the ledger is what makes it happen once a day
 * rather than once a mount. Calling it again is cheap and returns the same
 * list.
 */
export function dailyDigest(): Promise<DigestPayload> {
  return invoke<DigestPayload>("daily_digest");
}

/**
 * The digest card's read.
 *
 * `useVaultQuery` rather than a bridge hook of its own, and that is the whole
 * mechanism: it fetches on mount and refetches on the vault bus, which
 * `useLedgerChangedBridge` also feeds at the shell root. So a process that sat
 * in the tray across midnight picks up its new digest at the next thing that
 * touches the vault, without a timer watching the clock.
 *
 * Failure reads as no digest at all. The card is contextual chrome on the
 * landing view; a day with nothing to report and a day the ledger could not
 * answer both correctly show nothing, and the Commitments view is where a
 * broken ledger says so.
 */
export function useDailyDigest(): DigestPayload | null {
  const { data } = useVaultQuery(useCallback(() => dailyDigest(), []));
  return data;
}
