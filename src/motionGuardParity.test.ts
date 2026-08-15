import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

/*
 * The reduced-motion swap guard's lockstep.
 *
 * `eslint.config.js` holds a list of the transform-bearing animations, and
 * DESIGN_SYSTEM §4 holds the table that same list is derived from. Neither can
 * see the other, so a new moving animation added to the doctrine — or a row
 * retired from it — would leave the guard checking yesterday's set, silently
 * and forever. This asserts the two are the same set, in the same shape as the
 * invoke-string guard next door: it runs in `pnpm test`, which CI already runs,
 * and adds no dependency.
 *
 * It pins the LIST, not the swap. Which partner is the right one, and whether
 * the partner survives specificity once it is written, are still review's job
 * (`.claude/rules/design-consistency.md`).
 */

const ROOT = process.cwd();
const ESLINT_CONFIG = join(ROOT, "eslint.config.js");
const DESIGN_SYSTEM = join(ROOT, "docs", "DESIGN_SYSTEM.md");

/** The tokens the eslint guard treats as needing a `motion-reduce:` partner. */
function guardedTokens(): string[] {
  const source = readFileSync(ESLINT_CONFIG, "utf8");
  const list = /const MOTION_TOKENS = \[([^\]]*)\]/.exec(source);
  if (!list) {
    throw new Error("no MOTION_TOKENS array found in eslint.config.js");
  }
  return [...list[1].matchAll(/"([a-z-]+)"/g)].map((entry) => entry[1]);
}

/**
 * The moving animations in §4's swap table — the "Normal" column of every row
 * that names an `animate-*` token.
 *
 * The table's other two rows (`active:scale-97` and a switch knob's
 * `translate`) are deliberately skipped: neither is an animation utility, so
 * neither is something a className guard can look for.
 */
function tabulatedTokens(): string[] {
  const source = readFileSync(DESIGN_SYSTEM, "utf8");
  const section = /### Reduced motion keeps opacity life([^]*?)\n---/.exec(source);
  if (!section) {
    throw new Error(
      "no '### Reduced motion keeps opacity life' section found in docs/DESIGN_SYSTEM.md",
    );
  }
  const tokens: string[] = [];
  for (const row of section[1].split(/\r?\n/)) {
    if (!row.startsWith("|")) continue;
    const normal = row.split("|")[1] ?? "";
    for (const match of normal.matchAll(/`animate-([a-z][a-z-]*)`/g)) {
      tokens.push(match[1]);
    }
  }
  return tokens;
}

describe("reduced-motion guard parity", () => {
  it("guards exactly the animations §4's swap table calls moving", () => {
    const tabulated = tabulatedTokens();
    const guarded = guardedTokens();

    expect(
      guarded.filter((token) => !tabulated.includes(token)),
      "eslint.config.js guards these animate-* tokens, but DESIGN_SYSTEM §4's swap table " +
        "does not list them as moving — either the table lost a row or the guard gained one",
    ).toEqual([]);

    expect(
      tabulated.filter((token) => !guarded.includes(token)),
      "DESIGN_SYSTEM §4's swap table lists these animate-* tokens as moving, but " +
        "eslint.config.js does not guard them — a call site could ship one with no " +
        "motion-reduce: partner and nothing would complain",
    ).toEqual([]);
  });

  it("finds the token list and the swap table at all", () => {
    // A guard on the guard: if either extraction silently stopped matching, the
    // test above would pass vacuously forever.
    expect(guardedTokens().length).toBeGreaterThan(5);
    expect(tabulatedTokens().length).toBeGreaterThan(5);
  });
});
