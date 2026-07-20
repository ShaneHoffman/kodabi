import { useId, type ComponentPropsWithRef, type ReactNode } from "react";
import "./Textarea.css";

type Props = Omit<ComponentPropsWithRef<"textarea">, "id"> & {
  label: ReactNode;
  /** Optional explicit id; one is generated (useId) when omitted. */
  id?: string;
  /** Keep the label as the accessible name but give it no visual row. */
  hideLabel?: boolean;
  /** Quiet helper text below the field, bound via aria-describedby. */
  hint?: ReactNode;
};

/**
 * A labelled multi-line input — TextField's shape for prose.
 *
 * Two hand-rolled textareas existed (quick capture's box and the note editor's
 * body) with different padding steps and different focus treatments. Both get
 * the control padding, the writing-line hairline, and the focus ring from here.
 *
 * The body rhythm (--lh-body) is a line-height, so it lives in the co-located
 * CSS; height and resize behaviour are the caller's, since a capture box and a
 * note body want very different amounts of room.
 */
export function Textarea({
  label,
  id,
  hideLabel = false,
  hint,
  className = "",
  ...rest
}: Props) {
  const generatedId = useId();
  const fieldId = id ?? generatedId;
  const hintId = hint ? `${fieldId}-hint` : undefined;

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2xs">
      <label
        htmlFor={fieldId}
        className={hideLabel ? "sr-only" : "text-cap text-text-soft"}
      >
        {label}
      </label>
      <textarea
        id={fieldId}
        aria-describedby={hintId}
        className={`ui-textarea ui-focus-ring rounded-md bg-bg-sink px-xs py-2xs text-body text-text placeholder:text-text-faint${
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
