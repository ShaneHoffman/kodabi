import { useMemo } from "react";
import { useNavigation } from "./useNavigation";
import { entryView, formatSlug, useProjects } from "./useProjects";
import { showQuickCaptureWindow } from "./quickCapture";

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
    const commands: Command[] = entries.map((entry) => ({
      id: entry.kind === "inbox" ? "jump:inbox" : `jump:${entry.project.slug}`,
      title: entry.kind === "inbox" ? "Inbox" : formatSlug(entry.project.slug),
      hint: "Jump to",
      run: () => navigate(entryView(entry)),
    }));

    commands.push(
      {
        id: "new-note",
        title: "New note",
        run: () => navigate({ kind: "noteEditor", noteId: null, project: null }),
      },
      {
        id: "open-capture",
        title: "Quick capture",
        // Quick capture is its own window (#45), not a main-window view — the
        // palette action pops it, matching the global hotkey.
        run: () => {
          void showQuickCaptureWindow();
        },
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
