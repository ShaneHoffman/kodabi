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

/** The router's confidence in where it filed a note, as a display string. A
 * hand-filed note carries no score, which reads as 0%. */
export function matchScore(confidence: number | null): string {
  return `${Math.round((confidence ?? 0) * 100)}% match`;
}
