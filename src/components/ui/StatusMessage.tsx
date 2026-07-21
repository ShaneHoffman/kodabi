import type { ReactNode } from "react";

type Props = {
  /** What kind of thing this says, which fixes both the look and the ARIA role. */
  variant: "empty" | "error" | "status";
  /** Row-level rather than view-level: steps the type down to text-cap. */
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
 * Errors are text-text-soft, never red: there is no red token, and DESIGN.md
 * does not rank with hue. Weight and the announcement carry the urgency.
 */
export function StatusMessage({ variant, compact = false, children, className = "" }: Props) {
  // An error is assertive because the user did not ask for it and may not be
  // looking; a status is polite because it accompanies something they started.
  const role = variant === "error" ? "alert" : variant === "status" ? "status" : undefined;
  // All three variants read at --text-soft. `status` used to sit at
  // --text-faint, which is the metadata register (3.12:1 day, 3.37:1 night —
  // under the 4.5:1 floor, docs/DESIGN_SYSTEM.md §6). A status line is a
  // sentence the app is deliberately announcing, often through a live region:
  // it is the last thing that should be the hardest to read. Hierarchy between
  // the three comes from the role and from `compact`, not from fading one.
  const tone = "text-text-soft";
  const size = compact ? "text-cap" : "text-body";

  return (
    <p role={role} className={`${size} ${tone}${className ? ` ${className}` : ""}`}>
      {children}
    </p>
  );
}
