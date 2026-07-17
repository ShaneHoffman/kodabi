import { PALETTE_SHORTCUT_LABEL } from "../useCommandPalette";
import { useNavigation } from "../useNavigation";
import { entryView, isEntrySelected, slugDepth, useProjects } from "../useProjects";
import { ListeningIndicator } from "./ListeningIndicator";
import { Button } from "./ui/Button";
import "./Sidebar.css";

type Props = {
  onOpenPalette: () => void;
};

/**
 * The quiet project switcher: Inbox sentinel pinned first, projects below
 * (nested ones indented by slug depth), the persistent listening indicator
 * and the palette affordance at the foot. Selection is a surface value
 * shift — never the reserved green.
 */
export function Sidebar({ onOpenPalette }: Props) {
  const { entries } = useProjects();
  const { view, navigate } = useNavigation();

  return (
    <aside className="sidebar flex w-64 flex-none flex-col gap-md bg-bg-sink p-md">
      {/* The document's h1: heading navigation needs a level-1 root even
          though the wordmark reads quietly (preflight strips h1 sizing). */}
      <h1 className="font-serif text-body text-text-soft">kodabi</h1>

      <nav
        aria-label="Projects"
        className="flex min-h-0 flex-1 flex-col gap-3xs overflow-y-auto"
      >
        <p className="sidebar__eyebrow mb-2xs text-eyebrow uppercase text-text-faint">
          Projects
        </p>
        {entries.map((entry) => {
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
                selected
                  ? "is-selected bg-surface text-text"
                  : "text-text-soft hover:text-text"
              }`}
            >
              <span>{name}</span>
              <span className="text-cap text-text-faint">{count}</span>
            </Button>
          );
        })}
      </nav>

      <footer className="flex flex-col gap-sm">
        <ListeningIndicator />
        <Button
          variant="quiet"
          onClick={onOpenPalette}
          className="flex w-full items-baseline justify-between text-left text-cap text-text-faint hover:text-text-soft"
        >
          <span>Commands</span>
          <span>{PALETTE_SHORTCUT_LABEL}</span>
        </Button>
      </footer>
    </aside>
  );
}
