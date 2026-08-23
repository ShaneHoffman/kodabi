import type {
  Commitment,
  CommitmentDirection,
  CommitmentEvidence,
  SettledSummary,
} from "./useCommitments";

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

/** One meeting's worth of newly enrolled commitments, for the triage strip. */
export type TriageGroup = {
  /** The source note's id, or `""` for the run of rows whose source line is
   * gone. Unique across groups, so it is safe as a React key. */
  noteId: string;
  /** The heading a person reads: the source note's title, plus the day the
   * meeting happened when one is known. The date is not decoration — grouping
   * is by note, so a recurring meeting would otherwise render two groups with
   * the same heading and no way to tell Monday's standup from Tuesday's. */
  label: string;
  rows: Commitment[];
};

/** The label for rows the ledger holds but whose source note it cannot name. */
const UNSOURCED_LABEL = "Other";

/**
 * One triage group's heading: the source note's title and the day it happened.
 *
 * `last_mention` is the note's own date rather than a clock reading, so it is
 * the meeting's day even for a commitment enrolled by a backfill days later.
 * An unsourced row gets no date, because there is no meeting to date.
 */
function triageLabel(commitment: Commitment): string {
  const title = commitment.source?.title;
  if (!title) return UNSOURCED_LABEL;
  const day = formatInstant(commitment.last_mention);
  return day === commitment.last_mention ? title : `${title} ${day}`;
}

/**
 * Groups the commitments enrolled since `lastSeen` by the meeting that
 * produced them.
 *
 * `created_at` is the only stable "when did this enter my ledger" instant:
 * `updated_at` moves on every judgement and `last_mention` moves whenever a
 * later meeting restates the commitment, so either would drag old rows back
 * into a review list that is supposed to be about what is new.
 *
 * Grouped by source note because a meeting is the unit a person actually
 * reviews: six commitments from one standup are one decision, not six. Groups
 * come oldest-first so a review works forwards through the day, which is also
 * the order the watermark below can advance through.
 *
 * A `null` marker yields nothing at all. That is the honest reading of "we do
 * not know when you last looked": declaring every commitment new would turn a
 * convenience into a wall of work nobody asked for.
 */
export function arrangeTriage(
  entries: Commitment[],
  lastSeen: string | null,
): TriageGroup[] {
  if (!lastSeen) return [];

  const groups = new Map<string, TriageGroup>();
  for (const commitment of entries) {
    // RFC 3339 UTC on both sides, so a lexical compare is a chronological one.
    if (commitment.created_at <= lastSeen) continue;
    const noteId = commitment.source?.note_id ?? "";
    const group = groups.get(noteId);
    if (group) {
      group.rows.push(commitment);
    } else {
      groups.set(noteId, {
        noteId,
        label: triageLabel(commitment),
        rows: [commitment],
      });
    }
  }

  for (const group of groups.values()) {
    group.rows.sort((left, right) =>
      left.created_at.localeCompare(right.created_at),
    );
  }
  return [...groups.values()].sort((left, right) =>
    left.rows[0].created_at.localeCompare(right.rows[0].created_at),
  );
}

/**
 * How far the last-seen marker may advance, given which rows have been
 * reviewed.
 *
 * The largest `created_at` whose entire earlier-or-equal prefix is reviewed, or
 * `null` when the very oldest row is still outstanding.
 *
 * **Contiguous prefix, not the maximum.** The marker is a single instant, so
 * advancing it past an unreviewed row hides that row forever: it would fall
 * below the cut on the next mount and never be offered again. Taking the
 * maximum of whatever was clicked is exactly that bug, and it bites hardest in
 * the case the strip is for, where somebody keeps one commitment out of a busy
 * day and the other twenty vanish. Keeping a row out of order is therefore
 * cheap and safe: it simply reappears next time, until the rows before it are
 * dealt with too.
 *
 * `active` is the set of entries the ledger still lists as live. A batch row
 * missing from it was settled while the strip was open, by the queue below or
 * by a later meeting, which is a stronger judgement than "reviewed" and cannot
 * be recorded in the strip because the row is no longer drawn there. Such a row
 * therefore counts as dealt with: leaving it to block the prefix would mean a
 * person who cleared everything still on screen recorded nothing at all, and
 * met the same list again on the next mount.
 */
export function triageWatermark(
  batch: readonly { entry_id: string; created_at: string }[],
  reviewed: ReadonlySet<string>,
  active: ReadonlySet<string>,
): string | null {
  const ordered = [...batch].sort((left, right) =>
    left.created_at.localeCompare(right.created_at),
  );
  let watermark: string | null = null;
  for (const row of ordered) {
    if (!reviewed.has(row.entry_id) && active.has(row.entry_id)) break;
    watermark = row.created_at;
  }
  return watermark;
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

/** The claim that closed a commitment, or `null` where nothing did.
 *
 * The newest claim whose source matches `closed_via`: evidence arrives oldest
 * first, and a commitment can carry several claims before one of them clears
 * the auto-close threshold, so the last matching claim is the one that spoke.
 */
export function closingClaim(commitment: Commitment): CommitmentEvidence | null {
  if (commitment.state !== "closed" || commitment.closed_via === null) {
    return null;
  }
  for (let index = commitment.evidence.length - 1; index >= 0; index -= 1) {
    const claim = commitment.evidence[index];
    if (claim.source === commitment.closed_via) return claim;
  }
  return null;
}

/** How a settled row says what settled it. Never silent: an entry closed by an
 * evidence pass has to say so, or the undo beside it is unexplained.
 *
 * An auto-close says so in the active voice and dates itself from the claim's
 * `observed_at`, which is the source note's own day. "Closed itself from the
 * Aug 19 conversation" is the app showing its work; "closed from a
 * conversation" was the app being modest about the best thing it does. */
export function settledBy(commitment: Commitment): string {
  if (commitment.state === "waived") return "waived";
  // Untracked is not a closure and must not read as one: it says this was never
  // your business, where waived says it was and stopped mattering.
  if (commitment.state === "untracked") return "untracked";
  const claim = closingClaim(commitment);
  switch (commitment.closed_via) {
    case "github":
      // A claim with no stored day degrades to the undated sentence rather
      // than rendering an unreadable one.
      return claim
        ? `closed itself from GitHub on ${formatInstant(claim.observed_at)}`
        : "closed itself from GitHub";
    case "conversation":
      return claim
        ? `closed itself from the ${formatInstant(claim.observed_at)} conversation`
        : "closed itself from a conversation";
    case "manual":
      return "closed by you";
    default:
      return "closed";
  }
}

/** How sure the pass was that closed this, or `null` where a person closed it.
 *
 * Held apart from `settledBy` because it is a different register: the sentence
 * says what happened, the percentage is the evidence behind it. */
export function autoCloseConfidence(commitment: Commitment): string | null {
  if (commitment.closed_via === "manual") return null;
  const claim = closingClaim(commitment);
  return claim ? formatConfidence(claim.confidence) : null;
}

/** What the settled shelf leads with: the week's wins, and how many of them the
 * app noticed on its own.
 *
 * `undefined` at zero, matching `ViewFrame`'s summary rule: a shelf with
 * nothing cleared says nothing rather than reporting a nought. */
export function settledSummaryLine(
  summary: SettledSummary | null,
): string | undefined {
  if (summary === null || summary.cleared === 0) return undefined;
  const parts = [`${summary.cleared} cleared this week`];
  const conversation = summary.closed_from_conversation;
  const github = summary.closed_from_github;
  if (conversation > 0) {
    parts.push(
      `${conversation} closed ${conversation === 1 ? "itself" : "themselves"} from conversation`,
    );
  }
  if (github > 0) {
    // The verb is already spent when conversation said it, so GitHub joins the
    // same sentence rather than repeating it.
    parts.push(
      conversation > 0
        ? `${github} from GitHub`
        : `${github} closed ${github === 1 ? "itself" : "themselves"} from GitHub`,
    );
  }
  return parts.join(", ");
}
