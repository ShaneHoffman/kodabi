import type { KeyboardEvent as ReactKeyboardEvent } from "react";

/**
 * Focusable descendants for a dialog's Tab-wrap trap.
 *
 * Every focusable element kind is listed even when a given dialog renders only
 * a few of them: the trap works by finding the FIRST and LAST match, so a
 * control this selector cannot see is not merely skipped — it sits outside the
 * wrap entirely and Tab escapes the modal at it. That is a hole that opens
 * silently, the first time someone adds a field to a dialog.
 *
 * Nothing inert is excluded by attribute either. `:not([disabled])` used to be
 * the whole guard, which was right when a busy control took the native
 * attribute; now that a write in flight is `aria-disabled` (it has to stay
 * focusable — docs/DESIGN_SYSTEM.md §6), a busy control is still a real tab
 * stop and still belongs in the wrap.
 */
const FOCUSABLE = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "textarea:not([disabled])",
  "select:not([disabled])",
  "[contenteditable]:not([contenteditable='false'])",
  '[tabindex]:not([tabindex="-1"])',
].join(", ");

/**
 * Wraps a Tab keydown across a dialog panel's focusable descendants: Tab on
 * the last control focuses the first, Shift+Tab on the first focuses the last.
 * Call from the panel's `onKeyDown` for `event.key === "Tab"`; pair with
 * `useDialogFocus` for the open/close hand-off. Extracted from the identical
 * traps ConsentNudge and the project dialogs each hand-rolled.
 */
export function wrapDialogTab(
  event: ReactKeyboardEvent<HTMLDivElement>,
  panel: HTMLElement | null,
): void {
  const focusables = panel?.querySelectorAll<HTMLElement>(FOCUSABLE);
  if (!focusables || focusables.length === 0) return;
  const first = focusables[0];
  const last = focusables[focusables.length - 1];
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first.focus();
  }
}
