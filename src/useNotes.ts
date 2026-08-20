import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useVaultQuery } from "./useVaultQuery";

/*
 * The note wire shapes, mirroring the Rust DTOs in `src-tauri/src/note_cmds.rs`
 * (which in turn mirror the MCP `NoteSummary` — docs/MCP_TOOL_SURFACE.md):
 * `project: null` stands in for the frontmatter's Inbox sentinel, and
 * `confidence: null` for an omitted key (a hand-filed note carries no routing
 * score).
 */

export type NoteType = "meeting" | "note" | "chat";

/** The Inbox sentinel folder name (mirrors `note::INBOX` in kodabi-core). A
 * note's `project` is `null` in the `NoteSummary` wire shape, but the folder —
 * and the `list_notes`/`read_note` project argument for an unfiled note — is
 * this literal string. */
export const INBOX_PROJECT = "Inbox";

/** Mirrors `RouteGuessDto` in `src-tauri/src/note_cmds.rs`: where the router
 * would file an unfiled note today, scored against the vault's current
 * signals. `confidence` is the strength of that suggestion, which is a
 * different number from the sibling `NoteSummary.confidence` — that one
 * records why the note landed in the Inbox in the first place. */
export type RouteGuess = {
  project: string;
  confidence: number;
};

/** Mirrors `MeetingCategory` in `crates/kodabi-core/src/note.rs` (and the
 * `MeetingCategory` `$def` in `docs/MCP_TOOL_SURFACE.md`): the meeting's genre,
 * the second facet beside `type`. A closed set, so a corrected value means the
 * same thing next week. These kebab-case strings are the frontmatter values;
 * their labels live in `CATEGORY_OPTIONS` below. */
export type NoteCategory =
  | "standup"
  | "one-on-one"
  | "client"
  | "working-session"
  | "review"
  | "all-hands"
  | "observer";

/** The seven meeting kinds, in the order every surface offers them.
 *
 * One table, shared by the note view's picker, the Inbox row menu and the
 * Settings card, so the three can never drift — the same reason
 * `useSettings.ts` keeps `THEME_OPTIONS` and `RETENTION_OPTIONS`. The labels
 * are user-facing copy: change them here and nowhere else. */
export const CATEGORY_OPTIONS: { value: NoteCategory; label: string }[] = [
  { value: "standup", label: "Stand-up" },
  { value: "one-on-one", label: "One-on-one" },
  { value: "client", label: "Client" },
  { value: "working-session", label: "Working session" },
  { value: "review", label: "Review" },
  { value: "all-hands", label: "All hands" },
  { value: "observer", label: "Observer" },
];

/** The display label for a category, falling back to the raw wire value so a
 * note written by a newer build never renders as blank. */
export function categoryLabel(category: NoteCategory): string {
  return CATEGORY_OPTIONS.find((o) => o.value === category)?.label ?? category;
}

export type NoteSummary = {
  id: string;
  path: string;
  title: string;
  type: NoteType;
  project: string | null;
  date: string;
  tags: string[];
  source: string;
  confidence: number | null;
  /** The meeting's genre; `null` on an unclassified meeting or any other type. */
  category: NoteCategory | null;
  /** How strongly the distill pass backed `category`. `null` once a person has
   * set the category by hand, so its presence answers "did anyone confirm
   * this". */
  category_confidence: number | null;
  /** The per-meeting commitment-tracking override, or `null` to inherit. */
  tracking: "tracked" | "context-only" | null;
  /** A derived one-line body preview for list rows — a UI-only extension
   * beyond the doc'd MCP `NoteSummary`, never stored. */
  snippet: string;
  /** The router's current best guess at a home, for Inbox rows to offer as a
   * suggested destination. The other UI-only extension, likewise derived at
   * list time and never stored; `null` outside Inbox listings, and `null` for
   * a note the router has nothing to say about. */
  guess: RouteGuess | null;
};

/** One opened note: the MCP `get_note` vocabulary, flattened. */
export type NoteDetail = NoteSummary & { body_markdown: string };

/** `write_note`'s echo: `title` is the caller-supplied seed, absent when the
 * filename fell back to the id (listing derives titles from filenames). The
 * two list-only extensions are omitted because the Rust `WrittenNote` doesn't
 * send them — a write echoes the note it just made, and neither a body preview
 * nor a routing guess is a fact about it. The three classification facets are
 * omitted for a different reason: a note this call just created carries none of
 * them, so sending three nulls would be noise rather than news. */
export type CreatedNote = Omit<
  NoteSummary,
  "title" | "snippet" | "guess" | "category" | "category_confidence" | "tracking"
> & {
  title: string | null;
};

/** A note to create. `confidence` is deliberately absent: a hand-created note
 * has no routing score, so the backend files it as `Routing::Manual` and the
 * frontmatter omits the key. */
export type CreateNoteInput = {
  type: NoteType;
  project: string;
  date: string;
  tags: string[];
  source: "manual";
  body: string;
  title: string | null;
};

/** An edit to an existing note. `id` and `project` only locate the file; the
 * backend preserves `id`, `source`, and routing verbatim. Mirrors
 * `SaveNoteInput` in `src-tauri/src/note_cmds.rs`. */
export type SaveNoteInput = {
  id: string;
  project: string;
  type: NoteType;
  date: string;
  tags: string[];
  body: string;
  /** The new display title, or `null` to preserve the stored one. Send `null`
   * whenever the user did not edit it: `NoteSummary.title` is the *effective*
   * title, which for a note with no frontmatter `title` key is its de-slugged
   * filename, so echoing it back verbatim would materialize that fallback into
   * the file. The filename never follows the title. */
  title: string | null;
};

/** The `file_note_to_project` outcome, mirroring the MCP tool's output: the
 * note after routing, where it came from (`project: null` when it was in the
 * Inbox), and whether the file moved (`false` when already in the target). */
export type FileNoteOutcome = {
  note: NoteSummary;
  previous: { path: string; project: string | null };
  moved: boolean;
};

export function createNote(input: CreateNoteInput): Promise<CreatedNote> {
  return invoke<CreatedNote>("write_note", { input });
}

/** One-click human correction: re-route a note to `project`. The backend moves
 * the file, preserves the stable `id`, records the correction as confidence
 * `1.0` (the contract default — `create_project`/`confidence`/`reason` stay at
 * their defaults for this one-click path), and logs a routing example. */
export function fileNoteToProject(input: {
  id: string;
  project: string;
}): Promise<FileNoteOutcome> {
  return invoke<FileNoteOutcome>("file_note_to_project", { input });
}

/** One-click human correction of a meeting's kind. The backend rewrites the
 * note's frontmatter, clears the classifier's confidence in its own guess,
 * records the correction as an example that teaches the classifier, and
 * broadcasts `vault:changed` itself — so call sites apply nothing optimistically
 * and wire no refetch. Mirrors `SetCategoryInput` in
 * `src-tauri/src/note_cmds.rs`. */
export function setNoteCategory(input: {
  id: string;
  category: NoteCategory;
}): Promise<NoteSummary> {
  return invoke<NoteSummary>("set_note_category", { input });
}

export function saveNote(input: SaveNoteInput): Promise<NoteDetail> {
  return invoke<NoteDetail>("save_note", { input });
}

/** The `delete_note` outcome, mirroring `DeletedNoteDto` in
 * `src-tauri/src/note_cmds.rs`: the removed note's id, its title and former
 * project (both `null` when the note was already gone; `project: null` for an
 * Inbox note), and whether a paired recording/transcript was deleted with it. */
export type DeletedNote = {
  id: string;
  title: string | null;
  project: string | null;
  session_deleted: boolean;
};

/** Permanently deletes a note, wherever it lives. The backend also removes the
 * note's paired session artifacts (recording + transcript, for a distilled
 * note) and its index rows, then broadcasts `vault:changed`, so every view
 * refreshes without caller wiring. Deleting an already-gone note is a no-op
 * success. */
export function deleteNote(id: string): Promise<DeletedNote> {
  return invoke<DeletedNote>("delete_note", { id });
}

/** One-off read of a project's notes — the same `list_notes` command
 * `useProjectNotes` wraps, for a caller that needs a single lookup rather
 * than a live-refetching subscription (the filed-toast's click-to-open,
 * which resolves a distill outcome's path to the note it belongs to). */
export function listNotes(project: string): Promise<NoteSummary[]> {
  return invoke<NoteSummary[]>("list_notes", { project });
}

/**
 * A project's notes, newest first, straight from disk — refetched (and
 * response-sequenced) via `useVaultQuery` on every vault change.
 */
export function useProjectNotes(slug: string): {
  notes: NoteSummary[];
  loading: boolean;
  error: string | null;
} {
  const { data, loading, error } = useVaultQuery(
    useCallback(() => invoke<NoteSummary[]>("list_notes", { project: slug }), [slug]),
    // Fixed: every way a listing fails is the same event to the reader, and the
    // reassurance (the files are still there) is what a failed list most needs
    // to say.
    "Couldn't read your notes. They are still on disk; reopen this view to try again.",
  );
  return { notes: data ?? [], loading, error };
}

/**
 * One opened note by `(project, id)`. `setNote` lets a save land its returned
 * state immediately; the vault-changed refetch then reconfirms from disk
 * (`useVaultQuery`'s sequencing keeps an older in-flight read from clobbering
 * the save's echo).
 */
export function useNote(
  project: string,
  id: string,
): {
  note: NoteDetail | null;
  loading: boolean;
  error: string | null;
  setNote: (note: NoteDetail) => void;
} {
  const { data, loading, error, setData } = useVaultQuery(
    useCallback(() => invoke<NoteDetail>("read_note", { project, id }), [project, id]),
  );
  return { note: data, loading, error, setNote: setData };
}

/** Today as a local `YYYY-MM-DD` (not `toISOString`, which is UTC and can be
 * yesterday/tomorrow near midnight). */
export function todayIsoDate(): string {
  const now = new Date();
  const month = String(now.getMonth() + 1).padStart(2, "0");
  const day = String(now.getDate()).padStart(2, "0");
  return `${now.getFullYear()}-${month}-${day}`;
}

/** Splits a comma/space-separated tag field. Normalization (trimming,
 * lowercase folding) lives in the backend's `Tag::parse_normalized` — the one
 * authority, so every entry point folds identically; its message surfaces
 * verbatim on anything still invalid. */
export function parseTagsInput(raw: string): string[] {
  return raw.split(/[\s,]+/).filter(Boolean);
}
