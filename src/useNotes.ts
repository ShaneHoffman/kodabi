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
  /** A derived one-line body preview for list rows — a UI-only extension
   * beyond the doc'd MCP `NoteSummary`, never stored. */
  snippet: string;
};

/** One opened note: the MCP `get_note` vocabulary, flattened. */
export type NoteDetail = NoteSummary & { body_markdown: string };

/** `write_note`'s echo: `title` is the caller-supplied seed, absent when the
 * filename fell back to the id (listing derives titles from filenames). */
export type CreatedNote = Omit<NoteSummary, "title"> & { title: string | null };

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
 * backend preserves `id`, `source`, and routing verbatim. */
export type SaveNoteInput = {
  id: string;
  project: string;
  type: NoteType;
  date: string;
  tags: string[];
  body: string;
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

export function saveNote(input: SaveNoteInput): Promise<NoteDetail> {
  return invoke<NoteDetail>("save_note", { input });
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
