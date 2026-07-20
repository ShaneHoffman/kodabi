import { useMemo } from "react";
import { useNavigation } from "./useNavigation";
import { entryView, formatSlug, useProjects } from "./useProjects";
import { useFailedSessions } from "./useSessions";
import { showQuickCaptureWindow } from "./quickCapture";

/** Which half of the palette a command belongs to: somewhere to go, or
 * something to do. The palette heads these as sections while the list is
 * unfiltered. */
export type CommandGroup = "jump" | "action";

export const COMMAND_GROUP_LABEL: Record<CommandGroup, string> = {
  jump: "Jump to",
  action: "Actions",
};

export type Command = {
  id: string;
  title: string;
  group: CommandGroup;
  /** Quiet right-aligned context, e.g. "Jump to". Searched along with title,
   * and shown only once filtering has broken the sections up — inside its own
   * section the heading has already said it. */
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
  const { sessions } = useFailedSessions();

  return useMemo(() => {
    const commands: Command[] = entries.map((entry) => ({
      id: entry.kind === "inbox" ? "jump:inbox" : `jump:${entry.project.slug}`,
      title: entry.kind === "inbox" ? "Inbox" : formatSlug(entry.project.slug),
      group: "jump",
      hint: "Jump to",
      run: () => navigate(entryView(entry)),
    }));

    // Conditional, like the sidebar row it mirrors: a jump to a view whose only
    // content is "all clear" is a command that wastes the one thing the palette
    // sells, which is that everything in it is worth doing.
    if (sessions.length > 0) {
      commands.push({
        id: "needs-attention",
        title: "Needs attention",
        group: "jump",
        hint: "Jump to",
        run: () => navigate({ kind: "needsAttention" }),
      });
    }

    commands.push(
      {
        id: "new-note",
        group: "action",
        title: "New note",
        run: () => navigate({ kind: "noteEditor", noteId: null, project: null }),
      },
      {
        id: "open-capture",
        group: "action",
        title: "Quick capture",
        // Quick capture is its own window (#45), not a main-window view — the
        // palette action pops it, matching the global hotkey.
        run: () => {
          void showQuickCaptureWindow();
        },
      },
      {
        id: "search",
        group: "action",
        title: "Search notes",
        run: () => navigate({ kind: "search", query: "" }),
      },
      {
        id: "settings",
        group: "action",
        title: "Settings",
        run: () => navigate({ kind: "settings" }),
      },
    );

    return commands;
  }, [entries, navigate, sessions]);
}
