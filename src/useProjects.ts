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

export function slugDepth(slug: string): number {
  return slug.split("/").length - 1;
}

/** The `list_projects` wire shape: the pinned Inbox badge count plus every
 * project on disk, already sorted by slug (parents before children). */
type ProjectList = {
  inbox_note_count: number;
  projects: Project[];
};

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
