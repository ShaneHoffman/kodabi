import { listen } from "@tauri-apps/api/event";
import { SETTINGS_CHANGED_EVENT } from "./events";
import { getSettings, type Settings, type Theme } from "./useSettings";

// The DOM contract for the theme lives here; the type itself lives with the
// other wire types, since it is what the Rust `Theme` enum serializes to.
export type { Theme };

/**
 * Toggle `data-theme` on the <html> element (document.documentElement).
 * "system" removes the attribute so design/tokens.css falls back to
 * prefers-color-scheme (light by default, dark when the OS asks).
 * Must target documentElement, not body — tokens.css keys off :root.
 */
export function applyTheme(theme: Theme): void {
  const root = document.documentElement;
  if (theme === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", theme);
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
}
