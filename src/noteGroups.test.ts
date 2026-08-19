import { describe, expect, it } from "vitest";
import { groupNotes } from "./noteGroups";
import type { NoteSummary } from "./useNotes";

const TODAY = "2026-07-21";

/** A `NoteSummary` as `list_notes` returns one. Only `date` matters here. */
function note(date: string, id = `n_${date}`): NoteSummary {
  return {
    id,
    path: `Growth/${id}.md`,
    title: id,
    type: "note",
    project: "Growth",
    date,
    tags: [],
    source: "manual",
    confidence: null,
    category: null,
    category_confidence: null,
    tracking: null,
    snippet: "",
    // A project listing never carries a guess: the note already has a home.
    guess: null,
  };
}

/** What the index actually renders: the labels, in order, and how many rows sit
 * under each. */
function shape(groups: ReturnType<typeof groupNotes>) {
  return groups.map((group) => [group.label, group.notes.length] as const);
}

describe("groupNotes", () => {
  it("leads with the current month, under Recent rather than its own name", () => {
    const groups = groupNotes([note("2026-07-20"), note("2026-07-02")], TODAY);

    expect(shape(groups)).toEqual([["Recent", 2]]);
  });

  it("names each earlier month with its year", () => {
    // The year is not optional decoration: a vault two years old otherwise
    // grows two "March" groups that look like a bug.
    const groups = groupNotes([note("2026-07-20"), note("2026-03-04"), note("2025-03-30")], TODAY);

    expect(shape(groups)).toEqual([
      ["Recent", 1],
      ["March 2026", 1],
      ["March 2025", 1],
    ]);
  });

  it("keeps a future-dated note in Recent instead of opening a month above it", () => {
    // A note dated ahead (a hand-written frontmatter date, a device whose clock
    // ran fast) is still the newest thing in the folder. Grouping it under
    // "August 2026" would put a heading above Recent and imply an ordering
    // failure that isn't one.
    const groups = groupNotes([note("2026-08-14"), note("2026-07-20")], TODAY);

    expect(shape(groups)).toEqual([["Recent", 2]]);
  });

  it("reads the month off an offset-bearing timestamp without re-zoning it", () => {
    // The late-in-the-month case the slice exists for: 21:00 on the 31st in a
    // western offset is 04:00 on the 1st in UTC, so anything that parsed this
    // would file it under the following month.
    const groups = groupNotes([note("2026-07-20"), note("2026-05-31T21:00:00-07:00")], TODAY);

    expect(shape(groups)).toEqual([
      ["Recent", 1],
      ["May 2026", 1],
    ]);
  });

  it("preserves the backend's order rather than re-sorting", () => {
    // `scan_project_notes` already sorts newest-first with a filename
    // tie-breaker. Sorting again here would drop that tie-breaker on the floor.
    const groups = groupNotes(
      [note("2026-06-10", "n_first"), note("2026-06-10", "n_second")],
      TODAY,
    );

    expect(groups[0].notes.map((row) => row.id)).toEqual(["n_first", "n_second"]);
  });

  it("does not merge two runs of the same month that are not adjacent", () => {
    // A degenerate ordering the backend should never produce. One linear pass
    // renders it faithfully instead of silently reordering rows to tidy it up,
    // which would hide the real bug.
    const groups = groupNotes([note("2026-05-04"), note("2026-04-02"), note("2026-05-01")], TODAY);

    expect(shape(groups)).toEqual([
      ["May 2026", 1],
      ["April 2026", 1],
      ["May 2026", 1],
    ]);
  });

  it("gives a month's second run its own key, so the two cannot collide", () => {
    // The keys are React keys, and the ordering above is not hypothetical: the
    // backend sorts by instant, so a note written at 21:00 on May 31 in a
    // western offset (04:00Z on June 1) leads a June note dated at midnight
    // UTC, and May opens again below it. Two `<Fragment key="2026-05">`
    // siblings would reconcile into one slot and lose a group on refetch.
    const groups = groupNotes(
      [note("2026-05-31T21:00:00-07:00"), note("2026-06-01"), note("2026-05-30")],
      TODAY,
    );

    expect(groups.map((group) => group.key)).toEqual(["2026-05", "2026-06", "2026-05#1"]);
    expect(shape(groups)).toEqual([
      ["May 2026", 1],
      ["June 2026", 1],
      ["May 2026", 1],
    ]);
  });

  it("falls back to the raw month on a date it cannot name", () => {
    // The vault scanner does not reject a malformed frontmatter date, so this
    // renders "2026-00" rather than "undefined 2026". (A malformed month that
    // sorts ABOVE the current one — "2026-13" — is swallowed by the
    // future-dated rule above and reads as Recent, which is the same answer
    // that case gets for a well-formed date.)
    const groups = groupNotes([note("2026-00-01")], TODAY);

    expect(shape(groups)).toEqual([["2026-00", 1]]);
  });

  it("has no groups for no notes", () => {
    expect(groupNotes([], TODAY)).toEqual([]);
  });
});
