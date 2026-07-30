/**
 * The user's own "reduce motion" preference, on top of the OS one.
 *
 * `prefers-reduced-motion` is already honoured (design/tokens.css sets `--move:
 * 0` from it), but that is the OS setting and Windows buries it three levels
 * into Accessibility. This is the in-app override: someone who wants the app to
 * stop sliding and growing things should be able to say so here, without
 * changing a system-wide preference for every other app they own.
 *
 * The attribute this sets is the second of the switch's two branches, and it can
 * only ever ADD reduction: there is no "off" value, so it cannot overrule an OS
 * request for reduced motion. See docs/DESIGN_SYSTEM.md §4.
 *
 * It lives in localStorage rather than the settings store because it is a
 * per-device display preference with no backend field yet — the same class of
 * thing as the theme, which the backend *does* store, so this is the one to
 * move into `AppearanceSettings` when that field lands. Until then localStorage
 * keeps it surviving a restart, which is the part that actually matters.
 *
 * Applied imperatively (entry modules at boot, the Settings toggle on click)
 * rather than from an effect: this is one-time document-level bootstrap plus an
 * event handler, which `.claude/rules/no-use-effect.md` names as explicitly not
 * effects.
 */

const STORAGE_KEY = "kodabi:reduce-motion";
const ATTRIBUTE = "data-reduce-motion";

/** Whether the user has asked us to hold still. Reads storage directly, so a
 * component can seed state from it during render without an effect. */
export function readReduceMotion(): boolean {
  try {
    return window.localStorage.getItem(STORAGE_KEY) === "on";
  } catch {
    // Storage can throw in a locked-down webview. A preference we cannot read
    // is a preference the user has not expressed.
    return false;
  }
}

/** Set the preference and reflect it on <html>, where the CSS floor keys off
 * it. Must target documentElement, not body — index.css keys off :root. */
export function applyReduceMotion(reduce: boolean): void {
  const root = document.documentElement;
  if (reduce) root.setAttribute(ATTRIBUTE, "on");
  else root.removeAttribute(ATTRIBUTE);
  try {
    if (reduce) window.localStorage.setItem(STORAGE_KEY, "on");
    else window.localStorage.removeItem(STORAGE_KEY);
  } catch {
    // A preference we cannot persist still applies for this session.
  }
}

/** Reflect the stored preference at window start. Called from each entry
 * module, since the quick-capture and overlay windows mount no shell. */
export function startReduceMotion(): void {
  applyReduceMotion(readReduceMotion());
}
