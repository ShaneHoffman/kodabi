import { useMemo } from "react";
import { useNavigation } from "./useNavigation";
import { useProjects } from "./useProjects";

export type Command = {
  id: string;
  title: string;
  /** Quiet right-aligned context, e.g. "Jump to". Searched along with title. */
  hint?: string;
  run: () => void;
};

/**
 * The palette's command registry: one jump per sidebar entry (Inbox first),
 * then the action verbs. "Search notes" is the plain destination; queries
 * typed into the palette reach search via the no-match fallback row instead,
 * so selecting a command by name never leaks the typed text as a query.
 */
export function useCommands(): Command[] {
  const { navigate } = useNavigation();
  const { entries } = useProjects();

  return useMemo(() => {
    const commands: Command[] = entries.map((entry) =>
      entry.kind === "inbox"
        ? {
            id: "jump:inbox",
            title: "Inbox",
            hint: "Jump to",
            run: () => navigate({ kind: "inbox" }),
          }
        : {
            id: `jump:${entry.project.slug}`,
            title: entry.project.slug.split("/").join(" / "),
            hint: "Jump to",
            run: () => navigate({ kind: "project", slug: entry.project.slug }),
          },
    );

    commands.push(
      {
        id: "new-note",
        title: "New note",
        run: () => navigate({ kind: "noteEditor", noteId: null, project: null }),
      },
      {
        id: "open-capture",
        title: "Open capture",
        run: () => navigate({ kind: "capture" }),
      },
      {
        id: "search",
        title: "Search notes",
        run: () => navigate({ kind: "search", query: "" }),
      },
    );

    return commands;
  }, [entries, navigate]);
}
