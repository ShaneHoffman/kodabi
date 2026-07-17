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
 * variant's emphasis; it deliberately sets no text size or colour, so a
 * caller's own `text-*` utilities never collide with a baked-in one.
 * `primary` is a raised value plane (surface fill, hairline, medium
 * weight); `quiet` is a transparent ghost that inherits its colour.
 * Hierarchy is value and type — never the reserved green (docs/DESIGN.md).
 */
export function Button({
  variant = "primary",
  type = "button",
  className = "",
  ...rest
}: Props) {
  const look =
    variant === "primary" ? "ui-btn--primary bg-surface text-text" : "bg-transparent";
  return (
    <button
      type={type}
      className={`ui-btn rounded-md px-xs py-2xs disabled:cursor-not-allowed disabled:text-text-faint ${look}${
        className ? ` ${className}` : ""
      }`}
      {...rest}
    />
  );
}
