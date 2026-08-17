import type { ReactNode } from "react";

type Props = {
  /** What kind of thing this says, which fixes both the look and the ARIA role. */
  variant: "empty" | "error" | "status";
  /** Row-level rather than view-level: steps the type down one size. */
  compact?: boolean;
  children: ReactNode;
  className?: string;
  /** For the few state blocks a test reaches for by hook rather than by role. */
  "data-testid"?: string;
};

/**
 * The one way a view says "nothing here", "that failed", or "working on it"
 * (docs/DESIGN_SYSTEM.md §3).
 *
 * Before this existed the app had three empty treatments, four error
 * treatments, and four loading treatments — and, more seriously, `role="alert"`
 * on only some of the errors, so several async failures were announced to
 * nobody. Binding the role to the variant makes that unforgettable.
 *
 * Errors read at `ink-dim`, never red: DESIGN.md does not rank with hue, and
 * weight plus the announcement carry the urgency. Grove's `--color-danger` is
 * not an opening here — it belongs to a destructive confirmation's confirm
 * control, not to reporting that something failed.
 */
export function StatusMessage({
  variant,
  compact = false,
  children,
  className = "",
  "data-testid": testId,
}: Props) {
  // An error is assertive because the user did not ask for it and may not be
  // looking; a status is polite because it accompanies something they started.
  const role = variant === "error" ? "alert" : variant === "status" ? "status" : undefined;
  // All three variants read at `ink-dim`. `status` used to sit a step fainter,
  // in the metadata register, and the promotion is the settled far side of
  // that register's line (docs/DESIGN_SYSTEM.md §6): a status is a sentence the
  // app is deliberately ANNOUNCING, often through a live region, so it is a
  // sentence the user has to read — where a hint, which stays faint, is one
  // they may. It is the last thing that should be the hardest to read.
  // Hierarchy between the three comes from the role and from `compact`, not
  // from fading one.
  //
  // The reason is register, not contrast. This comment used to cite 3.12:1 day
  // and 3.37:1 night against the 4.5:1 floor; those numbers measured a
  // pre-Grove token and no current surface produces them — today's `ink-faint`
  // clears the floor everywhere it renders (§6's table). The promotion stands
  // on what the text is, not on what it measured.
  const tone = "text-ink-dim";
  const size = compact ? "text-[12px]" : "text-[15px] leading-relaxed";

  return (
    <p
      role={role}
      data-testid={testId}
      className={`${size} ${tone}${className ? ` ${className}` : ""}`}
    >
      {children}
    </p>
  );
}
