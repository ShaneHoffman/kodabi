import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

/*
 * The title-step guard.
 *
 * A view-title step is a TRIPLE, never a bare size: `text-title-x` travels with
 * the `leading-title-x` and `tracking-title-x` that tighten alongside it
 * (docs/DESIGN_SYSTEM.md §1). Drop either half and the title silently falls
 * back to body leading with no tracking — which renders, passes typecheck, and
 * looks merely "a bit off", so nothing but a guard catches it.
 *
 * ViewFrame's own tests assert the triple for the two variants they exercise,
 * but that leaves the other emitters — `health`, `terminal`, `chat`, and every
 * title spelled by hand (the three dialog headings, the note editor's three) —
 * covered by nothing. This is the guard for all of them at once, in the same
 * static-scan idiom as invokeParity.test.ts: one file that cannot be forgotten,
 * rather than an assertion per call site that can.
 *
 * It also covers the case those per-site assertions never could — a NEW
 * ViewFrame variant, or a new hand-spelled title, added with a bare size.
 *
 * PRE-GROVE SCOPE. The four `--fs-title-*` steps belong to the legacy token
 * layer, so this guards the screens that have not migrated yet. Grove sets a
 * view title with utilities and has no title-step triple to drop half of; when
 * the last screen moves, this file goes with design/tokens.css.
 */

const ROOT = process.cwd();

/** The four view-title steps (design/tokens.css, `--fs-title-*`). */
const STEPS = ["panel", "health", "library", "doc"] as const;

/** Every source file that can spell a class string. Test files are excluded:
 * they name a single class deliberately (`toHaveClass("text-title-library")`),
 * which is an assertion about a class, not a use of one. */
function sourceFiles(dir: string, found: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) sourceFiles(full, found);
    else if (/\.tsx?$/.test(entry) && !/\.test\.tsx?$/.test(entry)) found.push(full);
  }
  return found;
}

type Offence = { file: string; line: number; missing: string; text: string };

/**
 * One thing this deliberately cannot see: a class list assembled across more
 * than one string literal. The check is per-literal, so a title split over a
 * concatenation reports as missing its other two halves rather than being
 * quietly waved through — a conservative failure, and the reason a title's
 * classes should stay one literal.
 */
function offences(): Offence[] {
  const found: Offence[] = [];
  for (const file of sourceFiles(join(ROOT, "src"))) {
    readFileSync(file, "utf8")
      .split(/\r?\n/)
      .forEach((line, index) => {
        for (const literal of line.match(/"[^"\n]*"/g) ?? []) {
          for (const step of STEPS) {
            if (!literal.includes(`text-title-${step}`)) continue;
            const missing = [`leading-title-${step}`, `tracking-title-${step}`].filter(
              (utility) => !literal.includes(utility),
            );
            if (missing.length > 0) {
              found.push({
                file: relative(ROOT, file),
                line: index + 1,
                missing: missing.join(" + "),
                text: line.trim(),
              });
            }
          }
        }
      });
  }
  return found;
}

describe("a view-title step is a triple, not a size", () => {
  it("never spells text-title-* without its leading and tracking", () => {
    expect(offences()).toEqual([]);
  });

  it("actually looks at the files that carry the steps", () => {
    // Without this, a broken walk or a too-eager exclude would make the check
    // above pass by scanning nothing at all.
    //
    // The floor RATCHETS DOWN as Grove lands: a migrated screen sets its title
    // with Grove utilities and stops spelling the legacy triple at all
    // (`DestructiveConfirmDialog` was the fifth, until the primitives ticket
    // moved it onto the Grove Dialog). Lower it to match reality when a
    // migration takes one out; the whole file goes with the legacy layer.
    const scanned = sourceFiles(join(ROOT, "src")).filter((file) =>
      STEPS.some((step) => readFileSync(file, "utf8").includes(`text-title-${step}`)),
    );
    expect(scanned.length).toBeGreaterThanOrEqual(4);
  });
});
