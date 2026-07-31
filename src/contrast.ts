/**
 * The user's own "more contrast" preference, on top of the OS one.
 *
 * `prefers-contrast: more` is already honoured (design/tokens.css sets
 * `--contrast: 1` from it), but that is the OS setting, and on Windows it is
 * reached only by turning on a Contrast theme — which also flips
 * `forced-colors: active` and replaces the palette wholesale. So on the platform
 * Kodabi ships to, this in-app override is not the convenience half of the
 * switch the way `reduceMotion` is: it is the branch that actually delivers the
 * high-contrast palette to someone who wants Kodabi sharper without handing
 * their whole desktop over to a Contrast theme.
 *
 * The attribute this sets is the second of the switch's two branches, and it can
 * only ever ADD contrast: there is no "off" value, so it cannot overrule an OS
 * request for more. See docs/DESIGN_SYSTEM.md §6.
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

const STORAGE_KEY = "kodabi:contrast";
const ATTRIBUTE = "data-contrast";
/** The one value the attribute and storage ever hold. `more` rather than `on`
 * because it mirrors the media feature's own vocabulary — a second word for one
 * state is how a selector and the code that sets it drift apart. */
const VALUE = "more";

/** Whether the user has asked us to sharpen. Reads storage directly, so a
 * component can seed state from it during render without an effect. */
export function readContrast(): boolean {
  try {
    return window.localStorage.getItem(STORAGE_KEY) === VALUE;
  } catch {
    // Storage can throw in a locked-down webview. A preference we cannot read
    // is a preference the user has not expressed.
    return false;
  }
}

/** Set the preference and reflect it on <html>, where the token remap keys off
 * it. Must target documentElement, not body — tokens.css keys off :root. */
export function applyContrast(more: boolean): void {
  const root = document.documentElement;
  if (more) root.setAttribute(ATTRIBUTE, VALUE);
  else root.removeAttribute(ATTRIBUTE);
  try {
    if (more) window.localStorage.setItem(STORAGE_KEY, VALUE);
    else window.localStorage.removeItem(STORAGE_KEY);
  } catch {
    // A preference we cannot persist still applies for this session.
  }
}

/** Reflect the stored preference at window start. Called from each entry
 * module, since the quick-capture and overlay windows mount no shell. */
export function startContrast(): void {
  applyContrast(readContrast());
}
