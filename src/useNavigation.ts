import { createContext, useContext } from "react";

/**
 * The shell's destination screens. Each later ticket replaces a placeholder
 * view, not this union's shape: Inbox (#44), note editor (#46), search
 * (Phase 3). Quick capture (#45) is deliberately absent — it ships as its own
 * always-available window (`src/components/capture/QuickCapture.tsx`), driven by a
 * global hotkey and the palette's "Quick capture" action, not as a main-window
 * destination.
 */
export type View =
  | { kind: "inbox" }
  | { kind: "needsAttention" }
  | { kind: "project"; slug: string }
  | {
      kind: "noteEditor";
      noteId: string | null;
      project: string | null;
      /** Where the note was opened FROM, when the opener knew. The back link
       * prefers it and falls back to the note's filing location, so a note
       * opened from a search returns to those results rather than to the
       * folder it happens to live in. Deliberately absent from `viewKey`: it
       * is how you arrived, not which note you are on, and keying on it would
       * remount the editor for a difference the document cannot see. */
      origin?: View;
    }
  | { kind: "search"; query: string }
  | { kind: "settings" }
  | { kind: "terminal" }
  | { kind: "chat" };

/** Inbox is home: the unrouted bucket is the first thing worth seeing. */
export const INITIAL_VIEW: View = { kind: "inbox" };

/**
 * A destination's full identity as one string, for anything that has to tell
 * two views of the same kind apart.
 *
 * `view.kind` alone is not that: three of the six kinds carry a payload, so
 * "project" names every project there is. The error boundary keys on this,
 * because its fallback tells the user to pick another screen — and picking a
 * second project has to actually clear it.
 */
export function viewKey(view: View): string {
  switch (view.kind) {
    case "project":
      return `project:${view.slug}`;
    case "noteEditor":
      return `noteEditor:${view.noteId ?? "new"}:${view.project ?? ""}`;
    case "search":
      return `search:${view.query}`;
    default:
      return view.kind;
  }
}

/**
 * Call sites see only this shape, so a future history stack (back/forward)
 * swaps in behind `navigate` by touching NavigationProvider alone.
 */
type NavigationContextValue = {
  view: View;
  navigate: (view: View) => void;
};

export const NavigationContext = createContext<NavigationContextValue | undefined>(
  undefined,
);

export function useNavigation(): NavigationContextValue {
  const ctx = useContext(NavigationContext);
  if (!ctx) {
    throw new Error("useNavigation must be used within <NavigationProvider>");
  }
  return ctx;
}
