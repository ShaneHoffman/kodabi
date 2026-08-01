import { listen } from "@tauri-apps/api/event";
import { SETTINGS_CHANGED_EVENT } from "./events";
import { getSettings, type Settings, type Theme } from "./useSettings";

// The DOM contract for the theme lives here; the type itself lives with the
// other wire types, since it is what the Rust `Theme` enum serializes to.
export type { Theme };

/** The Grove day class. Night is the default and carries no class, so this
 * is only ever added or removed — there is no `.night`. Exported so the
 * primitive gallery's ground toggle writes the same class this module does,
 * rather than a string that could drift from it. */
export const DAY_CLASS = "day";

/** The query the "system" choice resolves through. Held at module scope so the
 * listener below is attached exactly once per window, however many times
 * `applyTheme` runs. */
const prefersLight =
  typeof window !== "undefined" && typeof window.matchMedia === "function"
    ? window.matchMedia("(prefers-color-scheme: light)")
    : null;

/** The last theme asked for, so the OS listener knows whether it still speaks
 * for this window: it does while the choice is "system", and never otherwise. */
let currentTheme: Theme = "system";

/**
 * Reflect a theme choice on the <html> element (document.documentElement).
 *
 * Two systems are driven here, because two live side by side while the Grove
 * migration runs:
 *
 *   - `data-theme` — the pre-Grove one. "system" removes the attribute so
 *     design/tokens.css falls back to its own prefers-color-scheme query.
 *   - the `day` class — Grove's. Its tokens are plain CSS with no media query
 *     of their own, so "system" has to be *resolved* here rather than deferred.
 *     That is the real difference between the two halves, and the reason this
 *     function grew a matchMedia read.
 *
 * Must target documentElement, not body — both systems key off :root.
 */
export function applyTheme(theme: Theme): void {
  currentTheme = theme;
  const root = document.documentElement;

  if (theme === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", theme);

  // A window with no matchMedia (jsdom without a stub) can still honour an
  // explicit choice; only "system" needs the query, and unresolved reads as
  // night, which is Grove's default anyway.
  const isDay = theme === "light" || (theme === "system" && (prefersLight?.matches ?? false));
  root.classList.toggle(DAY_CLASS, isDay);
}

/**
 * Wires this webview's theme to the stored preference, for the whole life of
 * the window.
 *
 * Called imperatively from each entry module, not from a component: this is
 * one-time document-level bootstrap, which `.claude/rules/no-use-effect.md`
 * names as explicitly not an effect (the same reason `fonts.ts` is imported
 * rather than hooked). It also has to work in the quick-capture and overlay
 * windows, which mount no shell at all.
 *
 * The caller applies "system" synchronously first, so the window paints in the
 * OS theme rather than flashing a default while the read is in flight; this
 * only corrects it afterwards if the user chose otherwise. The event is what
 * keeps all three windows in step, since each is a separate webview and the
 * Settings view lives in only one of them.
 */
export function startThemeSync(): void {
  const applyFrom = (settings: Settings) => applyTheme(settings.appearance.theme);
  // A failed read is not worth a message here: the window is already painted in
  // the OS theme, which is the same thing the default would have chosen.
  void getSettings().then(applyFrom, () => {});
  void listen<Settings>(SETTINGS_CHANGED_EVENT, (event) => applyFrom(event.payload));

  // The OS half of "system". tokens.css answers the media query itself, but the
  // Grove class was resolved once in applyTheme and would otherwise be stale the
  // moment the desktop flips at sunset. Guarded on the current choice so it goes
  // quiet as soon as the user picks a side.
  prefersLight?.addEventListener("change", () => {
    if (currentTheme === "system") applyTheme("system");
  });
}
