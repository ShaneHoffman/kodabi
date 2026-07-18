import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useVaultQuery } from "./useVaultQuery";

export { notifyVaultChanged, onVaultChanged } from "./useVaultQuery";

/*
 * The note wire shapes, mirroring the Rust DTOs in `src-tauri/src/note_cmds.rs`
 * (which in turn mirror the MCP `NoteSummary` — docs/MCP_TOOL_SURFACE.md):
 * `project: null` stands in for the frontmatter's Inbox sentinel, and
 * `confidence: null` for an omitted key (a hand-filed note carries no routing
 * score).
 */

export type NoteType = "meeting" | "note" | "chat";

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

export function createNote(input: CreateNoteInput): Promise<CreatedNote> {
  return invoke<CreatedNote>("write_note", { input });
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
