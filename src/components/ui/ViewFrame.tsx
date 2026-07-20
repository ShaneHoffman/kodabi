import type { ReactNode } from "react";

type Props = {
  /** The small uppercase label above the title. Names the section, not the field. */
  eyebrow?: ReactNode;
  /** Omit both this and `eyebrow` to get the bare scaffold — for a view whose
   * header is genuinely a different shape (the note editor's carries a back
   * link and its own actions) and supplies its own as a child. */
  title?: ReactNode;
  /** A single header-level action, right-aligned on the title's baseline. */
  action?: ReactNode;
  children: ReactNode;
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
export function ViewFrame({ eyebrow, title, action, children }: Props) {
  return (
    <section className="flex min-h-full flex-col p-xl">
      <div className="mx-auto flex w-full max-w-content flex-col gap-lg">
        {title && (
          <header className="flex items-baseline justify-between gap-md">
            <div className="flex flex-col gap-3xs">
              {eyebrow && (
                <p className="text-eyebrow uppercase tracking-eyebrow text-text-faint">
                  {eyebrow}
                </p>
              )}
              <h2 className="font-serif text-h2 text-text">{title}</h2>
            </div>
            {action && <div className="flex-none">{action}</div>}
          </header>
        )}
        {children}
      </div>
    </section>
  );
}
