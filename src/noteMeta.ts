import type { NoteSummary } from "./useNotes";

/** The separator between meta segments, in one place so the three surfaces
 * that render a meta line can't drift apart on punctuation. */
const SEPARATOR = " · ";

/**
 * The quiet meta line a note row earns: the day first, then whatever the
 * surface adds, then the note's tags.
 *
 * Three near-identical builders existed (InboxView's added a match score,
 * NoteEditorView's added the note type, ProjectView's added nothing), each
 * with its own `.join(" · ")`. The shape is the same everywhere; only the
 * middle segment differs, so that is the only thing a caller passes.
 *
 * `date.slice(0, 10)` takes the calendar day off either frontmatter `date`
 * form — a local `YYYY-MM-DD` or an offset-bearing RFC 3339 timestamp
 * (docs/FRONTMATTER_SCHEMA.md). It is a display string, never a comparison.
 */
export function noteMeta(
  note: Pick<NoteSummary, "date" | "tags">,
  ...middle: (string | null | undefined)[]
): string {
  return [note.date.slice(0, 10), ...middle.filter((part) => !!part), ...note.tags].join(
    SEPARATOR,
  );
}

/**
 * The meta line a note row carries inside the folder that holds it: the kind
 * first, then the day.
 *
 * Two things differ from `noteMeta`, and both follow from the browse register.
 * The kind LEADS because the row stacks — title, meta, snippet — so the meta
 * line is read as a caption under a title rather than scanned down a date
 * column, and what the eye wants first is what sort of thing this is. And the
 * tags are DROPPED: inside a project, the shared filing is the thing you
 * already know, so repeating it on every row is noise the calm register cannot
 * afford. The day is sliced the same way `noteMeta` slices it.
 */
export function projectRowMeta(note: Pick<NoteSummary, "date" | "type">): string {
  return [noteKind(note.type), note.date.slice(0, 10)].filter((part) => !!part).join(SEPARATOR);
}

/** The router's confidence in where it filed a note, as a display string. A
 * hand-filed note carries no score, which reads as 0%. */
export function matchScore(confidence: number | null): string {
  return `${Math.round((confidence ?? 0) * 100)}% match`;
}

/**
 * A note's kind as a meta segment, or `null` when it says nothing.
 *
 * Every note is a `note` until proven otherwise, so that value is noise in a
 * dense list; a `meeting` or a `chat` is worth a word. `noteMeta` drops falsy
 * middles, so the `null` disappears without the caller branching.
 *
 * The single-note surface (`NoteEditorView`) deliberately passes `note.type`
 * straight through instead: there, one note fills the view and its kind is
 * information rather than clutter.
 */
export function noteKind(type: NoteSummary["type"]): string | null {
  return type === "note" ? null : type;
}
