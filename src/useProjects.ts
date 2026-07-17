import type { View } from "./useNavigation";

/**
 * Mirrors the MCP `Project` shape (docs/MCP_TOOL_SURFACE.md) so the Phase 3
 * `list_projects` swap never touches the shell: replace this hook's body with
 * an invoke (following the useCaptureState mount pattern) and the
 * `SidebarEntry` return type stays stable.
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

const SAMPLE_ENTRIES: SidebarEntry[] = [
  { kind: "inbox", note_count: 3 },
  {
    kind: "project",
    project: {
      id: "p_growth",
      slug: "Growth",
      display_name: "Growth",
      parent: null,
      note_count: 5,
      meeting_count: 2,
      last_activity: "2026-07-14T18:20:00Z",
    },
  },
  {
    kind: "project",
    project: {
      id: "p_q3",
      slug: "Growth/Q3",
      display_name: "Q3",
      parent: "Growth",
      note_count: 8,
      meeting_count: 4,
      last_activity: "2026-07-16T09:05:00Z",
    },
  },
  {
    kind: "project",
    project: {
      id: "p_pgolf",
      slug: "Briarwood Golf",
      display_name: "Briarwood Golf",
      parent: null,
      note_count: 2,
      meeting_count: 1,
      last_activity: null,
    },
  },
];

/** Stub until the Phase 3 `list_projects` backend exists — sample data only. */
export function useProjects(): { entries: SidebarEntry[]; loading: boolean } {
  return { entries: SAMPLE_ENTRIES, loading: false };
}
