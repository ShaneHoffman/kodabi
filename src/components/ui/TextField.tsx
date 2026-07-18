import { useId, type InputHTMLAttributes, type ReactNode } from "react";
import "./TextField.css";

type Props = Omit<InputHTMLAttributes<HTMLInputElement>, "id"> & {
  label: ReactNode;
  /** Optional explicit id; one is generated (useId) when omitted. */
  id?: string;
  /** Quiet helper text below the field, bound via aria-describedby. */
  hint?: ReactNode;
};

/**
 * A labelled text input. Control padding (px-xs py-2xs), the bottom
 * hairline (an inset shadow, never a border) and the focus ring live in
 * TextField.css; the label sits an inline gap (gap-2xs) above the field
 * and is bound to it by id. Tokens only (docs/UI_CONVENTIONS.md).
 */
export function TextField({ label, id, hint, className = "", ...rest }: Props) {
  const generatedId = useId();
  const inputId = id ?? generatedId;
  const hintId = hint ? `${inputId}-hint` : undefined;

  return (
    <div className="flex flex-col gap-2xs">
      <label htmlFor={inputId} className="text-cap text-text-soft">
        {label}
      </label>
      <input
        id={inputId}
        aria-describedby={hintId}
        className={`ui-field rounded-md bg-surface px-xs py-2xs text-body text-text placeholder:text-text-faint${
          className ? ` ${className}` : ""
        }`}
        {...rest}
      />
      {hint && (
        <p id={hintId} className="text-cap text-text-faint">
          {hint}
        </p>
      )}
    </div>
  );
}
