import { useMemo } from "react";
import { useNavigation } from "./useNavigation";
import { entryView, folderHue, formatSlug, useProjects, type FolderHue } from "./useProjects";
import { useFailedSessions } from "./useSessions";
import { showQuickCaptureWindow } from "./quickCapture";

/**
 * What kind of thing a command is. The palette groups by this and prints it as
 * the row's kind tag, so it is the answer to "what am I about to do" rather
 * than a filing category: a place, a folder, or a verb.
 *
 * **These strings are user-facing copy** — the palette renders the member
 * itself rather than mapping it through a table, so renaming one here changes
 * what the tag says. Keep them words a reader would use.
 *
 * `folder` is split out from `navigate` (both are somewhere to go) because a
 * folder is the one row that carries an identity hue, and because a vault with
 * forty projects would otherwise bury the six fixed destinations in them.
 */
export type CommandKind = "navigate" | "folder" | "action";

export type Command = {
  id: string;
  title: string;
  kind: CommandKind;
  /** Folder rows only: the project's identity hue, for the row's dot. */
  hue?: FolderHue;
  /** The keyboard shortcut that runs this without opening the palette, e.g.
   * "Ctrl+Shift+K". Searched along with the title. Only a command that really
   * has a global binding carries one — the palette is where people learn the
   * shortcuts, so an invented hint here teaches something false. */
  hint?: string;
  run: () => void;
};

/**
 * The palette's command registry: one jump per sidebar entry (Inbox first),
 * then the action verbs. "Search notes" is the plain destination; queries
 * typed into the palette reach search via the no-match fallback row instead,
 * so selecting a command by name never leaks the typed text as a query.
 *
 * Order is load-bearing in one small way: the palette groups by kind while
 * preserving this array's order, so Inbox leads the navigate group and the
 * folders come out in backend slug order, with no second sort anywhere.
 */
export function useCommands(): Command[] {
  const { navigate } = useNavigation();
  const { entries } = useProjects();
  const { sessions } = useFailedSessions();

  return useMemo(() => {
    const commands: Command[] = entries.map((entry) =>
      entry.kind === "inbox"
        ? {
            id: "jump:inbox",
            title: "Inbox",
            // Inbox is a place, not a folder: it is the unrouted bucket
            // rather than a project, and it has no identity hue to wear.
            kind: "navigate" as const,
            run: () => navigate(entryView(entry)),
          }
        : {
            id: `jump:${entry.project.slug}`,
            title: formatSlug(entry.project.slug),
            kind: "folder" as const,
            hue: folderHue(entry.project.slug),
            run: () => navigate(entryView(entry)),
          },
    );

    // Conditional on the *full* listing, dismissed sessions included — a
    // deliberate divergence from the sidebar row, which counts only the
    // undismissed. When everything is dismissed the sidebar stops nagging (as
    // dismissal promises), and this jump becomes the way back to the dismissed
    // shelf; without it, dismissed captures would be unreachable and dismissal
    // would stop being reversible. A vault with no failed sessions at all still
    // drops the command: a jump to a view whose only content is "all clear"
    // wastes the one thing the palette sells.
    if (sessions.length > 0) {
      commands.push({
        id: "needs-attention",
        title: "Needs attention",
        kind: "navigate",
        run: () => navigate({ kind: "needsAttention" }),
      });
    }

    commands.push(
      {
        id: "new-note",
        kind: "action",
        title: "New note",
        run: () => navigate({ kind: "noteEditor", noteId: null, project: null }),
      },
      {
        id: "open-capture",
        kind: "action",
        title: "Quick capture",
        // The one command here with a global binding, mirroring
        // `DEFAULT_QUICK_CAPTURE_SHORTCUT` in src-tauri/src/quick_capture.rs.
        hint: "Ctrl+Alt+Space",
        // Quick capture is its own window (#45), not a main-window view — the
        // palette action pops it, matching the global hotkey.
        run: () => {
          void showQuickCaptureWindow();
        },
      },
      // The five below open a screen rather than doing something, so they are
      // `navigate` despite reading as verbs: the kind tag answers "where does
      // this take me", and "Open chat" takes you to the chat view.
      {
        id: "search",
        kind: "navigate",
        title: "Search notes",
        run: () => navigate({ kind: "search", query: "" }),
      },
      {
        id: "chat",
        kind: "navigate",
        title: "Open chat",
        run: () => navigate({ kind: "chat" }),
      },
      {
        // The terminal's only way in: it lost its sidebar row when Chat took
        // the front-door slot (the foot was four rows and read as clutter),
        // and the palette is where its power-user audience already lives.
        id: "terminal",
        kind: "navigate",
        title: "Open terminal",
        run: () => navigate({ kind: "terminal" }),
      },
      {
        // The vault-wide glossary's front door. Every project glossary is
        // reachable from its own project, but this one belongs to no folder —
        // and it is the glossary that matters most, since transcription biases
        // against it for every capture. Settings kept it alive; this is the way
        // in that does not require knowing it is in Settings.
        id: "glossary",
        kind: "navigate",
        title: "Vault glossary",
        run: () => navigate({ kind: "glossary", slug: null }),
      },
      {
        id: "settings",
        kind: "navigate",
        title: "Settings",
        run: () => navigate({ kind: "settings" }),
      },
    );

    return commands;
  }, [entries, navigate, sessions]);
}
