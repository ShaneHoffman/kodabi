import type { ComponentPropsWithRef, ReactNode } from "react";
import "./Button.css";

type Props = ComponentPropsWithRef<"button"> & {
  variant?: "primary" | "quiet";
  /** Whether the action this button started is still running. */
  loading?: boolean;
  /** What to read while `loading`; falls back to the button's own children. */
  loadingLabel?: ReactNode;
};

/**
 * The one action control — the single home for control padding
 * (px-xs py-2xs), the focus ring, and every interaction state, so screens
 * never restate them (docs/DESIGN_SYSTEM.md §2).
 *
 * It owns structure only (padding, rounding, focus, hover, active, disabled)
 * plus each variant's emphasis; it deliberately sets no text size, and `quiet`
 * sets no background either, so a caller's own `text-*` / `bg-*` utilities
 * never collide with a baked-in one (two competing `bg-*` classes resolve by
 * Tailwind's emit order, not the caller's order). `primary` is a raised
 * value plane (surface fill, hairline, medium weight); `quiet` is a ghost
 * that stays transparent via Preflight's `button { background: transparent }`
 * and inherits its colour, so a selected row can add its own `bg-surface`.
 * Hierarchy is value and type — never the reserved green (docs/DESIGN.md).
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
  const look = variant === "primary" ? "ui-btn--primary bg-surface text-text" : "ui-btn--quiet";
  const classes = [
    "ui-btn ui-focus-ring rounded-md px-xs py-2xs",
    "disabled:cursor-not-allowed disabled:text-text-faint",
    "aria-disabled:cursor-not-allowed aria-disabled:text-text-faint",
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
