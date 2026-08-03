import { useState, type CSSProperties } from "react";
import { cva } from "class-variance-authority";
import { clsx } from "clsx";
import { DISTILL_STATE_EVENT } from "../../events";
import type { DistillEvent } from "../../useDistillState";
import { useNavigation } from "../../useNavigation";
import {
  entryView,
  folderHue,
  isEntrySelected,
  slugDepth,
  useProjects,
  type FolderHue,
  type SidebarEntry,
} from "../../useProjects";
import { useFailedSessions } from "../../useSessions";
import { useTauriEvent } from "../../useTauriEvent";
import { notifyVaultChanged } from "../../useVaultQuery";
import { CreateProjectDialog } from "../dialogs/CreateProjectDialog";
import { StatusMessage } from "../ui/StatusMessage";

/**
 * A dock row. Not the Button primitive: an action is a rectangle that lifts,
 * and a destination is a place that fills. The active state here is a fill
 * plus an edge plus the lit top line — the same glass grammar as the dock
 * itself, one step brighter — which no Button variant carries.
 *
 * The transparent border on the resting row is load-bearing: gaining a border
 * on selection would otherwise shift every row's box by a pixel.
 */
const dockRow = cva(
  [
    "focus-ring-inset flex w-full items-center gap-[9px] rounded-button border py-2 pr-2.5",
    "text-left text-[13.5px]",
    "transition-[background-color,border-color,color] duration-140 ease-out-strong",
  ],
  {
    variants: {
      selected: {
        true: "border-edge bg-wash-hover text-ink shadow-[inset_0_1px_0_var(--color-edge-lit)]",
        false: "border-transparent text-ink-dim hover:bg-wash hover:text-ink",
      },
    },
    defaultVariants: { selected: false },
  },
);

/** Spelled out per hue, never built from a template: Tailwind reads source
 * text, so a constructed class name is a class that does not ship. */
const HUE_DOT: Record<FolderHue, string> = {
  coral: "bg-coral",
  cobalt: "bg-cobalt",
  teal: "bg-teal",
  plum: "bg-plum",
};

/** The row's left padding, which is also where nesting is expressed. Inline
 * because the depth is data, not a design step. */
const ROW_BASE_PAD = 10;
function depthStyle(depth: number): CSSProperties | undefined {
  return depth > 0
    ? { paddingLeft: `${ROW_BASE_PAD + depth * 12}px` }
    : undefined;
}

/**
 * The navigation dock: where the work is, what you can browse, and the two
 * tools at the foot.
 *
 * It is a floating pane on the grove ground rather than a rail sharing the
 * page plane — glass on glass, with its own scroll — so a long folder list
 * never pushes the tools at the foot out of reach, and the panel beside it
 * keeps its own edges.
 *
 * The scroll belongs to the DESTINATIONS, not to the pane. Put it on the
 * <aside> instead and the foot stops being a foot: `mt-auto` only distributes
 * FREE space, and an overflowing column has none, so a vault past a dozen
 * folders pushed Chat and Terminal below the fold and made reaching them a
 * scroll. Bounding the list is what keeps them pinned.
 *
 * The Inbox, Needs attention and Search lead as PEERS in a system group: they
 * are the three things that act on the whole vault. Folders are places to
 * browse and carry their hue and their count. Chat and Terminal sit under a
 * hairline at the foot because they are tools over the knowledge base, not
 * places inside it.
 *
 * Selection is value, never the reserved green — including the count, which
 * brightens to ink on the active row rather than taking the kodama's colour
 * (docs/DESIGN_SYSTEM.md §2).
 */
export function Dock() {
  const { entries, loading, error } = useProjects();
  const { view, navigate } = useNavigation();
  const [creatingProject, setCreatingProject] = useState(false);

  const inboxEntry = entries.find((entry) => entry.kind === "inbox");
  const projectEntries = entries.filter((entry) => entry.kind === "project");

  const renderRow = (entry: SidebarEntry) => {
    const selected = isEntrySelected(view, entry);
    const inbox = entry.kind === "inbox";
    const name = inbox ? "Inbox" : entry.project.display_name;
    const count = inbox ? entry.note_count : entry.project.note_count;
    const depth = inbox ? 0 : slugDepth(entry.project.slug);
    return (
      // A real list item. The nav lists were <div>s of buttons inside <nav>, so
      // a screen reader announced neither that they were lists nor how many
      // places there were to go — which is most of what a dock is for.
      <li key={inbox ? "inbox" : entry.project.id}>
        <button
          type="button"
          data-testid={inbox ? "sidebar-inbox" : undefined}
          aria-current={selected ? "page" : undefined}
          onClick={() => navigate(entryView(entry))}
          style={depthStyle(depth)}
          className={clsx(dockRow({ selected }), depth === 0 && "pl-2.5")}
        >
          {!inbox && (
            <span
              aria-hidden="true"
              className={clsx(
                "size-[9px] flex-none rounded-[3px]",
                HUE_DOT[folderHue(entry.project.slug)],
              )}
            />
          )}
          {/* min-w-0 is what lets it truncate at all: a flex child's default
              min-width is its content, so a long folder name would blow the
              dock open instead of shortening. */}
          <span className="min-w-0 flex-1 truncate">{name}</span>
          {/* Tabular figures: this column counts up and down as notes are
              filed, and proportional digits made the numbers shuffle sideways
              against the dock's right edge as they changed. */}
          <span
            data-testid={inbox ? "sidebar-inbox-count" : undefined}
            className={clsx(
              "font-data text-[11px] tabular-nums",
              selected ? "text-ink" : "text-ink-faint",
            )}
          >
            {count}
          </span>
        </button>
      </li>
    );
  };

  return (
    <aside className="glass-dock flex w-56 flex-none flex-col px-3 py-3.5">
      {/* min-h-0 is what makes the overflow real: a flex child's default
          min-height is its content, so without it the nav grows past the pane
          and takes the foot with it instead of scrolling. */}
      <nav
        aria-label="Knowledge base"
        className="flex min-h-0 flex-1 flex-col overflow-y-auto"
      >
        {/* A failed listing must not masquerade as an empty vault. */}
        {error && (
          <StatusMessage variant="error" compact>
            Couldn&apos;t load projects: {error}
          </StatusMessage>
        )}

        {/* The system group: the three things that act on the whole vault. */}
        <ul aria-label="System" className="flex flex-col gap-0.5">
          {inboxEntry && renderRow(inboxEntry)}
          <NeedsAttentionRow />
          <li>
            <button
              type="button"
              aria-current={view.kind === "search" ? "page" : undefined}
              onClick={() => navigate({ kind: "search", query: "" })}
              className={clsx(
                dockRow({ selected: view.kind === "search" }),
                "pl-2.5",
              )}
            >
              <span className="min-w-0 flex-1 truncate">Search</span>
            </button>
          </li>
        </ul>

        {/* The eyebrow and the dock's one creation verb share a baseline row.
            The button is always visible (hover may enhance, never reveal). */}
        <div className="flex items-baseline justify-between pt-4 pb-1.5">
          <p className="px-2.5 font-data text-[10px] uppercase tracking-[0.22em] text-ink-faint">
            Folders
          </p>
          <button
            type="button"
            aria-haspopup="dialog"
            aria-label="New project"
            onClick={() => setCreatingProject(true)}
            className="focus-ring rounded-[6px] px-2 py-0.5 text-[11px] text-ink-dim transition-colors duration-140 ease-out-strong hover:bg-wash hover:text-ink"
          >
            New
          </button>
        </div>
        {/* Gated on !loading too: without it the first read of every cold start
            rendered "No projects yet." before the listing landed, so the app
            opened by telling the user their vault was empty
            (docs/DESIGN_SYSTEM.md §3). */}
        {projectEntries.length === 0 && !loading && !error && (
          // variant="empty", not "status": the variant fixes the ARIA role, and
          // this is first-run copy rather than progress — role="status" would
          // make a static sentence a live region (§3).
          <StatusMessage variant="empty" compact>
            No projects yet. Use the New button to create one.
          </StatusMessage>
        )}
        <ul aria-label="Folders" className="flex flex-col gap-0.5">
          {projectEntries.map(renderRow)}
        </ul>
      </nav>

      {/* mt-auto pins the foot on a SHORT list, and the nav's own scroll above
          pins it on a long one, so the folders can grow without the tools
          moving under the pointer either way. Its own landmark:
          `aria-current="page"` only means something inside a <nav>. */}
      <nav aria-label="Tools" className="mt-auto border-t border-edge pt-2">
        <ul className="flex flex-col gap-0.5">
          <li>
            {/* Chat leads: it is the front door onto the knowledge base
                (FOUNDING_DOC §4). The terminal is the same session shape with
                the safety off, so it follows rather than leads. */}
            <button
              type="button"
              aria-current={view.kind === "chat" ? "page" : undefined}
              onClick={() => navigate({ kind: "chat" })}
              className={clsx(
                dockRow({ selected: view.kind === "chat" }),
                "pl-2.5",
              )}
            >
              <span className="min-w-0 flex-1 truncate">Chat</span>
            </button>
          </li>
          <li>
            <button
              type="button"
              aria-current={view.kind === "terminal" ? "page" : undefined}
              onClick={() => navigate({ kind: "terminal" })}
              className={clsx(
                dockRow({ selected: view.kind === "terminal" }),
                "pl-2.5",
              )}
            >
              <span className="min-w-0 flex-1 truncate">Terminal</span>
            </button>
          </li>
        </ul>
      </nav>

      {creatingProject && (
        <CreateProjectDialog onClose={() => setCreatingProject(false)} />
      )}
    </aside>
  );
}

/**
 * The way in to NeedsAttentionView, and the reason failed captures could leave
 * the Inbox at all: without a standing signal, moving them out of the way would
 * have moved them out of sight.
 *
 * It stands beside the Inbox in the system group rather than down in the
 * chrome, because it is the same kind of thing — a queue with a count that
 * someone has to clear. A dismissed capture *is* cleared, so the row counts
 * and gates on the undismissed only: dismissing the last one collapses the
 * group back to the rows it started as, which is the whole point of
 * dismissing. (Dismissed captures stay reachable through the command palette's
 * needs-attention jump, which keys on the full listing.)
 *
 * It also owns the distill-failure refetch. That listener used to live in the
 * Inbox, which meant a failure only reached the list while the Inbox happened
 * to be open; this row is mounted for the whole session, so it is the one place
 * that can promise the signal arrives wherever the user is standing.
 */
function NeedsAttentionRow() {
  const { sessions, error } = useFailedSessions();
  const { view, navigate } = useNavigation();

  useTauriEvent<DistillEvent>(DISTILL_STATE_EVENT, (payload) => {
    if (payload.status === "error") notifyVaultChanged();
  });

  const activeCount = sessions.filter((session) => !session.dismissed).length;

  // A failed listing must not masquerade as all clear, so it keeps the row.
  if (activeCount === 0 && !error) return null;

  const selected = view.kind === "needsAttention";
  return (
    <li>
      <button
        type="button"
        data-testid="needs-attention-nav"
        aria-current={selected ? "page" : undefined}
        onClick={() => navigate({ kind: "needsAttention" })}
        className={clsx(dockRow({ selected }), "pl-2.5")}
      >
        <span className="min-w-0 flex-1 truncate">Needs attention</span>
        {activeCount > 0 && (
          <span
            className={clsx(
              "font-data text-[11px] tabular-nums",
              selected ? "text-ink" : "text-ink-faint",
            )}
          >
            {activeCount}
          </span>
        )}
      </button>
    </li>
  );
}
