import type { CSSProperties } from "react";
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

/** A row's nesting depth, handed to CSS so the indent composes with the row
 * padding instead of overriding it. */
function depthStyle(depth: number): CSSProperties | undefined {
  return depth > 0 ? ({ "--row-depth": depth } as CSSProperties) : undefined;
}

/**
 * The shell's left rail, top to bottom: what the app is called, where the
 * work is, what you can browse, what capture is doing, and the chrome.
 *
 * Two structural ideas carry it. First, it shares the PAGE plane with the
 * content beside it — one sheet of paper, divided by a single hairline —
 * rather than sitting on a recessed fill that read as a second box. Second,
 * the active destination is the only raised thing in it: a pill that lifts
 * lighter than the page, so where you are is legible before any label is
 * read. Selection is value, never the reserved green.
 *
 * The Inbox and Needs attention lead as PEERS in a system rail of their own,
 * above the projects. They are the two places work waits; a project is a
 * place to browse. The rule under the wordmark and the value step down to the
 * footer are the only two lines in the rail, and each separates a genuinely
 * different kind of thing.
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
    const rail = entry.kind === "inbox";
    return (
      <Button
        key={entry.kind === "inbox" ? "inbox" : entry.project.id}
        variant="quiet"
        aria-current={selected ? "page" : undefined}
        onClick={() => navigate(entryView(entry))}
        style={depthStyle(depth)}
        className={`sidebar__row${rail ? " sidebar__row--rail" : ""} flex w-full items-center justify-between gap-2xs text-left text-body ${
          selected ? "is-selected text-text" : "text-text-soft"
        }`}
      >
        <span className="truncate">{name}</span>
        <span className="font-mono text-cap font-normal text-text-faint">
          {count}
        </span>
      </Button>
    );
  };

  return (
    <aside className="sidebar flex flex-none flex-col bg-bg">
      {/* The document's h1: heading navigation needs a level-1 root even
          though the wordmark reads quietly (preflight strips h1 sizing). It is
          the one piece of the window that names the app, and it is the only
          place the reading face appears outside a note — the wordmark is a
          word you read, not a control you operate. */}
      <h1 className="sidebar__wordmark font-serif text-wordmark tracking-wordmark text-text">
        kodabi
      </h1>
      <div className="sidebar__divider" />

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

        {/* The system rail: the two places work waits, as peers. */}
        <div className="flex flex-col gap-3xs">
          {inboxEntry && renderRow(inboxEntry)}
          <NeedsAttentionRow />
        </div>

        <p className="sidebar__eyebrow px-2xs pb-2xs text-eyebrow font-semibold uppercase tracking-rail text-text-faint">
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
        <div className="sidebar__list">{projectEntries.map(renderRow)}</div>
      </nav>

      {/* mt-auto pins the foot, so the rows above can grow without the
          Settings and Commands rows moving under the pointer. What capture is
          doing is a status; the two below it are places to go, and the value
          step between them is what says so. */}
      <footer className="mt-auto flex flex-col">
        <ListeningIndicator />
        <div className="sidebar__list">
          <Button
            variant="quiet"
            aria-current={view.kind === "settings" ? "page" : undefined}
            onClick={() => navigate({ kind: "settings" })}
            className={`sidebar__row flex w-full text-left text-body ${
              view.kind === "settings" ? "is-selected text-text" : "text-text-soft"
            }`}
          >
            <span>Settings</span>
          </Button>
          <Button
            variant="quiet"
            onClick={onOpenPalette}
            className="sidebar__row flex w-full items-center justify-between gap-2xs text-left text-body text-text-faint"
          >
            <span>Commands</span>
            <span className="font-mono text-cap">{PALETTE_SHORTCUT_LABEL}</span>
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
 * It stands beside the Inbox in the system rail rather than down in the
 * chrome, because it is the same kind of thing — a queue with a count that
 * someone has to clear. It still disappears at zero, so the sidebar carries no
 * permanent reminder of a problem nobody has, and the rail collapses to the
 * single Inbox row it started as.
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
      className={`sidebar__row sidebar__row--rail flex w-full items-center justify-between gap-2xs text-left text-body ${
        selected ? "is-selected text-text" : "text-text-soft"
      }`}
    >
      <span>Needs attention</span>
      {sessions.length > 0 && (
        <span className="font-mono text-cap font-normal text-text-faint">
          {sessions.length}
        </span>
      )}
    </Button>
  );
}
