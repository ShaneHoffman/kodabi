import { createContext, useContext } from "react";

/**
 * The shell's destination screens. Each later ticket replaces a placeholder
 * view, not this union's shape: Inbox (#44), note editor (#46), quick
 * capture (#45 — ships as a separate window; the palette action is the
 * seam), search (Phase 3).
 */
export type View =
  | { kind: "inbox" }
  | { kind: "project"; slug: string }
  | { kind: "noteEditor"; noteId: string | null; project: string | null }
  | { kind: "search"; query: string }
  | { kind: "capture" };

/**
 * Navigation is a dispatched action rather than a bare setter so a future
 * history stack ({ type: "back" }) slots in without touching call sites.
 */
export type NavigationAction = { type: "navigate"; view: View };

/** Inbox is home: the unrouted bucket is the first thing worth seeing. */
export const INITIAL_VIEW: View = { kind: "inbox" };

export function navigationReducer(state: View, action: NavigationAction): View {
  switch (action.type) {
    case "navigate":
      return action.view;
    default:
      return state;
  }
}

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
