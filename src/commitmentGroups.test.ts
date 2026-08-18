import { describe, expect, it } from "vitest";

import {
  arrangeCommitments,
  formatDay,
  nextWeekIso,
  settledBy,
  tomorrowIso,
  workloadSummary,
} from "./commitmentGroups";
import type { Commitment, CommitmentItem } from "./useCommitments";

/*
 * The arrangement IS the design on this surface — which half a row belongs to,
 * what floats to the top, what goes on a shelf — so it is tested here without a
 * render, where each rule can be stated on its own.
 */

function commitment(
  overrides: Partial<Commitment> & Pick<Commitment, "entry_id">,
): Commitment {
  return {
    state: "open",
    direction: "mine",
    owner: "You",
    description: "do the thing",
    project: "Briarwood Golf",
    created_at: "2026-08-01T00:00:00Z",
    updated_at: "2026-08-01T00:00:00Z",
    last_mention: "2026-08-01T00:00:00Z",
    snoozed_until: null,
    snooze_lapsed: false,
    closed_via: null,
    review_reason: null,
    item: null,
    source: null,
    evidence: [],
    ...overrides,
  };
}

function item(overrides: Partial<CommitmentItem> = {}): CommitmentItem {
  return {
    note_id: "n_a1b2c3",
    item_id: "a_111111",
    description: "do the thing",
    owner: "You",
    due_date: null,
    done: false,
    status: "open",
    ...overrides,
  };
}

const ids = (rows: Commitment[]) => rows.map((row) => row.entry_id);

describe("arrangeCommitments", () => {
  it("splits mine from waiting-on-them, mine first", () => {
    const { groups } = arrangeCommitments(
      [
        commitment({ entry_id: "le_theirs", direction: "theirs", owner: "Priya" }),
        commitment({ entry_id: "le_none", direction: "unassigned" }),
        commitment({ entry_id: "le_mine", direction: "mine" }),
      ],
      [],
    );

    expect(groups.map((group) => group.direction)).toEqual([
      "mine",
      "theirs",
      "unassigned",
    ]);
    expect(groups[1].label).toBe("Waiting on them");
  });

  it("drops a group with no rows rather than printing an empty heading", () => {
    const { groups } = arrangeCommitments(
      [commitment({ entry_id: "le_mine" })],
      [],
    );
    expect(groups).toHaveLength(1);
  });

  it("sorts needs-review, then overdue, then due soonest, then aging", () => {
    const { groups } = arrangeCommitments(
      [
        commitment({
          entry_id: "le_fresh",
          last_mention: "2026-08-16T00:00:00Z",
        }),
        commitment({
          entry_id: "le_soon",
          item: item({ due_date: "2026-08-25", status: "open" }),
        }),
        commitment({
          entry_id: "le_aging",
          last_mention: "2026-07-01T00:00:00Z",
        }),
        commitment({
          entry_id: "le_overdue",
          item: item({ due_date: "2026-08-10", status: "overdue" }),
        }),
        commitment({ entry_id: "le_review", state: "needs_review" }),
      ],
      [],
    );

    expect(ids(groups[0].rows)).toEqual([
      "le_review",
      "le_overdue",
      "le_soon",
      "le_aging",
      "le_fresh",
    ]);
  });

  it("puts the longest overdue first and the soonest due first", () => {
    const { groups } = arrangeCommitments(
      [
        commitment({
          entry_id: "le_late",
          item: item({ due_date: "2026-08-14", status: "overdue" }),
        }),
        commitment({
          entry_id: "le_latest",
          item: item({ due_date: "2026-08-02", status: "overdue" }),
        }),
      ],
      [],
    );
    expect(ids(groups[0].rows)).toEqual(["le_latest", "le_late"]);
  });

  it("shelves a snoozed row but lets a lapsed one rejoin the live work", () => {
    // Nothing writes on the day a snooze lapses, so this split is the only
    // thing that makes "until Friday" visible again on Friday.
    const { groups, snoozed } = arrangeCommitments(
      [
        commitment({
          entry_id: "le_sleeping",
          state: "snoozed",
          snoozed_until: "2026-12-01",
          snooze_lapsed: false,
        }),
        commitment({
          entry_id: "le_woke",
          state: "snoozed",
          snoozed_until: "2026-08-17",
          snooze_lapsed: true,
        }),
      ],
      [],
    );

    expect(ids(snoozed)).toEqual(["le_sleeping"]);
    expect(ids(groups[0].rows)).toEqual(["le_woke"]);
  });

  it("orders equal rows by their text, so a refetch never reshuffles them", () => {
    const { groups } = arrangeCommitments(
      [
        commitment({ entry_id: "le_b", description: "book the venue" }),
        commitment({ entry_id: "le_a", description: "answer Dana" }),
      ],
      [],
    );
    expect(ids(groups[0].rows)).toEqual(["le_a", "le_b"]);
  });

  it("prefers the note's current text over the ledger's cached copy", () => {
    const { groups } = arrangeCommitments(
      [
        commitment({
          entry_id: "le_z",
          description: "zzz stale",
          item: item({ description: "aaa current" }),
        }),
        commitment({ entry_id: "le_b", description: "bbb" }),
      ],
      [],
    );
    // Sorted by the live line, not the stale cache.
    expect(ids(groups[0].rows)).toEqual(["le_z", "le_b"]);
  });
});

describe("workloadSummary", () => {
  it("says what is on you and what is on others", () => {
    const { groups } = arrangeCommitments(
      [
        commitment({ entry_id: "le_1" }),
        commitment({
          entry_id: "le_2",
          item: item({ due_date: "2026-08-01", status: "overdue" }),
        }),
        commitment({ entry_id: "le_3", direction: "theirs", owner: "Priya" }),
      ],
      [],
    );
    expect(workloadSummary(groups, null)).toBe("2 on you, 1 overdue · 1 on others");
  });

  it("drops the clauses that would read as zero, and names a scope", () => {
    const { groups } = arrangeCommitments(
      [commitment({ entry_id: "le_1", direction: "theirs", owner: "Priya" })],
      [],
    );
    expect(workloadSummary(groups, "Briarwood Golf")).toBe(
      "1 on others · Briarwood Golf",
    );
  });

  it("is absent when nothing is live, so the empty state speaks alone", () => {
    expect(workloadSummary([], null)).toBeUndefined();
  });
});

describe("settledBy", () => {
  it("never leaves a settled row unexplained", () => {
    expect(
      settledBy(commitment({ entry_id: "le_1", state: "waived" })),
    ).toBe("waived");
    expect(
      settledBy(
        commitment({ entry_id: "le_2", state: "closed", closed_via: "manual" }),
      ),
    ).toBe("closed by you");
    expect(
      settledBy(
        commitment({ entry_id: "le_3", state: "closed", closed_via: "github" }),
      ),
    ).toBe("closed from GitHub");
    expect(
      settledBy(
        commitment({
          entry_id: "le_4",
          state: "closed",
          closed_via: "conversation",
        }),
      ),
    ).toBe("closed from a conversation");
  });
});

describe("dates", () => {
  it("reads a day in local time, never shifted a day west by UTC parsing", () => {
    // `new Date("2026-08-20")` is UTC midnight, which is the 19th anywhere west
    // of Greenwich. A due date is a local calendar day and has to render as one.
    const rendered = formatDay("2026-08-20");
    const local = new Date(2026, 7, 20).toLocaleDateString(undefined, {
      month: "short",
      day: "numeric",
    });
    expect(rendered).toBe(local);
  });

  it("falls back to the raw string rather than rendering nothing", () => {
    expect(formatDay("next Tuesday")).toBe("next Tuesday");
  });

  it("computes the snooze presets as local calendar days", () => {
    const today = new Date(2026, 7, 17);
    expect(tomorrowIso(today)).toBe("2026-08-18");
    expect(nextWeekIso(today)).toBe("2026-08-24");
  });

  it("carries a preset across a month boundary", () => {
    expect(tomorrowIso(new Date(2026, 7, 31))).toBe("2026-09-01");
  });
});
