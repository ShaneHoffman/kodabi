import { DISTILL_STATE_EVENT } from "../events";
import { PALETTE_SHORTCUT_LABEL } from "../useCommandPalette";
import type { DistillEvent } from "../useDistillState";
import { useNavigation } from "../useNavigation";
import {
  entryView,
  isEntrySelected,
  slugDepth,
  useProjects,
  type SidebarEntry,
} from "../useProjects";
import { useFailedSessions } from "../useSessions";
import { useTauriEvent } from "../useTauriEvent";
import { notifyVaultChanged } from "../useVaultQuery";
import { ListeningIndicator } from "./ListeningIndicator";
import { Button } from "./ui/Button";
import { StatusMessage } from "./ui/StatusMessage";
import "./Sidebar.css";

type Props = {
  onOpenPalette: () => void;
};

/**
 * The quiet project switcher: the Inbox sentinel stands on its own at the top
 * (it is not a project — it is where unrouted notes wait), then the projects
 * below their own heading, nested ones indented by slug depth. A hairline
 * divides the two so the Inbox never reads as just another project. The
 * persistent listening indicator and the palette affordance sit at the foot.
 * Selection is a surface value shift — never the reserved green.
 */
export function Sidebar({ onOpenPalette }: Props) {
  const { entries, loading, error } = useProjects();
  const { view, navigate } = useNavigation();

  const inboxEntry = entries.find((entry) => entry.kind === "inbox");
  const projectEntries = entries.filter((entry) => entry.kind === "project");

  const renderRow = (entry: SidebarEntry) => {
    const selected = isEntrySelected(view, entry);
    const name = entry.kind === "inbox" ? "Inbox" : entry.project.display_name;
    const count =
      entry.kind === "inbox" ? entry.note_count : entry.project.note_count;
    const depth = entry.kind === "inbox" ? 0 : slugDepth(entry.project.slug);
    return (
      <Button
        key={entry.kind === "inbox" ? "inbox" : entry.project.id}
        variant="quiet"
        aria-current={selected ? "page" : undefined}
        onClick={() => navigate(entryView(entry))}
        style={{
          paddingInlineStart: `calc(var(--space-xs) + ${depth} * var(--space-sm))`,
        }}
        className={`sidebar__row flex w-full items-baseline justify-between text-left text-body ${
          selected ? "is-selected bg-surface text-text" : "text-text-soft"
        }`}
      >
        <span>{name}</span>
        <span className="text-cap text-text-faint">{count}</span>
      </Button>
    );
  };

  return (
    <aside className="sidebar flex w-64 flex-none flex-col gap-md bg-bg-sink p-md">
      {/* The document's h1: heading navigation needs a level-1 root even
          though the wordmark reads quietly (preflight strips h1 sizing).
          It sits at ink value rather than soft — it is the one piece of the
          window that names the app, and reading like a disabled nav row was
          under-selling it — with the eyebrow's tracking to set it apart from
          the rows below without making it bigger than them. */}
      <h1 className="font-serif text-body tracking-caps text-text">kodabi</h1>

      <nav
        aria-label="Knowledge base"
        className="flex min-h-0 flex-1 flex-col gap-md overflow-y-auto"
      >
        {/* A failed listing must not masquerade as an empty vault. */}
        {error && (
          <StatusMessage variant="error" compact>
            Couldn&apos;t load projects: {error}
          </StatusMessage>
        )}

        {inboxEntry && (
          <div className="flex flex-col gap-3xs">{renderRow(inboxEntry)}</div>
        )}

        <div className="sidebar__group flex flex-col gap-3xs pt-md">
          <p className="mb-2xs text-eyebrow uppercase tracking-eyebrow text-text-faint">
            Projects
          </p>
          {/* Gated on !loading too: without it the first read of every cold
              start rendered "No projects yet." before the listing landed, so
              the app opened by telling the user their vault was empty
              (docs/DESIGN_SYSTEM.md §3). */}
          {projectEntries.length === 0 && !loading && !error && (
            // variant="empty", not "status": the variant fixes the ARIA role,
            // and this is first-run copy rather than progress — role="status"
            // would make a static sentence a live region (§3).
            <StatusMessage variant="empty" compact>
              No projects yet.
            </StatusMessage>
          )}
          {projectEntries.map(renderRow)}
        </div>
      </nav>

      {/* Two groups, not four evenly-spaced rows: what capture is doing right
          now is a status, and the rest are places to go. Flat at one gap they
          read as one arbitrary list, and the indicator's mark sat 12px left of
          every label because it alone carried no control padding. The px-xs
          wrapper puts the mark on the same left edge as the row labels below,
          so the group has one column instead of three. */}
      <footer className="flex flex-col gap-md">
        <div className="px-xs">
          <ListeningIndicator />
        </div>
        <div className="flex flex-col gap-3xs">
          <NeedsAttentionRow />
          <Button
            variant="quiet"
            aria-current={view.kind === "settings" ? "page" : undefined}
            onClick={() => navigate({ kind: "settings" })}
            className={`flex w-full items-baseline text-left text-cap ${
              view.kind === "settings" ? "text-text-soft" : "text-text-faint"
            }`}
          >
            <span>Settings</span>
          </Button>
          <Button
            variant="quiet"
            onClick={onOpenPalette}
            className="flex w-full items-baseline justify-between text-left text-cap text-text-faint"
          >
            <span>Commands</span>
            <span>{PALETTE_SHORTCUT_LABEL}</span>
          </Button>
        </div>
      </footer>
    </aside>
  );
}

/**
 * The way in to NeedsAttentionView, and the reason failed captures could leave
 * the Inbox at all: without a standing signal, moving them out of the way would
 * have moved them out of sight.
 *
 * It appears only when there is something to act on and disappears at zero, so
 * the sidebar carries no permanent reminder of a problem nobody has. It leads
 * the footer's nav group rather than the nav above: that nav is labelled
 * "Knowledge base", and a capture that failed to distill is not knowledge, it is
 * plumbing. The footer is bottom-anchored, so the row grows the group upward
 * and Settings and Commands never move under the pointer.
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

  // A failed listing must not masquerade as all clear, so it keeps the row.
  if (sessions.length === 0 && !error) return null;

  const selected = view.kind === "needsAttention";
  return (
    <Button
      variant="quiet"
      data-testid="needs-attention-nav"
      aria-current={selected ? "page" : undefined}
      onClick={() => navigate({ kind: "needsAttention" })}
      // A step above Settings' faint: this is work someone has to do, and rank
      // is carried by value (docs/DESIGN_SYSTEM.md §1).
      className="flex w-full items-baseline justify-between text-left text-cap text-text-soft"
    >
      <span>Needs attention</span>
      {sessions.length > 0 && (
        <span className="text-text-faint">{sessions.length}</span>
      )}
    </Button>
  );
}
