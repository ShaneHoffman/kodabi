export type Theme = "light" | "dark" | "system";

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
