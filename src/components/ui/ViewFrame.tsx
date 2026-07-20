import type { ReactNode } from "react";

/**
 * What kind of view this is. Not decoration: each variant answers "what am I
 * looking at" before the heading is read, which is the whole point of having
 * them (docs/DESIGN_SYSTEM.md §1).
 *
 *   queue   — work to get through. Carries a workload sentence.
 *   library — a place to browse. Carries a quiet count.
 *   panel   — configuration. Narrower column, h3-led sections, no summary.
 *
 * Omitted is a real fourth answer: the bare scaffold, for a view that supplies
 * its own header shape (the note editor) or has nothing to summarise yet (the
 * Search placeholder).
 */
type Variant = "queue" | "library" | "panel";

type Props = {
  /** The small uppercase label above the title. Names the section, not the field. */
  eyebrow?: ReactNode;
  /** Omit both this and `eyebrow` to get the bare scaffold — for a view whose
   * header is genuinely a different shape (the note editor's carries a back
   * link and its own actions) and supplies its own as a child. */
  title?: ReactNode;
  /** A single header-level action, right-aligned on the title's baseline. */
  action?: ReactNode;
  variant?: Variant;
  /**
   * The line under the title. Deliberately not a free slot: `variant` fixes its
   * typographic role, so a workload sentence can never render at a count's
   * weight in one view and a heading's in another. Pass the content, not the
   * styling. Omit it when the count is zero — the empty state speaks then, and
   * two "nothing here" voices in one header is one too many.
   */
  summary?: ReactNode;
  children: ReactNode;
};

/** A queue states the work; a library states the size. Both recede from the title. */
const SUMMARY_CLASS: Record<Variant, string> = {
  queue: "text-body text-text-soft",
  library: "text-cap text-text-faint",
  panel: "",
};

/**
 * The page scaffold every full view sits in: the gutter, the centred content
 * column, the section rhythm, and the eyebrow/title header
 * (docs/DESIGN_SYSTEM.md §1).
 *
 * This was repeated inline in four views and extracted locally as `Frame` in a
 * fifth. The eyebrow in particular is why it is worth a component: it is
 * exactly `text-eyebrow uppercase tracking-eyebrow text-text-faint`, and when
 * each view spelled that out by hand most of them reached for Tailwind's
 * `tracking-wide` (0.025em) while the Sidebar used the `--ls-eyebrow` token
 * (0.22em) — the same role rendering 8.8x apart.
 */
export function ViewFrame({
  eyebrow,
  title,
  action,
  variant,
  summary,
  children,
}: Props) {
  // Config reads narrow: a settings column that runs the full content width
  // makes every label and control drift apart from the thing it labels.
  const column =
    variant === "panel"
      ? "mx-auto flex w-full max-w-measure flex-col gap-lg"
      : "mx-auto flex w-full max-w-content flex-col gap-lg";
  const summaryClass = variant ? SUMMARY_CLASS[variant] : "";

  return (
    // p-lg, not p-xl. 64px on every edge spent a tenth of a 640px-tall window
    // on nothing before the eyebrow even started, and pushed the first row of
    // every list past 200px down. Ma is space that does work; this was space
    // that just cost.
    <section className="flex min-h-full flex-col p-lg">
      <div className={column}>
        {title && (
          <header className="flex items-baseline justify-between gap-md">
            <div className="flex flex-col gap-3xs">
              {eyebrow && (
                <p className="text-eyebrow uppercase tracking-eyebrow text-text-faint">
                  {eyebrow}
                </p>
              )}
              <h2 className="font-serif text-h2 text-text">{title}</h2>
              {summary && summaryClass && (
                <p className={summaryClass}>{summary}</p>
              )}
            </div>
            {action && <div className="flex-none">{action}</div>}
          </header>
        )}
        {children}
      </div>
    </section>
  );
}
