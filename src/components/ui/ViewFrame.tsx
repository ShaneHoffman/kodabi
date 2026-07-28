import type { ReactNode } from "react";
import "./ViewFrame.css";

/**
 * What kind of place this view is. Not decoration: the variant is the one
 * thing that answers "what am I looking at" before the heading is read. It
 * fixes the title's size and the shape of the header, so two views of the
 * same kind cannot drift apart (docs/DESIGN_SYSTEM.md §1).
 *
 * It does NOT fix the gutter or the alignment. Those are the same on every
 * view, on all four sides, and nothing centres — see the banner in
 * ViewFrame.css for why moving them per view failed in the running app.
 *
 *   queue   — work to get through. A compact one-line masthead instead of a
 *             big title: a queue is not a document, and giving it a 34px
 *             serif heading made it read as one. Caps no column.
 *   library — a place to browse. The largest title in the app, and the
 *             opposite stance from `queue` in weight and density. Caps no
 *             column: its rows are rows, not prose.
 *   panel   — configuration. A small title and no column cap, so a tab rail
 *             can run the full pane (its rows cap themselves).
 *   health  — system state to recover from. A short list of pre-lifted
 *             cards under a serif title. Caps no column.
 *   doc     — a note, on the measure it was written to (--measure-doc).
 *   search  — results under a pinned query (--measure-search).
 *   terminal— the embedded Claude Code terminal. A small masthead over a
 *             full-bleed pane: its body (the xterm mount) grows to fill the
 *             height the gutter leaves, and scrolls inside itself.
 *   chat    — the designed chat over the knowledge base. The terminal's
 *             full-height stance on a document's measure (--chat-measure):
 *             the log scrolls inside itself and the composer stays put, but
 *             what fills the pane is prose, so it caps like a doc.
 *
 * `doc` and `search` render no header of their own: their headers are a
 * genuinely different shape (a back link and its own actions; a query field)
 * and arrive as children.
 */
type Variant =
  | "queue"
  | "library"
  | "panel"
  | "health"
  | "doc"
  | "search"
  | "terminal"
  | "chat";

type BaseProps = {
  /** The small uppercase label above the title. Names the section, not the field. */
  eyebrow?: ReactNode;
  /** The view's name. On `queue` it leads the one-line masthead instead. */
  title?: ReactNode;
  /** A single header-level action, right-aligned on the title's first line. */
  action?: ReactNode;
  children: ReactNode;
};

/**
 * `summary` is only accepted by the variants that draw one.
 *
 * The line under the title — a workload sentence for a queue, a count for a
 * library, a state for a health view. Deliberately not a free styling slot:
 * the variant fixes its typographic role, so a count can never render at a
 * heading's weight in one view and a caption's in another. Pass the content,
 * never a class. Omit it at zero — the empty state speaks then, and two
 * "nothing here" voices in one header is one too many.
 *
 * On `panel`, `doc` and `search` it is a TYPE ERROR rather than a silent
 * no-op. The render guarded on `SUMMARY_CLASS[variant]` being non-empty, so
 * those three accepted the prop, dropped it, and raised nothing — a prop that
 * works on half the variants and quietly does not on the rest.
 */
type Props = BaseProps &
  (
    | { variant: SummaryVariant; summary?: ReactNode }
    | { variant: Exclude<Variant, SummaryVariant>; summary?: never }
  );

/** Each variant's title step. A config panel and a note must not open at the
 * same size, which is exactly what one shared `text-h2` used to make them do.
 *
 * `ui-balance` on all three: these run to 26–34px in a serif inside a capped
 * measure, which is the exact shape that drops a single word onto a line of
 * its own. */
const TITLE_CLASS: Record<Variant, string> = {
  queue: "",
  library: "ui-balance font-serif text-title-library leading-title text-text",
  panel: "ui-balance font-serif text-title-panel leading-title text-text",
  health: "ui-balance font-serif text-title-health leading-title text-text",
  doc: "",
  search: "",
  // A tool, sized like the config panel: a small serif title, so the pane below
  // it gets the room.
  terminal: "ui-balance font-serif text-title-panel leading-title text-text",
  // Same stance as the terminal: the conversation is the content, not the
  // masthead.
  chat: "ui-balance font-serif text-title-panel leading-title text-text",
};

/** A queue states the work; a library and a health view state the size.
 *
 * `ui-tnum` because every one of these is a count that changes under the
 * user ("12 notes" → "13 notes"), rendered in the proportional interface
 * sans. The empty entries are not oversights — see `summary` on Props. */
const SUMMARY_CLASS: Record<Variant, string> = {
  queue: "",
  library: "ui-tnum text-label text-text-faint",
  panel: "",
  health: "ui-tnum text-label text-text-faint",
  doc: "",
  search: "",
  terminal: "",
  chat: "",
};

/** The variants that render `summary` at all. `panel`, `doc` and `search`
 * have no typographic role for one, so passing it there is a mistake rather
 * than a no-op — the type below is what says so, at the call site, instead of
 * the value being silently dropped at render. */
type SummaryVariant = "queue" | "library" | "health";

/**
 * The page scaffold every full view sits in: the gutter, the column, and the
 * header. The gutter and column come from ViewFrame.css, keyed off the
 * variant; the header is built here.
 *
 * The eyebrow in particular is why this is worth a component: it is exactly
 * `font-mono text-eyebrow uppercase tracking-eyebrow text-text-faint`, and
 * when each view spelled that out by hand most of them reached for Tailwind's
 * `tracking-wide` (0.025em) while the Sidebar used the token (0.16em) — the
 * same role rendering 6.4x apart.
 */
export function ViewFrame({
  variant,
  eyebrow,
  title,
  action,
  summary,
  children,
}: Props) {
  const header = renderHeader({ variant, eyebrow, title, action, summary } as Omit<
    Props,
    "children"
  >);

  return (
    // Named, so it is a real region landmark. A bare <section> has no
    // accessible name and is not exposed as one at all, which left the window
    // with a main, an aside and a nav and nothing identifying the view inside
    // them. `title` when there is one, `eyebrow` when the view draws its own
    // header instead (doc, search).
    <section
      aria-label={landmarkName(title) ?? landmarkName(eyebrow)}
      className={`view view--${variant}`}
    >
      <div className="view__column">
        {header}
        {children}
      </div>
    </section>
  );
}

/** An accessible name, but only from a node that is already a plain string.
 * A view whose title is composed of elements gets no name rather than a
 * stringified one: "[object Object]" is worse than silence. */
function landmarkName(node: ReactNode): string | undefined {
  return typeof node === "string" && node.trim() ? node : undefined;
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
            {summary && <span className="ui-tnum text-text-faint"> · {summary}</span>}
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
