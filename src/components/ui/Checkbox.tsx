import { useId, type InputHTMLAttributes, type ReactNode } from "react";
import "./Checkbox.css";

type Props = Omit<
  InputHTMLAttributes<HTMLInputElement>,
  "id" | "type" | "onChange"
> & {
  label: ReactNode;
  checked: boolean;
  /** Receives the new checked value, not the event — the caller almost always
   * wants the boolean. */
  onChange: (checked: boolean) => void;
  /** Optional explicit id; one is generated (useId) when omitted. */
  id?: string;
  /** Quiet helper text below the control, bound via aria-describedby. */
  hint?: ReactNode;
};

/**
 * A labelled boolean control. A real `<input type="checkbox">` under a token
 * skin (appearance: none), so keyboard, form semantics and screen-reader state
 * come from the platform rather than from re-implemented ARIA.
 *
 * The checked state reads through value, not hue: the mark is ink, never the
 * reserved green, which means "audio is being recorded" and nothing else
 * (docs/UI_CONVENTIONS.md). The box, its hairline and the focus ring live in
 * Checkbox.css; the label sits inline beside the control.
 */
export function Checkbox({
  label,
  checked,
  onChange,
  id,
  hint,
  className = "",
  ...rest
}: Props) {
  const generatedId = useId();
  const inputId = id ?? generatedId;
  const hintId = hint ? `${inputId}-hint` : undefined;

  return (
    <div className="flex flex-col gap-3xs">
      <div className="flex items-center gap-2xs">
        <input
          type="checkbox"
          id={inputId}
          checked={checked}
          aria-describedby={hintId}
          onChange={(event) => onChange(event.target.checked)}
          className={`ui-checkbox rounded-sm${className ? ` ${className}` : ""}`}
          {...rest}
        />
        <label htmlFor={inputId} className="text-cap text-text-soft">
          {label}
        </label>
      </div>
      {hint && (
        <p id={hintId} className="ui-checkbox__hint text-cap text-text-faint">
          {hint}
        </p>
      )}
    </div>
  );
}
