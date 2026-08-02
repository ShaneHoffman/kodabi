import { useCallback, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { View } from "./useNavigation";
import { useVaultQuery } from "./useVaultQuery";

/**
 * Mirrors the MCP `Project` shape (docs/MCP_TOOL_SURFACE.md), fed by the
 * `list_projects` command (a disk scan of the knowledge base — `id` is the
 * slug until an index exists to mint anything more opaque).
 */
export type Project = {
  id: string;
  slug: string;
  display_name: string;
  parent: string | null;
  note_count: number;
  meeting_count: number;
  last_activity?: string | null;
};

/**
 * Inbox is the sentinel unrouted bucket, not a real project (frontmatter
 * stores `project: Inbox`; the MCP NoteSummary represents it as null), so it
 * is a first-class union member rather than a fake Project row.
 */
export type SidebarEntry =
  | { kind: "inbox"; note_count: number }
  | { kind: "project"; project: Project };

/**
 * The one entry→destination mapping. Every surface that jumps to or matches
 * a sidebar entry (sidebar rows, palette commands, selection highlight)
 * derives from here, so a View shape change lands in exactly one place.
 */
export function entryView(entry: SidebarEntry): View {
  return entry.kind === "inbox"
    ? { kind: "inbox" }
    : { kind: "project", slug: entry.project.slug };
}

/** Whether `view` is the destination `entry` navigates to. */
export function isEntrySelected(view: View, entry: SidebarEntry): boolean {
  const target = entryView(entry);
  if (target.kind === "project") {
    return view.kind === "project" && view.slug === target.slug;
  }
  return view.kind === target.kind;
}

/* Slug policy — segments are "/"-separated. Every parse of a slug's
   structure goes through these, next to the Project type they describe. */

export function formatSlug(slug: string): string {
  return slug.split("/").join(" / ");
}

/** The four folder hues (docs/DESIGN_SYSTEM.md §2). Marigold is deliberately
 * absent: it means failure, and a project must never be mistaken for one. */
export type FolderHue = "coral" | "cobalt" | "teal" | "plum";

const FOLDER_HUES: readonly FolderHue[] = ["coral", "cobalt", "teal", "plum"];

/**
 * A project's hue, derived from its slug.
 *
 * Hues are identity, never status, so this has to be a pure function of the
 * name and nothing else: cycling by list position would repaint every folder
 * below a newly created one, and a colour that moves is a colour that means
 * nothing. Derived rather than stored because a hue is a rendering of the
 * project, not a fact about it — nothing on disk or in the index carries one.
 *
 * The hash is the textbook 31x rotation, `| 0` to stay in int32 rather than
 * drifting into float territory on a long slug. Its exact output is pinned by
 * a test: changing it would silently recolour every vault in existence.
 */
export function folderHue(slug: string): FolderHue {
  let hash = 0;
  for (let index = 0; index < slug.length; index += 1) {
    hash = (hash * 31 + slug.charCodeAt(index)) | 0;
  }
  return FOLDER_HUES[Math.abs(hash) % FOLDER_HUES.length];
}

export function slugDepth(slug: string): number {
  return slug.split("/").length - 1;
}

/** The `list_projects` wire shape: the pinned Inbox badge count plus every
 * project on disk, already sorted by slug (parents before children). */
type ProjectList = {
  inbox_note_count: number;
  projects: Project[];
};

/** Mirrors `DeletedProjectDto` in src-tauri/src/note_cmds.rs (`delete_project`). */
export type DeletedProject = {
  slug: string;
  moved_note_count: number;
};

/**
 * Creates an empty project folder, echoing the canonical (casing-adopted)
 * project row (`ProjectDto` in src-tauri/src/note_cmds.rs) so the caller can
 * navigate straight to it. List refreshes ride the backend's `vault:changed`
 * broadcast.
 */
export function createProject(project: string): Promise<Project> {
  return invoke<Project>("create_project", { project });
}

/**
 * Deletes a project; its notes (including notes in child projects) move back
 * to the Inbox first. The backend broadcasts `vault:changed` and queues an
 * index reconcile, so every view refreshes without caller wiring.
 */
export function deleteProject(project: string): Promise<DeletedProject> {
  return invoke<DeletedProject>("delete_project", { project });
}

/**
 * The sidebar's world, straight from disk: Inbox pinned first, projects in
 * backend slug order. Fetched (and response-sequenced) via `useVaultQuery`,
 * refetched on every vault change, so a note created into a brand-new project
 * surfaces without a restart. `error` must be surfaced by the consumer — a
 * failed listing looks identical to an empty vault otherwise.
 */
export function useProjects(): {
  entries: SidebarEntry[];
  loading: boolean;
  error: string | null;
} {
  const { data, loading, error } = useVaultQuery(
    useCallback(() => invoke<ProjectList>("list_projects"), []),
  );

  // Memoized so consumers keyed on `entries` (useCommands) don't recompute
  // every render.
  const entries = useMemo<SidebarEntry[]>(
    () => [
      { kind: "inbox", note_count: data?.inbox_note_count ?? 0 },
      ...(data?.projects ?? []).map((project) => ({ kind: "project" as const, project })),
    ],
    [data],
  );

  return { entries, loading, error };
}
