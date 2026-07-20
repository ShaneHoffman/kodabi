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
 * the user's place in the page; keeping the button mounted and busy does not.
 */
export function Button({
  variant = "primary",
  type = "button",
  className = "",
  loading = false,
  loadingLabel,
  disabled,
  children,
  ...rest
}: Props) {
  const look = variant === "primary" ? "ui-btn--primary bg-surface text-text" : "ui-btn--quiet";
  const classes = [
    "ui-btn rounded-md px-xs py-2xs disabled:cursor-not-allowed disabled:text-text-faint",
    look,
    className,
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <button
      type={type}
      className={classes}
      // A busy control is not actionable, so it is genuinely disabled rather
      // than merely styled — but it stays in the DOM, and aria-busy tells a
      // screen reader why it went inert.
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      {...rest}
    >
      {loading ? (loadingLabel ?? children) : children}
    </button>
  );
}
