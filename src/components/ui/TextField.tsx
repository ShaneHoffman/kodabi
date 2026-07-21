import { useId, type ComponentPropsWithRef, type ReactNode } from "react";
import { StatusMessage } from "./StatusMessage";
import "./TextField.css";

type Props = Omit<ComponentPropsWithRef<"input">, "id"> & {
  label: ReactNode;
  /** Optional explicit id; one is generated (useId) when omitted. */
  id?: string;
  /** Quiet helper text below the field, bound via aria-describedby. */
  hint?: ReactNode;
  /** What is wrong with the current value. Marks the field invalid and
   * announces the message. */
  error?: string | null;
};

/**
 * A labelled text input. Control padding (px-xs py-2xs), the bottom
 * hairline (an inset shadow, never a border) and the interaction states live
 * in TextField.css; the label sits an inline gap (gap-2xs) above the field
 * and is bound to it by id. Tokens only (docs/UI_CONVENTIONS.md).
 *
 * `error` is a prop rather than something callers render beside the field,
 * because the message and `aria-invalid` have to travel together — every
 * hand-rolled version in the app announced one without setting the other.
 */
export function TextField({
  label,
  id,
  hint,
  error,
  className = "",
  ...rest
}: Props) {
  const generatedId = useId();
  const inputId = id ?? generatedId;
  const hintId = hint ? `${inputId}-hint` : undefined;
  const errorId = error ? `${inputId}-error` : undefined;

  return (
    <div className="flex flex-col gap-2xs">
      <label htmlFor={inputId} className="text-cap text-text-soft">
        {label}
      </label>
      <input
        id={inputId}
        // Both are announced, and the error comes first so it is heard before
        // the hint that the value just contradicted.
        aria-describedby={[errorId, hintId].filter(Boolean).join(" ") || undefined}
        aria-invalid={error ? true : undefined}
        className={`ui-field ui-focus-ring rounded-md px-xs py-2xs text-body text-text placeholder:text-text-faint${
          className ? ` ${className}` : ""
        }`}
        {...rest}
      />
      {error && (
        <StatusMessage variant="error" compact>
          <span id={errorId}>{error}</span>
        </StatusMessage>
      )}
      {hint && (
        <p id={hintId} className="text-cap text-text-faint">
          {hint}
        </p>
      )}
    </div>
  );
}
