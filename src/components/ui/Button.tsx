import type { ButtonHTMLAttributes } from "react";
import "./Button.css";

type Props = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "primary" | "quiet";
};

/**
 * The one action control — the single home for control padding
 * (px-xs py-2xs) and the focus ring, so screens never restate them.
 *
 * It owns structure only (padding, rounding, focus, disabled) plus each
 * variant's emphasis; it deliberately sets no text size, and `quiet` sets
 * no background either, so a caller's own `text-*` / `bg-*` utilities never
 * collide with a baked-in one (two competing `bg-*` classes resolve by
 * Tailwind's emit order, not the caller's order). `primary` is a raised
 * value plane (surface fill, hairline, medium weight); `quiet` is a ghost
 * that stays transparent via Preflight's `button { background: transparent }`
 * and inherits its colour, so a selected row can add its own `bg-surface`.
 * Hierarchy is value and type — never the reserved green (docs/DESIGN.md).
 */
export function Button({
  variant = "primary",
  type = "button",
  className = "",
  ...rest
}: Props) {
  const look = variant === "primary" ? "ui-btn--primary bg-surface text-text" : "";
  const classes = [
    "ui-btn rounded-md px-xs py-2xs disabled:cursor-not-allowed disabled:text-text-faint",
    look,
    className,
  ]
    .filter(Boolean)
    .join(" ");
  return <button type={type} className={classes} {...rest} />;
}
