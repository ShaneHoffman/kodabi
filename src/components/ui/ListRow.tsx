import type { ReactNode } from "react";
import "./ListRow.css";

type Props = {
  title: ReactNode;
  /** The quiet meta line. Build it with `noteMeta` (src/noteMeta.ts). */
  meta?: ReactNode;
  /** A body preview, clamped to two lines. Forces the stacked layout. */
  snippet?: string | null;
  /** Makes the title region a button. Omit for a row that isn't openable. */
  onOpen?: () => void;
  /** A trailing control (a picker, a retry). Sits in a fixed column so the
   * controls in a list line up regardless of title length. */
  action?: ReactNode;
  /**
   * `stacked` puts meta under the title and is right when a row has a snippet
   * or a trailing control. `inline` keeps title and meta on one baseline and is
   * right for a bare list of notes.
   */
  layout?: "stacked" | "inline";
};

/**
 * One row in a list of notes or sessions (docs/DESIGN_SYSTEM.md §1).
 *
 * Three row layouts existed for the same conceptual thing, with three title
 * treatments and three separately-built meta lines. The one that mattered most
 * was hover: the Inbox put `hover:text-text` on the inner title span while the
 * focus ring sat on the outer button, so keyboard focus drew a ring with no
 * colour change and hovering the meta line lit nothing. Hover and focus belong
 * to the same element, and here they are.
 *
 * A list is not a table: no rules, no striping, no borders. Separation is space
 * and value (docs/DESIGN.md).
 */
export function ListRow({
  title,
  meta,
  snippet,
  onOpen,
  action,
  layout = "stacked",
}: Props) {
  const stacked = layout === "stacked" || !!snippet;

  const body = stacked ? (
    <>
      <span className="ui-list-row__title font-serif text-body">{title}</span>
      {meta && <span className="text-cap text-text-faint">{meta}</span>}
      {snippet && (
        <span className="ui-list-row__snippet text-cap text-text-soft">{snippet}</span>
      )}
    </>
  ) : (
    <>
      <span className="ui-list-row__title font-serif text-body">{title}</span>
      {meta && <span className="flex-none text-cap text-text-faint">{meta}</span>}
    </>
  );

  const bodyClasses = stacked
    ? "flex min-w-0 flex-1 flex-col gap-3xs rounded-md text-left text-text-soft"
    : "flex w-full items-baseline justify-between gap-md rounded-md py-2xs text-left text-text-soft";

  return (
    <div
      className={
        stacked
          ? "flex items-start justify-between gap-md py-2xs"
          : "flex items-baseline gap-md"
      }
    >
      {onOpen ? (
        <button
          type="button"
          onClick={onOpen}
          className={`ui-list-row ui-focus-ring ${bodyClasses}`}
        >
          {body}
        </button>
      ) : (
        <div className={bodyClasses}>{body}</div>
      )}
      {/* Fixed column so trailing controls align down the list rather than
          tracking each title's width. A layout dimension, not a spacing role,
          so a plain utility is correct here (docs/UI_CONVENTIONS.md). */}
      {action && <div className="w-48 flex-none">{action}</div>}
    </div>
  );
}
