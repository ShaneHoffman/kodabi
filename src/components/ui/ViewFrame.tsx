import type { ReactNode } from "react";
import "./ViewFrame.css";

/**
 * What kind of place this view is. Not decoration: the variant is the one
 * thing that answers "what am I looking at" before the heading is read, and
 * it fixes the gutter, the column, the alignment AND the title's size
 * together, so two views of the same kind cannot drift apart
 * (docs/DESIGN_SYSTEM.md §1).
 *
 *   queue   — work to get through. Pinned hard left, densest gutter, and a
 *             compact one-line masthead instead of a big title: a queue is
 *             not a document, and giving it a 34px serif heading made it read
 *             as one.
 *   library — a place to browse. Centred on a reading measure, the airiest
 *             gutter, the largest title. Deliberately the opposite stance
 *             from `queue`.
 *   panel   — configuration. Left-pinned, a small title, and no column cap
 *             (its rows cap themselves) so a tab rail can run the full pane.
 *   health  — system state to recover from. The narrowest column, centred,
 *             so a short list of problems does not sprawl.
 *   doc     — a note. Left-pinned on the measure it was written to.
 *   search  — results under a pinned query.
 *
 * `doc` and `search` render no header of their own: their headers are a
 * genuinely different shape (a back link and its own actions; a query field)
 * and arrive as children.
 */
type Variant = "queue" | "library" | "panel" | "health" | "doc" | "search";

type Props = {
  variant: Variant;
  /** The small uppercase label above the title. Names the section, not the field. */
  eyebrow?: ReactNode;
  /** The view's name. On `queue` it leads the one-line masthead instead. */
  title?: ReactNode;
  /** A single header-level action, right-aligned on the title's first line. */
  action?: ReactNode;
  /**
   * The line under the title — a workload sentence for a queue, a count for a
   * library, a state for a health view. Deliberately not a free styling slot:
   * the variant fixes its typographic role, so a count can never render at a
   * heading's weight in one view and a caption's in another. Pass the content,
   * never a class. Omit it at zero — the empty state speaks then, and two
   * "nothing here" voices in one header is one too many.
   */
  summary?: ReactNode;
  children: ReactNode;
};

/** Each variant's title step. A config panel and a note must not open at the
 * same size, which is exactly what one shared `text-h2` used to make them do. */
const TITLE_CLASS: Record<Variant, string> = {
  queue: "",
  library: "font-serif text-title-library leading-title text-text",
  panel: "font-serif text-title-panel leading-title text-text",
  health: "font-serif text-title-health leading-title text-text",
  doc: "",
  search: "",
};

/** A queue states the work; a library and a health view state the size. */
const SUMMARY_CLASS: Record<Variant, string> = {
  queue: "",
  library: "text-label text-text-faint",
  panel: "",
  health: "text-label text-text-faint",
  doc: "",
  search: "",
};

/**
 * The page scaffold every full view sits in: the gutter, the column, and the
 * header. The gutter and column come from ViewFrame.css, keyed off the
 * variant; the header is built here.
 *
 * The eyebrow in particular is why this is worth a component: it is exactly
 * `font-mono text-eyebrow uppercase tracking-eyebrow text-text-faint`, and
 * when each view spelled that out by hand most of them reached for Tailwind's
 * `tracking-wide` (0.025em) while the Sidebar used the token (0.22em) — the
 * same role rendering 8.8x apart.
 */
export function ViewFrame({
  variant,
  eyebrow,
  title,
  action,
  summary,
  children,
}: Props) {
  const header = renderHeader({ variant, eyebrow, title, action, summary });

  return (
    <section className={`view view--${variant}`}>
      <div className="view__column">
        {header}
        {children}
      </div>
    </section>
  );
}

function renderHeader({
  variant,
  eyebrow,
  title,
  action,
  summary,
}: Omit<Props, "children">) {
  if (!eyebrow && !title) return null;

  const eyebrowNode = eyebrow && (
    <p className="font-mono text-eyebrow uppercase tracking-eyebrow text-text-faint">
      {eyebrow}
    </p>
  );

  // A queue's masthead is one line, not a stack: the view's name and the
  // amount of work in it belong to the same sentence ("Inbox · 4 to file"),
  // and splitting them across a title and a subtitle made a short list of
  // chores look like a chapter opening.
  if (variant === "queue") {
    return (
      <header className="flex items-baseline justify-between gap-md">
        <div>
          {eyebrowNode}
          <p className="mt-2xs text-lead text-text">
            <span className="font-semibold">{title}</span>
            {summary && <span className="text-text-faint"> · {summary}</span>}
          </p>
        </div>
        {action && <div className="flex-none">{action}</div>}
      </header>
    );
  }

  return (
    <header className="flex items-start justify-between gap-md">
      <div>
        {eyebrowNode}
        {title && <h2 className={`mt-2xs ${TITLE_CLASS[variant]}`}>{title}</h2>}
        {summary && SUMMARY_CLASS[variant] && (
          <p className={`mt-2xs ${SUMMARY_CLASS[variant]}`}>{summary}</p>
        )}
      </div>
      {action && <div className="flex-none">{action}</div>}
    </header>
  );
}
