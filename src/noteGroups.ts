import type { NoteSummary } from "./useNotes";

/** One run of consecutive notes under a single eyebrow label. `key` is the
 * month it was cut from (`"recent"` for the leading run), which is stable
 * across refetches in a way the label is not. */
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
 */
export function groupNotes(notes: NoteSummary[], todayIso: string): NoteGroup[] {
  const currentMonth = todayIso.slice(0, 7);
  const groups: NoteGroup[] = [];

  for (const note of notes) {
    const month = note.date.slice(0, 7);
    const recent = month >= currentMonth;
    const key = recent ? "recent" : month;
    const last: NoteGroup | undefined = groups[groups.length - 1];

    if (last?.key === key) {
      last.notes.push(note);
    } else {
      groups.push({ key, label: recent ? RECENT_LABEL : monthLabel(month), notes: [note] });
    }
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
