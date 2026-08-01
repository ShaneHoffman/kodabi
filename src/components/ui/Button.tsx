import type { ComponentPropsWithRef, ReactNode } from "react";
// eslint-disable-next-line no-restricted-syntax -- pre-Grove; the primitives' Grove ticket deletes it
import "./Button.css";

type Props = ComponentPropsWithRef<"button"> & {
  variant?: "primary" | "quiet" | "filled" | "destructive";
  /** Whether the action this button started is still running. */
  loading?: boolean;
  /** What to read while `loading`; falls back to the button's own children. */
  loadingLabel?: ReactNode;
};

/**
 * The one action control — the single home for the focus ring and every
 * interaction state, so screens never restate them (docs/DESIGN_SYSTEM.md §2).
 *
 * It owns structure only (rounding, focus, hover, active, disabled) plus each
 * variant's emphasis; it deliberately sets no text size, so a caller's own
 * `text-*` utilities never collide with a baked-in one. Three variants, and
 * they are three weights of the same value ladder — never three hues
 * (docs/DESIGN.md):
 *
 *   primary — the raised control chip: a lighter fill, a ring and a shallow
 *             shadow, all in `--lift-chip`. Hover lifts it; it never fills
 *             darker. This is the form a settings control takes.
 *   filled  — ink fill, page-coloured label. The heaviest control in the app
 *             and the only one that inverts, spent on the single action that
 *             ends a surface ("Done", "File it") and nothing else.
 *   quiet   — a ghost that stays transparent via Preflight's
 *             `button { background: transparent }` and inherits its colour,
 *             so a selected row can add its own `bg-surface`.
 *
 * `destructive` is not a fourth weight: it wears the quiet ghost's exact
 * chrome (docs/DESIGN_SYSTEM.md §2 marks a destructive action by
 * confirmation, not colour) and exists so call sites state intent.
 * Grove added a `--color-danger`, which does NOT change that: it is spent on
 * the confirm control inside a confirmation dialog, never on the button that
 * opens one.
 * It may only ever appear inside a confirmation dialog, as the non-default
 * control beside a `primary` Cancel that holds initial focus.
 *
 * Padding follows the variant rather than the component: `primary` and
 * `filled` are real chips with a fixed size, but a `quiet` button is whatever
 * shape its context needs — a sidebar nav row, a text action beside a title,
 * a menu item — and those differ by more than a step, so each consumer sets
 * its own in its co-located CSS.
 *
 * Hover is owned here rather than by callers. It used to be bolted on at each
 * site in two different destination colours; there is one hover step, and it
 * is toward --text.
 *
 * `loading` exists so a pending action never unmounts its own control. Swapping
 * a focused button for a `<span>Saving…</span>` drops focus to <body> and strips
 * the user's place in the page.
 *
 * Keeping the button mounted is only half of that: the native `disabled`
 * attribute drops focus too. An element that is focused when it becomes
 * disabled is blurred and focus resets to <body> (the HTML focus fixup rule),
 * which is the very thing `loading` exists to prevent — and it takes the
 * surrounding keyboard context with it, since a dialog's Escape/Tab handling
 * listens on an ancestor the focus has just left. So a busy button is marked
 * `aria-disabled` instead: inert to a screen reader, still in the tab order,
 * with its activation swallowed here. `disabled` stays a genuine disable, for
 * a control that has nothing to do rather than something in flight.
 */
export function Button({
  variant = "primary",
  type = "button",
  className = "",
  loading = false,
  loadingLabel,
  disabled,
  children,
  onClick,
  ...rest
}: Props) {
  // Busy, not disabled: an explicit `disabled` wins, since a caller asking for
  // a genuinely inert control means it.
  const busy = loading && !disabled;
  // Each variant brings its own shape. `primary` is the raised control chip
  // and takes the app's control padding; `filled` and `quiet` own their
  // geometry in Button.css (the filled chip is a size of its own, and a quiet
  // ghost is whatever shape its context needs).
  const look = {
    primary: "ui-btn--primary rounded-md px-xs py-2xs bg-surface text-text",
    filled: "ui-btn--filled",
    quiet: "ui-btn--quiet",
    destructive: "ui-btn--destructive",
  }[variant];
  // The disabled LOOK lives in Button.css, not here, because it is per
  // variant: `primary` and `quiet` recede to --text-faint, but `filled` is an
  // ink fill with a page-coloured label, and fading the label against it
  // produced --text-faint on --text — about 3.6:1 in the dark theme, i.e. an
  // unreadable label on the app's most emphatic control. Only the cursor is
  // variant-independent and stays a utility.
  const classes = [
    "ui-btn ui-focus-ring",
    "disabled:cursor-not-allowed aria-disabled:cursor-not-allowed",
    look,
    className,
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <button
      type={type}
      className={classes}
      disabled={disabled}
      aria-disabled={busy || undefined}
      // aria-busy says *why* it went inert; aria-disabled says that it did.
      aria-busy={loading || undefined}
      // preventDefault, not just "skip the handler": a submit button's click is
      // what submits the form, and Enter inside a field reaches the form
      // through a synthesized click on this very button. Cancelling the click
      // stops both, matching what `disabled` used to do.
      onClick={
        busy
          ? (event) => {
              event.preventDefault();
              event.stopPropagation();
            }
          : onClick
      }
      {...rest}
    >
      {loading ? (loadingLabel ?? children) : children}
    </button>
  );
}
