import { PALETTE_SHORTCUT_LABEL } from "../useCommandPalette";
import { useNavigation } from "../useNavigation";
import {
  entryView,
  isEntrySelected,
  slugDepth,
  useProjects,
  type SidebarEntry,
} from "../useProjects";
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
          though the wordmark reads quietly (preflight strips h1 sizing). */}
      <h1 className="font-serif text-body text-text-soft">kodabi</h1>

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
            <StatusMessage variant="status" compact>
              No projects yet.
            </StatusMessage>
          )}
          {projectEntries.map(renderRow)}
        </div>
      </nav>

      <footer className="flex flex-col gap-sm">
        <ListeningIndicator />
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
      </footer>
    </aside>
  );
}
