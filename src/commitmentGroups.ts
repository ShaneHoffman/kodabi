import type { Commitment, CommitmentDirection } from "./useCommitments";

/*
 * How the Commitments view arranges what the ledger hands it: the me/them
 * split, the order inside each group, and which rows belong on a shelf.
 *
 * Pure and separate from the view for the same reason `noteGroups.ts` is: the
 * arrangement IS the design here, so it is worth testing without a render.
 */

/** One direction's run of rows. */
export type CommitmentGroup = {
  direction: CommitmentDirection;
  /** The heading a person reads. */
  label: string;
  rows: Commitment[];
};

/** Everything the view renders, already arranged. */
export type ArrangedCommitments = {
  /** Mine, then Waiting on them, then Unassigned. Empty groups are dropped. */
  groups: CommitmentGroup[];
  /** Snoozed rows whose day has not arrived. */
  snoozed: Commitment[];
  /** Closed and waived rows, newest first, for the undo shelf. */
  settled: Commitment[];
};

/**
 * The group headings.
 *
 * "Waiting on them" rather than "Theirs": the heading names the position you
 * are in, not a possessive, because the whole point of the split is that the
 * two halves ask different things of you.
 */
const GROUP_LABELS: Record<CommitmentDirection, string> = {
  mine: "Mine",
  theirs: "Waiting on them",
  unassigned: "Unassigned",
};

/** Mine first: it is the half you can act on. */
const GROUP_ORDER: CommitmentDirection[] = ["mine", "theirs", "unassigned"];

/**
 * Parses a `YYYY-MM-DD` day into a comparable number.
 *
 * Hand-split rather than `new Date(string)`, which reads a date-only string as
 * UTC midnight and so lands on the previous day for every timezone west of
 * Greenwich. A due date is a local calendar day, and the comparison has to
 * agree with the one the backend made when it derived `overdue`.
 */
function dayValue(day: string | null | undefined): number | null {
  if (!day) return null;
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(day);
  if (!match) return null;
  return new Date(
    Number(match[1]),
    Number(match[2]) - 1,
    Number(match[3]),
  ).getTime();
}

/** The description a row shows: the note's current line when there is one, and
 * the ledger's cached copy when the line was edited away. */
export function commitmentText(commitment: Commitment): string {
  return commitment.item?.description ?? commitment.description;
}

/** The owner a row shows, same rule as the description. */
export function commitmentOwner(commitment: Commitment): string {
  return commitment.item?.owner ?? commitment.owner;
}

/** Whether a row is asking a question rather than waiting on work. */
export function needsReview(commitment: Commitment): boolean {
  return commitment.state === "needs_review";
}

/**
 * When anything last touched a commitment: the later of the mention that named
 * it and the evidence check that looked for it.
 *
 * Both are RFC 3339 UTC with a `Z`, the format whose lexical order is its
 * chronological order, so this is a string comparison on purpose. It matches
 * the anchor `AgingTier::derive` uses backend-side, which is what keeps the
 * within-band order agreeing with the tier the band was chosen by.
 */
export function agingAnchor(commitment: Commitment): string {
  const checked = commitment.last_evidence_check;
  return checked !== null && checked > commitment.last_mention ? checked : commitment.last_mention;
}

/**
 * Where a row sorts inside its group. Lower comes first.
 *
 * The order the ticket asks for, with one thing ahead of it: a needs-review row
 * is one keystroke from resolution and the ledger is asking directly, which
 * outranks any due date. Then overdue, then due soonest, then the undated ones
 * by tier.
 *
 * The tiers subdivide the undated band rather than reordering the dated one: a
 * commitment with a date carries its own escalation, since it becomes overdue
 * on its own and lands in band 1. An undated one has nothing but this to
 * resurface it, so going quiet is the only signal it has, and stale leads.
 */
function sortRank(commitment: Commitment): number {
  if (needsReview(commitment)) return 0;
  if (commitment.item?.status === "overdue") return 1;
  if (commitment.item?.due_date) return 2;
  if (commitment.tier === "stale") return 3;
  if (commitment.tier === "aging") return 4;
  return 5;
}

/** Orders one group's rows. */
function compare(a: Commitment, b: Commitment): number {
  const rank = sortRank(a) - sortRank(b);
  if (rank !== 0) return rank;

  // Within overdue and within dated: soonest first, which for overdue rows
  // means the longest overdue leads.
  const dueA = dayValue(a.item?.due_date);
  const dueB = dayValue(b.item?.due_date);
  if (dueA !== null && dueB !== null && dueA !== dueB) return dueA - dueB;

  // Undated: quietest first, so within a tier the ones nobody has touched for
  // longest lead.
  if (dueA === null && dueB === null) {
    const anchorA = agingAnchor(a);
    const anchorB = agingAnchor(b);
    if (anchorA !== anchorB) return anchorA < anchorB ? -1 : 1;
  }

  // Stable and deterministic, so a refetch never reshuffles equal rows.
  return commitmentText(a).localeCompare(commitmentText(b));
}

/**
 * Splits the ledger's answer into the runs the view draws.
 *
 * A snoozed row whose day has arrived rejoins the live groups rather than
 * staying on the shelf: nothing writes when a snooze lapses, so this is where
 * "until Friday" actually becomes visible again.
 */
export function arrangeCommitments(
  entries: Commitment[],
  settled: Commitment[],
): ArrangedCommitments {
  const live: Commitment[] = [];
  const snoozed: Commitment[] = [];
  for (const commitment of entries) {
    if (commitment.state === "snoozed" && !commitment.snooze_lapsed) {
      snoozed.push(commitment);
    } else {
      live.push(commitment);
    }
  }

  const groups = GROUP_ORDER.map((direction) => ({
    direction,
    label: GROUP_LABELS[direction],
    rows: live
      .filter((commitment) => commitment.direction === direction)
      .sort(compare),
  })).filter((group) => group.rows.length > 0);

  return { groups, snoozed, settled };
}

/**
 * The queue's workload sentence: what is on you, and what you are waiting on.
 *
 * A sentence rather than a count, which is what the `queue` frame variant asks
 * for. Empty when nothing is live, so the empty state speaks alone.
 */
export function workloadSummary(
  groups: CommitmentGroup[],
  scope: string | null,
): string | undefined {
  const mine = groups.find((group) => group.direction === "mine")?.rows ?? [];
  const others = groups
    .filter((group) => group.direction !== "mine")
    .flatMap((group) => group.rows);
  if (mine.length === 0 && others.length === 0) return undefined;

  const parts: string[] = [];
  if (mine.length > 0) {
    const overdue = mine.filter(
      (commitment) => commitment.item?.status === "overdue",
    ).length;
    const owed = `${mine.length} on you`;
    parts.push(overdue > 0 ? `${owed}, ${overdue} overdue` : owed);
  }
  if (others.length > 0) parts.push(`${others.length} on others`);
  if (scope) parts.push(scope);
  return parts.join(" · ");
}

/** Tomorrow, as a local `YYYY-MM-DD` day. */
export function tomorrowIso(today: Date = new Date()): string {
  return shiftIso(today, 1);
}

/** A week out, as a local `YYYY-MM-DD` day. */
export function nextWeekIso(today: Date = new Date()): string {
  return shiftIso(today, 7);
}

function shiftIso(from: Date, days: number): string {
  const day = new Date(from.getFullYear(), from.getMonth(), from.getDate());
  day.setDate(day.getDate() + days);
  const month = String(day.getMonth() + 1).padStart(2, "0");
  const date = String(day.getDate()).padStart(2, "0");
  return `${day.getFullYear()}-${month}-${date}`;
}

/** A `YYYY-MM-DD` day as "Aug 20", for a meta line. Falls back to the raw
 * string, which is already readable, rather than rendering nothing. */
export function formatDay(day: string): string {
  const value = dayValue(day);
  if (value === null) return day;
  return new Date(value).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}

/** An RFC 3339 instant as "Aug 12". Same fallback rule as `formatDay`. */
export function formatInstant(instant: string): string {
  const value = new Date(instant).getTime();
  if (Number.isNaN(value)) return instant;
  return new Date(value).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}

/** A claim's confidence as a percentage, matching `noteMeta`'s match score. */
export function formatConfidence(confidence: number): string {
  return `${Math.round(confidence * 100)}% confident`;
}

/** How a settled row says what settled it. Never silent: an entry closed by an
 * evidence pass has to say so, or the undo beside it is unexplained. */
export function settledBy(commitment: Commitment): string {
  if (commitment.state === "waived") return "waived";
  switch (commitment.closed_via) {
    case "github":
      return "closed from GitHub";
    case "conversation":
      return "closed from a conversation";
    case "manual":
      return "closed by you";
    default:
      return "closed";
  }
}
