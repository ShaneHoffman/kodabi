import { describe, expect, it } from "vitest";

import {
  arrangeCommitments,
  arrangeTriage,
  formatDay,
  nextWeekIso,
  settledBy,
  tomorrowIso,
  triageWatermark,
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
    last_evidence_check: null,
    tier: "fresh",
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

  it("sorts needs-review, then overdue, then due soonest, then stale, aging, fresh", () => {
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
          tier: "aging",
          last_mention: "2026-08-01T00:00:00Z",
        }),
        commitment({
          entry_id: "le_stale",
          tier: "stale",
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
      "le_stale",
      "le_aging",
      "le_fresh",
    ]);
  });

  it("leaves a dated commitment above an undated stale one", () => {
    // The contested call, pinned: tiers subdivide the undated band rather than
    // reordering the dated one. A commitment with a date escalates on its own
    // when that date passes; an undated one has only its tier.
    const { groups } = arrangeCommitments(
      [
        commitment({
          entry_id: "le_stale",
          tier: "stale",
          last_mention: "2026-01-01T00:00:00Z",
        }),
        commitment({
          entry_id: "le_far",
          item: item({ due_date: "2027-12-31", status: "open" }),
        }),
      ],
      [],
    );

    expect(ids(groups[0].rows)).toEqual(["le_far", "le_stale"]);
  });

  it("orders one tier by the quietest first, counting an evidence check", () => {
    const { groups } = arrangeCommitments(
      [
        commitment({
          entry_id: "le_checked",
          tier: "stale",
          // The oldest mention of the three, but something looked for it
          // recently, so it is not the quietest.
          last_mention: "2026-06-01T00:00:00Z",
          last_evidence_check: "2026-08-05T00:00:00Z",
        }),
        commitment({
          entry_id: "le_quietest",
          tier: "stale",
          last_mention: "2026-07-01T00:00:00Z",
        }),
        commitment({
          entry_id: "le_middle",
          tier: "stale",
          last_mention: "2026-08-01T00:00:00Z",
        }),
      ],
      [],
    );

    expect(ids(groups[0].rows)).toEqual(["le_quietest", "le_middle", "le_checked"]);
  });

  it("files a lapsed snooze into its tier's band rather than back at the end", () => {
    const { groups, snoozed } = arrangeCommitments(
      [
        commitment({ entry_id: "le_fresh" }),
        commitment({
          entry_id: "le_woken",
          state: "snoozed",
          snooze_lapsed: true,
          tier: "stale",
          last_mention: "2026-06-01T00:00:00Z",
        }),
        commitment({
          entry_id: "le_shelved",
          state: "snoozed",
          snoozed_until: "2026-12-01",
          tier: "stale",
        }),
      ],
      [],
    );

    // A snooze that lapsed months later rejoins reading honestly: it kept
    // aging on the shelf, so it leads rather than looking freshly minted.
    expect(ids(groups[0].rows)).toEqual(["le_woken", "le_fresh"]);
    expect(ids(snoozed)).toEqual(["le_shelved"]);
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

describe("arrangeTriage", () => {
  const source = (noteId: string, title: string) => ({
    note_id: noteId,
    title,
    project: "Briarwood Golf",
    path: `${noteId}.md`,
    category: null,
  });

  it("lists only what enrolled after the marker", () => {
    const groups = arrangeTriage(
      [
        commitment({ entry_id: "le_old", created_at: "2026-08-01T00:00:00Z" }),
        commitment({ entry_id: "le_new", created_at: "2026-08-03T00:00:00Z" }),
      ],
      "2026-08-02T00:00:00Z",
    );

    expect(groups.flatMap((group) => group.rows.map((row) => row.entry_id))).toEqual([
      "le_new",
    ]);
  });

  it("groups by source note and orders both groups and rows oldest first", () => {
    const groups = arrangeTriage(
      [
        commitment({
          entry_id: "le_c",
          created_at: "2026-08-03T12:00:00Z",
          source: source("n_standup", "Standup"),
        }),
        commitment({
          entry_id: "le_a",
          created_at: "2026-08-03T09:00:00Z",
          source: source("n_kickoff", "Kickoff"),
        }),
        commitment({
          entry_id: "le_b",
          created_at: "2026-08-03T10:00:00Z",
          source: source("n_standup", "Standup"),
        }),
      ],
      "2026-08-02T00:00:00Z",
    );

    expect(groups.map((group) => group.label)).toEqual(["Kickoff", "Standup"]);
    expect(groups[1].rows.map((row) => row.entry_id)).toEqual(["le_b", "le_c"]);
  });

  it("collects rows whose source note is gone under one heading", () => {
    const groups = arrangeTriage(
      [
        commitment({ entry_id: "le_a", created_at: "2026-08-03T00:00:00Z" }),
        commitment({ entry_id: "le_b", created_at: "2026-08-04T00:00:00Z" }),
      ],
      "2026-08-02T00:00:00Z",
    );

    expect(groups).toHaveLength(1);
    expect(groups[0].label).toBe("Other");
  });

  // Declaring every commitment new would turn a convenience into a wall of
  // work, so an unknown marker shows nothing at all.
  it("shows nothing when the marker is unknown", () => {
    expect(
      arrangeTriage(
        [commitment({ entry_id: "le_a", created_at: "2026-08-03T00:00:00Z" })],
        null,
      ),
    ).toEqual([]);
  });
});

describe("triageWatermark", () => {
  const batch = [
    { entry_id: "le_a", created_at: "2026-08-03T09:00:00Z" },
    { entry_id: "le_b", created_at: "2026-08-03T10:00:00Z" },
    { entry_id: "le_c", created_at: "2026-08-03T11:00:00Z" },
  ];

  it("advances through the reviewed prefix", () => {
    expect(triageWatermark(batch, new Set(["le_a", "le_b"]))).toBe(
      "2026-08-03T10:00:00Z",
    );
  });

  // The bug this rule exists to prevent: taking the maximum would carry the
  // marker past le_a, which would then never be offered again.
  it("stops at the first unreviewed row rather than taking the maximum", () => {
    expect(triageWatermark(batch, new Set(["le_b", "le_c"]))).toBeNull();
  });

  it("is null until the oldest row is reviewed", () => {
    expect(triageWatermark(batch, new Set())).toBeNull();
  });

  it("reaches the newest row once every row is reviewed", () => {
    expect(
      triageWatermark(batch, new Set(["le_a", "le_b", "le_c"])),
    ).toBe("2026-08-03T11:00:00Z");
  });

  it("orders the batch itself rather than trusting the caller", () => {
    expect(
      triageWatermark([...batch].reverse(), new Set(["le_a"])),
    ).toBe("2026-08-03T09:00:00Z");
  });
});
