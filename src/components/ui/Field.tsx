import { useId, type ComponentPropsWithRef, type ReactNode } from "react";
import { clsx } from "clsx";
import { StatusMessage } from "./StatusMessage";

type Props = Omit<ComponentPropsWithRef<"input">, "id" | "className"> & {
  label: ReactNode;
  /** Optional explicit id; one is generated (useId) when omitted. */
  id?: string;
  /** Quiet helper text below the field, bound via aria-describedby. */
  hint?: ReactNode;
  /** What is wrong with the current value. Marks the field invalid and
   * announces the message. */
  error?: string | null;
  /** Layout of the labelled block (its width, its place in a column). The
   * field's own chrome is the component's. */
  className?: string;
};

/**
 * The Grove text field: a labelled input in a glass row (docs/DESIGN_SYSTEM.md
 * §5). Same material as a button at the same radius — a translucent fill, an
 * edge, and the lit top edge every Grove surface carries — because a field and
 * the button beside it are the same physical chip at different jobs.
 *
 * The row is the bordered box, not the input: the input inside it is
 * transparent and outline-free, so `focus-within` can move the whole surface's
 * border rather than drawing a second rectangle inside the first. That is also
 * why this is the one interactive Grove control with no `focus-ring` — the
 * border step plus the kodama caret ARE the focus signal, and a ring around a
 * box that already changed would say it twice.
 *
 * `error` is a prop rather than something callers render beside the field,
 * because the message and `aria-invalid` have to travel together — every
 * hand-rolled version in the app announced one without setting the other. It
 * reads through `StatusMessage`, so a failure here is announced the same way a
 * failure anywhere else is, and in the same ink: errors are never red (Grove's
 * `--color-danger` belongs to a destructive confirmation's confirm control).
 */
export function Field({ label, id, hint, error, className, ...rest }: Props) {
  const generatedId = useId();
  const inputId = id ?? generatedId;
  const hintId = hint ? `${inputId}-hint` : undefined;
  const errorId = error ? `${inputId}-error` : undefined;

  return (
    <div className={clsx("flex flex-col gap-1.5", className)}>
      <label htmlFor={inputId} className="font-ui text-[11.5px] font-medium text-ink-dim">
        {label}
      </label>
      <div
        className={clsx(
          "flex items-center rounded-button border border-edge bg-wash px-3.5",
          "shadow-[inset_0_1px_0_var(--color-edge-lit)]",
          // Named, and one leg only: the border is the sole thing focus moves
          // here. `transition: all` on a field is how a placeholder's colour
          // ends up animating on every keystroke.
          "transition-[border-color] duration-140 ease-out-strong",
          "focus-within:border-ink-faint",
        )}
      >
        <input
          id={inputId}
          // Both are announced, and the error comes first so it is heard before
          // the hint that the value just contradicted.
          aria-describedby={[errorId, hintId].filter(Boolean).join(" ") || undefined}
          aria-invalid={error ? true : undefined}
          className={clsx(
            "w-full min-w-0 bg-transparent py-2.5",
            "font-ui text-[13.5px] text-ink caret-kodama outline-hidden",
            "placeholder:text-ink-faint",
          )}
          {...rest}
        />
      </div>
      {error && (
        <StatusMessage variant="error" compact>
          <span id={errorId}>{error}</span>
        </StatusMessage>
      )}
      {hint && (
        <p id={hintId} className="font-ui text-[11.5px] leading-relaxed text-ink-faint">
          {hint}
        </p>
      )}
    </div>
  );
}
