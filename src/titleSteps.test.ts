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

  it("actually reads the source tree", () => {
    // Without this, a broken walk or a too-eager exclude would make the check
    // above pass by scanning nothing at all.
    //
    // This used to count the files that still SPELL a step, ratcheting the
    // floor down as Grove landed (`DestructiveConfirmDialog` was the fifth,
    // until the primitives ticket moved it onto the Grove Dialog; `ConsentNudge`
    // and `CreateProjectDialog` were the fourth and third, until the dialogs
    // ticket did the same; `ViewFrame` was the second, until the shell ticket
    // standardized every view head on one Grove step; `NoteEditorView` was the
    // last, until this ticket). That floor has reached zero, and a floor of zero
    // cannot tell "nothing left to guard" from "the walk is broken" — which is
    // the one thing this test exists to tell apart.
    //
    // So it checks the walk instead, which is what it was always really for.
    // The guard above still forbids something real: `--fs-title-*` lives on in
    // the frozen legacy layer (design/tokens.css, bridged by src/index.css), so
    // the seven screens that have not migrated can still spell a bare size. The
    // whole file goes when that layer does.
    expect(sourceFiles(join(ROOT, "src")).length).toBeGreaterThan(50);
  });
});
