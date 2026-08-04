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
/** The Grove high-contrast class, which combines with `day`: `.hc.day` is the
 * high-contrast day grove. Exported for the same reason as `DAY_CLASS`: the
 * primitive gallery's toggle writes this, not a copy of it. */
export const HC_CLASS = "hc";

/** The OS request, which is the other way into high contrast. The pre-Grove
 * tokens.css answers this media query itself; Grove's `.hc` block is plain CSS
 * with no query of its own, so the OS branch has to be read here and folded
 * into the class. Module scope so the listener attaches once per window. */
const prefersMoreContrast =
  typeof window !== "undefined" && typeof window.matchMedia === "function"
    ? window.matchMedia("(prefers-contrast: more)")
    : null;
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
 * it. Must target documentElement, not body — both systems key off :root.
 *
 * Two of them, while the Grove migration runs: `data-contrast` for the
 * pre-Grove tokens.css, and the `hc` class for Grove. Only the in-app toggle is
 * persisted; the OS request is read live and OR-ed in, which is what keeps the
 * switch additive — it can add contrast, never take it away. */
export function applyContrast(more: boolean): void {
  reflectContrast(more);
  try {
    if (more) window.localStorage.setItem(STORAGE_KEY, VALUE);
    else window.localStorage.removeItem(STORAGE_KEY);
  } catch {
    // A preference we cannot persist still applies for this session.
  }
}

/** The document half of [`applyContrast`], without the write. Used wherever the
 * preference is already stored and only this window is behind — writing it back
 * would echo a `storage` event to every other window, which would write it back
 * in turn. */
function reflectContrast(more: boolean): void {
  const root = document.documentElement;
  if (more) root.setAttribute(ATTRIBUTE, VALUE);
  else root.removeAttribute(ATTRIBUTE);
  root.classList.toggle(HC_CLASS, more || (prefersMoreContrast?.matches ?? false));
}

/** Reflect the stored preference at window start, and keep both live halves of
 * the switch listening. Called from each entry module, since the quick-capture
 * and overlay windows mount no shell. */
export function startContrast(): void {
  applyContrast(readContrast());
  // The OS can turn a Contrast theme on mid-session. Re-derives from storage
  // rather than from the class, so turning the OS request back off cannot
  // clear a preference the user set in the app.
  prefersMoreContrast?.addEventListener("change", () => reflectContrast(readContrast()));
  // Settings lives in the main window, but the two capture windows are separate
  // webviews that read this once at boot. `storage` fires in every *other*
  // same-origin document when one of them writes, which is how the toggle
  // reaches them without a backend round trip. (Theme has the
  // `settings:changed` event for this; contrast has no backend field yet.)
  window.addEventListener("storage", (event) => {
    if (event.key === null || event.key === STORAGE_KEY) {
      reflectContrast(readContrast());
    }
  });
}
