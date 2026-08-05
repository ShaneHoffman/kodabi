import type { NoteSummary } from "./useNotes";

/** One run of consecutive notes under a single eyebrow label. `key` is the
 * month it was cut from (`"recent"` for the leading run), which is stable
 * across refetches in a way the label is not.
 *
 * It is also UNIQUE across the returned groups, because it is a React key: a
 * month can open more than one run (see `groupNotes`), and a second run of one
 * takes a `#n` suffix rather than repeating the first one's key. Two siblings
 * sharing a key is not a cosmetic warning — React reconciles them into one
 * slot, so a refetch can drop or swap a whole group's rows. */
export type NoteGroup = {
  key: string;
  label: string;
  notes: NoteSummary[];
};

/** Month names, spelled out rather than derived. `toLocaleString` would drag
 * in the machine's locale — a folder's index would then group under different
 * words on two machines looking at the same vault. */
const MONTH_NAMES = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];

/** The label the leading run carries. Not a month: what is worth knowing about
 * the newest notes is that they are the newest. */
const RECENT_LABEL = "Recent";

/**
 * A project's notes cut into the runs the browse index groups under.
 *
 * `notes` arrives newest-first from `scan_project_notes`, so this is one linear
 * pass that preserves encounter order: the current month leads as "Recent",
 * then each earlier month in the order it appears. Re-sorting here would be the
 * bug — the backend's ordering is the one that has a tie-breaker.
 *
 * Everything is derived by SLICING the frontmatter `date`, never by
 * constructing a `Date`. The field is either a local `YYYY-MM-DD` or an
 * offset-bearing RFC 3339 timestamp (docs/FRONTMATTER_SCHEMA.md), and parsing
 * the second form would re-zone it — a note written at 21:00 on the 31st
 * landing in the next month for a viewer further east. The first seven
 * characters are the month the note says it was written in, on both forms.
 *
 * `todayIso` is passed in rather than read here so the grouping stays pure and
 * testable; `todayIsoDate()` (useNotes.ts) is the local-day source callers use.
 * The comparison is `>=`, so a future-dated note joins "Recent" rather than
 * opening a month group above it.
 *
 * KNOWN SEAM, not fixed here: the backend orders by INSTANT and this groups by
 * the LOCAL month, and the two disagree at a month boundary. `date_sort_key`
 * (vault.rs) reads `2026-05-31T21:00:00-07:00` as 2026-06-01T04:00Z, newer than
 * a `2026-06-01` note at midnight UTC, so that May note leads and the index
 * heads "May 2026" above "June 2026" — and a month can therefore open more than
 * one run. Both halves are deliberate on their own (the sort wants a real
 * instant; the grouping must not re-zone a note out of the month it says it was
 * written in), so reconciling them is a decision, not a review fix. What is
 * handled here is the consequence that is not a judgement call: each run gets
 * its own `key`.
 */
export function groupNotes(notes: NoteSummary[], todayIso: string): NoteGroup[] {
  const currentMonth = todayIso.slice(0, 7);
  const groups: NoteGroup[] = [];
  // How many runs each label has already opened, so a month that opens a
  // second one gets a key of its own. A run is identified by its label rather
  // than by the key it will carry, precisely because the key is about to stop
  // being the same string twice.
  const runsSoFar = new Map<string, number>();

  for (const note of notes) {
    const month = note.date.slice(0, 7);
    const recent = month >= currentMonth;
    const label = recent ? RECENT_LABEL : monthLabel(month);
    const last: NoteGroup | undefined = groups[groups.length - 1];

    if (last?.label === label) {
      last.notes.push(note);
      continue;
    }

    const previousRuns = runsSoFar.get(label) ?? 0;
    runsSoFar.set(label, previousRuns + 1);
    const monthKey = recent ? "recent" : month;
    groups.push({
      key: previousRuns === 0 ? monthKey : `${monthKey}#${previousRuns}`,
      label,
      notes: [note],
    });
  }

  return groups;
}

/** `"2026-07"` → `"July 2026"`. A month the array cannot name (a malformed
 * date, which the vault scanner does not filter) falls back to the raw key
 * rather than rendering "undefined 2026". */
function monthLabel(month: string): string {
  const monthIndex = Number(month.slice(5, 7)) - 1;
  const name = MONTH_NAMES[monthIndex];
  return name ? `${name} ${month.slice(0, 4)}` : month;
}
