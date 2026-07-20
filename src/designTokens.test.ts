import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

/*
 * The token guard.
 *
 * `design/tokens.css` is the single source of truth for every colour, font and
 * duration in the app (CLAUDE.md). That has always been a review-enforced rule;
 * this makes it a gate. It runs in `pnpm test`, which CI already runs, and adds
 * no dependency — the companion half (numeric spacing and arbitrary values in
 * `className`) is an eslint rule, because those live in TSX rather than CSS.
 *
 * Escape hatch: put `token-guard-allow` in a comment on the offending line or
 * the line above it. It is deliberately noisy to type and greppable, so the
 * exceptions stay countable.
 */

const ROOT = process.cwd();
const TOKENS = join(ROOT, "design", "tokens.css");

/** Every CSS file that must consume tokens rather than restate values. */
function styleSheets(dir: string, found: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) styleSheets(full, found);
    else if (entry.endsWith(".css")) found.push(full);
  }
  return found;
}

/**
 * Blank out comment bodies while preserving line count and column offsets, so
 * prose can say "the reserved green" without tripping the colour check but
 * reported line numbers still point at the real line.
 */
function withoutComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, (comment) =>
    comment.replace(/[^\n]/g, " "),
  );
}

type Offence = { file: string; line: number; text: string };

/**
 * For each line, whether a `token-guard-allow` marker applies to it — either on
 * the line itself or anywhere in the comment block immediately above it.
 *
 * Checking only the single preceding line is not enough: a marker worth writing
 * usually comes with a paragraph of justification, so it lands several lines up
 * from the declaration it excuses.
 */
function allowedLines(raw: string, blanked: string): boolean[] {
  const rawLines = raw.split(/\r?\n/);
  const codeLines = blanked.split(/\r?\n/);
  const allowed = new Array<boolean>(rawLines.length).fill(false);
  // Comment text seen since the last line that carried actual code.
  let pending = "";
  rawLines.forEach((line, index) => {
    const isCode = codeLines[index].trim().length > 0;
    const marker = line.includes("token-guard-allow");
    if (isCode) {
      allowed[index] = marker || pending.includes("token-guard-allow");
      pending = "";
    } else {
      pending += line;
    }
  });
  return allowed;
}

/** `stripVars` is wrong for the font-family check, which needs to SEE the
 * `var(...)` to know the declaration is token-sourced. */
function scan(
  files: string[],
  pattern: RegExp,
  { stripVars = true }: { stripVars?: boolean } = {},
): Offence[] {
  const offences: Offence[] = [];
  for (const file of files) {
    const raw = readFileSync(file, "utf8");
    const blanked = withoutComments(raw);
    const lines = blanked.split(/\r?\n/);
    const rawLines = raw.split(/\r?\n/);
    const allowed = allowedLines(raw, blanked);
    lines.forEach((text, index) => {
      if (allowed[index]) return;
      const subject = stripVars ? text.replace(/var\(--[^)]*\)/g, "") : text;
      if (pattern.test(subject)) {
        offences.push({
          file: relative(ROOT, file),
          line: index + 1,
          text: (rawLines[index] ?? "").trim(),
        });
      }
    });
  }
  return offences;
}

const sheets = styleSheets(join(ROOT, "src")).filter((f) => f !== TOKENS);

/** The semantic tokens every theme block must map. Named explicitly rather
 * than matched by prefix, so the Layer-1 `--lift-day` / `--lift-night`
 * primitives that live alongside them aren't mistaken for semantic keys. */
const SEMANTIC = [
  "--bg",
  "--bg-sink",
  "--surface",
  "--text",
  "--text-soft",
  "--text-faint",
  "--accent",
  "--accent-dot",
  "--lift",
  "--lift-soft",
];

describe("design tokens are the single source of truth", () => {
  it("declares no literal colour outside design/tokens.css", () => {
    // Hex, the colour functions, and the common named colours. `transparent`,
    // `currentColor` and `none` are keywords rather than values and stay legal.
    const colour =
      /#[0-9a-fA-F]{3,8}\b|\b(?:rgba?|hsla?|oklch|lab)\(|(?<![\w-])(?:white|black|red|green|blue|grey|gray|yellow|orange|purple|pink|brown)(?![\w-])/;
    expect(scan(sheets, colour)).toEqual([]);
  });

  it("declares no literal duration outside design/tokens.css", () => {
    // Motion is a token family (--dur-*, --ease-*); a bare 0.2s in a component
    // is how the two surfaces that animate drifted apart before.
    const duration = /(?<![\w-])\d*\.?\d+m?s(?![\w-])/;
    expect(scan(sheets, duration)).toEqual([]);
  });

  it("declares no literal font-family outside design/tokens.css", () => {
    // The whitespace lives INSIDE the lookahead. With `\s*(?!…)` outside it,
    // `\s*` backtracks to zero width and the lookahead tests the space right
    // after the colon, which is never `var(` — so every declaration matched.
    const family = /font-family:(?!\s*(?:inherit|var\())/;
    expect(scan(sheets, family, { stripVars: false })).toEqual([]);
  });

  it("defines every pigment exactly once", () => {
    // Layer 1 is the only place a literal may appear, and each pigment is
    // defined there EXACTLY once so an edit lands in one place (tokens.css).
    const declarations = readFileSync(TOKENS, "utf8").match(/^\s*--k-[\w-]+:/gm) ?? [];
    const names = declarations.map((d) => d.trim().replace(":", ""));
    const duplicates = names.filter((name, index) => names.indexOf(name) !== index);
    expect(duplicates).toEqual([]);
    expect(names.length).toBeGreaterThan(0);
  });

  it("maps the same semantic tokens in every theme block", () => {
    // The dark mapping is stated twice (a media query cannot be merged with a
    // plain selector). Both copies only reference Layer 1, but they still have
    // to cover the same keys, or a token silently keeps its light value in one
    // dark path and not the other.
    const css = withoutComments(readFileSync(TOKENS, "utf8"));
    const blocks = [...css.matchAll(/\{([^}]*)\}/g)].map((match) => match[1]);

    const themed = blocks
      .map((block) =>
        SEMANTIC.filter((key) =>
          new RegExp(`^\\s*${key}:`, "m").test(block),
        ),
      )
      .filter((keys) => keys.length > 0);

    // Four theme blocks: default light, media dark, forced dark, forced light.
    expect(themed.length).toBe(4);
    for (const keys of themed) {
      expect(keys).toEqual(SEMANTIC);
    }
  });
});
