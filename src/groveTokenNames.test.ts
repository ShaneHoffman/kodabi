import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

/*
 * One name, one system.
 *
 * Grove's `@theme` block emits its tokens into `@layer theme`. `design/tokens.css`
 * and the per-component stylesheets are unlayered, and **unlayered declarations
 * beat every layer** — so when both systems spell a custom property the same way,
 * the legacy one wins and the Grove utility silently renders the legacy value.
 * Nothing raises a duplicate: the build succeeds, eslint is happy, and the only
 * symptom is a number that is quietly wrong.
 *
 * That is not hypothetical. `--radius-card` and `--radius-pill` were declared by
 * both, so `rounded-card` shipped at the legacy 12px rather than Grove's 14px and
 * `rounded-pill` at 11px rather than 999px — a pill that renders as a rounded
 * rectangle, which is the one shape distinction docs/DESIGN_SYSTEM.md §2 spends
 * on meaning.
 *
 * The collision is only possible while the two systems coexist, so this guard
 * goes with the legacy layer: the final Grove cleanup ticket deletes it along
 * with design/tokens.css. Until then it is the only thing standing between a new
 * Grove token and a legacy value, in the same static-scan idiom as
 * `src/titleSteps.test.ts` and `src/invokeParity.test.ts` — one file that cannot
 * be forgotten, rather than a check per token that can.
 */

const ROOT = process.cwd();
const INDEX = join(ROOT, "src", "index.css");

/** Blank comment bodies while preserving length, so prose that names a token
 * (design/tokens.css discusses `--radius-card` at length) does not read as a
 * declaration of one. */
function withoutComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, (comment) => comment.replace(/[^\n]/g, " "));
}

/** Every `.css` file the app ships apart from `src/index.css` — that is, every
 * unlayered stylesheet, which is precisely the set that can shadow a token. */
function legacyStyleSheets(dir: string, found: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) legacyStyleSheets(full, found);
    else if (entry.endsWith(".css") && full !== INDEX) found.push(full);
  }
  return found;
}

/**
 * The body of the Grove `@theme` block in `src/index.css`.
 *
 * Brace-matched rather than split on `}`, because the block nests every
 * `@keyframes` the theme owns. Anchored on `@theme {` with the brace on the same
 * line, which excludes the legacy `@theme inline {` bridge at the bottom of the
 * file — that one is `inline`, so it emits no variables at all and cannot
 * collide with anything.
 */
function groveThemeBody(): string {
  const css = withoutComments(readFileSync(INDEX, "utf8"));
  const open = css.search(/@theme\s*\{/);
  expect(open, "src/index.css declares no Grove @theme block").toBeGreaterThanOrEqual(0);
  const start = css.indexOf("{", open);
  let depth = 0;
  for (let index = start; index < css.length; index++) {
    if (css[index] === "{") depth++;
    else if (css[index] === "}" && --depth === 0) return css.slice(start + 1, index);
  }
  throw new Error("the Grove @theme block is unclosed");
}

/** The names `@theme` declares. Nested `@keyframes` bodies hold no custom
 * properties, so a flat scan of the block is enough. */
function groveTokenNames(): string[] {
  return [...groveThemeBody().matchAll(/^\s*(--[\w-]+)\s*:/gm)].map((match) => match[1]);
}

describe("Grove token names", () => {
  it("declares at least the tokens the doctrine names", () => {
    // A guard whose input silently became empty passes vacuously, and this one
    // reads the theme through a brace walk that a refactor could defeat.
    const names = groveTokenNames();
    expect(names).toContain("--color-ground");
    expect(names).toContain("--radius-pill");
    expect(names.length).toBeGreaterThan(20);
  });

  it("shares no name with an unlayered legacy stylesheet", () => {
    const names = new Set(groveTokenNames());
    const shadowed: { file: string; line: number; name: string }[] = [];

    for (const file of legacyStyleSheets(join(ROOT, "src")).concat(
      join(ROOT, "design", "tokens.css"),
    )) {
      const lines = withoutComments(readFileSync(file, "utf8")).split(/\r?\n/);
      lines.forEach((text, index) => {
        const declaration = /^\s*(--[\w-]+)\s*:/.exec(text);
        if (declaration !== null && names.has(declaration[1])) {
          shadowed.push({ file: relative(ROOT, file), line: index + 1, name: declaration[1] });
        }
      });
    }

    expect(shadowed).toEqual([]);
  });
});
