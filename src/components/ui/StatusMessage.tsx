import type { ReactNode } from "react";

type Props = {
  /** What kind of thing this says, which fixes both the look and the ARIA role. */
  variant: "empty" | "error" | "status";
  /** Row-level rather than view-level: steps the type down one size. */
  compact?: boolean;
  children: ReactNode;
  className?: string;
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
export function StatusMessage({ variant, compact = false, children, className = "" }: Props) {
  // An error is assertive because the user did not ask for it and may not be
  // looking; a status is polite because it accompanies something they started.
  const role = variant === "error" ? "alert" : variant === "status" ? "status" : undefined;
  // All three variants read at `ink-dim`. `status` used to sit a step fainter,
  // in the metadata register (3.12:1 day, 3.37:1 night — under the 4.5:1
  // floor, docs/DESIGN_SYSTEM.md §6). A status line is a sentence the app is
  // deliberately announcing, often through a live region: it is the last thing
  // that should be the hardest to read. Hierarchy between the three comes from
  // the role and from `compact`, not from fading one.
  const tone = "text-ink-dim";
  const size = compact ? "text-[12px]" : "text-[15px] leading-relaxed";

  return (
    <p role={role} className={`${size} ${tone}${className ? ` ${className}` : ""}`}>
      {children}
    </p>
  );
}
