import type { ReactNode } from "react";
// eslint-disable-next-line no-restricted-syntax -- pre-Grove; the primitives' Grove ticket deletes it
import "./ViewFrame.css";

/**
 * What kind of place this view is. Not decoration: the variant is the one
 * thing that answers "what am I looking at" before the heading is read. It
 * fixes the stance — the column cap and the body's density — so two views of
 * the same kind cannot drift apart (docs/DESIGN_SYSTEM.md §1).
 *
 * It no longer fixes the title's size. The Grove shell opens every view at one
 * step inside the panel; what distinguishes a queue from a library is what sits
 * under the head, not how loudly the head is set.
 *
 * It does NOT fix the gutter or the alignment either. Those are the same on
 * every view, on all four sides, and nothing centres — see the banner in
 * ViewFrame.css for why moving them per view failed in the running app.
 *
 *   queue   — work to get through. Its summary is a workload sentence rather
 *             than a count. Caps no column.
 *   library — a place to browse. Caps no column: its rows are rows, not prose.
 *   panel   — configuration. No column cap, so a tab rail can run the full pane
 *             (its rows cap themselves).
 *   health  — system state to recover from. A short list of pre-lifted cards.
 *             Caps no column.
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
 * and arrive as children. Those two therefore accept neither `action` nor
 * `summary` — both are type errors there rather than silent no-ops.
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
  /** The view's name, at the one step every view opens on. */
  title?: ReactNode;
  /**
   * The section landmark's accessible name, for a view whose `title` is
   * composed of elements rather than a plain string. Only that case needs it:
   * a string title already names the landmark, and passing both here would be
   * two sources for one name.
   */
  label?: string;
  children: ReactNode;
};

/** The variants that draw no header at all, so nothing that belongs *to* a
 * header can sit on them. `doc` and `search` supply their own instead. */
type HeaderlessVariant = "doc" | "search";

/**
 * The two props that only exist where there is a header to hold them.
 *
 * `action` is A SINGLE header-level action, right-aligned on the title's first
 * line: the one thing the view is for, never a container for a toolbar. Which
 * slot an action belongs in, and how many one surface may hold, is
 * *Composition* in docs/UI_CONVENTIONS.md.
 *
 * On `doc` and `search` it is a TYPE ERROR. Those two draw no header, so
 * `renderHeader`'s early return took the action down with it and reported
 * nothing — and a view that draws its own header puts its own actions in it.
 * (`eyebrow` and `title` stay legal there on purpose: they render an
 * undesigned header rather than vanishing, which is a visible mistake, not a
 * silent one.)
 *
 * `summary` is only accepted by the variants that draw one.
 *
 * The line beside the title — a workload sentence for a queue, a count for a
 * library, a state for a health view. Deliberately not a free styling slot:
 * the frame fixes its typographic role, so a count can never render at a
 * heading's weight in one view and a caption's in another. Pass the content,
 * never a class. Omit it at zero — the empty state speaks then, and two
 * "nothing here" voices in one header is one too many.
 *
 * On `panel`, `doc` and `search` it is a TYPE ERROR rather than a silent
 * no-op. The render used to guard on a per-variant class being non-empty, so
 * those three accepted the prop, dropped it, and raised nothing — a prop that
 * works on half the variants and quietly does not on the rest. The type is
 * what still says so now that the head is uniform.
 */
type Props = BaseProps &
  (
    | { variant: SummaryVariant; summary?: ReactNode; action?: ReactNode }
    | {
        variant: Exclude<Variant, SummaryVariant | HeaderlessVariant>;
        summary?: never;
        action?: ReactNode;
      }
    | { variant: HeaderlessVariant; summary?: never; action?: never }
  );

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
 * The eyebrow in particular is why this is worth a component: it is one exact
 * recipe, and when each view spelled it out by hand most of them reached for
 * Tailwind's `tracking-wide` (0.025em) while the rail used the token (0.16em)
 * — the same role rendering 6.4x apart.
 */
export function ViewFrame({
  variant,
  eyebrow,
  title,
  label,
  action,
  summary,
  children,
}: Props) {
  const header = renderHeader({ eyebrow, title, action, summary });

  return (
    // Named, so it is a real region landmark. A bare <section> has no
    // accessible name and is not exposed as one at all, which left the window
    // with a main, an aside and a nav and nothing identifying the view inside
    // them. `label` when one is given, else `title`, else `eyebrow`.
    //
    // `label` exists because `landmarkName` only names a landmark from a plain
    // string, and a view whose title is composed (ProjectView's hue dot beside
    // its name) would otherwise lose its name on the way to Grove — the same
    // failure above, arriving from the opposite direction.
    //
    // KNOWN GAP, still: `doc` and `search` pass none of the three, so
    // NoteEditorView's and SearchView's sections remain unnamed. `label` is now
    // the mechanism for closing that; doing so is those two views' own ticket,
    // since each draws its own header and has to decide what it is called.
    <section
      aria-label={label ?? landmarkName(title) ?? landmarkName(eyebrow)}
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

/**
 * One head for every view that draws one.
 *
 * The title and the summary share a BASELINE, not a stack: a view's name and
 * the size of what is in it are one statement, and stacking them made every
 * screen open like a chapter. The summary reads in the data face because that
 * is what it is — a count or a state, not a sentence — and it stays
 * `tabular-nums` so a number changing under the user doesn't shuffle the line
 * sideways.
 *
 * The per-variant title steps are gone. They existed to keep a config panel
 * from opening at a note's size, and the Grove shell answers that differently:
 * every view opens at one step inside the panel, and what tells them apart is
 * the density and shape below the head (docs/DESIGN_SYSTEM.md §1). The note
 * editor still spells its own larger step by hand, because a document genuinely
 * is the exception.
 */
function renderHeader({
  eyebrow,
  title,
  action,
  summary,
}: {
  eyebrow?: ReactNode;
  title?: ReactNode;
  action?: ReactNode;
  summary?: ReactNode;
}) {
  // No header content, no header. This used to swallow an `action` too, which
  // is why `action` is now a type error on the two variants that never pass
  // either of these.
  if (!eyebrow && !title) return null;

  return (
    <header className="flex flex-col gap-1.5">
      {eyebrow && (
        <p className="font-data text-[10px] uppercase tracking-[0.22em] text-ink-faint">
          {eyebrow}
        </p>
      )}
      <div className="flex items-baseline gap-4">
        {title && (
          <h2 className="ui-balance text-[26px] font-semibold leading-[1.15] tracking-[-0.01em] text-ink">
            {title}
          </h2>
        )}
        {summary && (
          <p className="font-data text-[11px] text-ink-dim tabular-nums">{summary}</p>
        )}
        {action && <div className="ml-auto flex-none self-center">{action}</div>}
      </div>
    </header>
  );
}
