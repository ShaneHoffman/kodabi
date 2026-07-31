import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

/*
 * The token guard.
 *
 * `design/tokens.css` is the single source of truth for every colour, font,
 * duration and spacing value in the app (CLAUDE.md). That has always been a
 * review-enforced rule; this makes it a gate. It runs in `pnpm test`, which CI
 * already runs, and adds no dependency — the companion half (numeric spacing
 * and arbitrary values in `className`) is an eslint rule, because those live in
 * TSX rather than CSS. Spacing is checked on both sides: eslint owns the class
 * strings it can parse, this owns the stylesheets it cannot.
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
 * `var(...)` to know the declaration is token-sourced.
 *
 * `joinContinuations` is what a PROPERTY-SCOPED pattern needs. Token names are
 * long enough that a three-value `padding` wraps, and the continuation line
 * carries the literal with no property in front of it — invisible to a pattern
 * that anchors on `padding:`. With it on, an unterminated declaration's head is
 * prepended to the next line before matching, and a `token-guard-allow` on the
 * line the declaration STARTED on still covers it. */
function scan(
  files: string[],
  pattern: RegExp,
  {
    stripVars = true,
    joinContinuations = false,
  }: { stripVars?: boolean; joinContinuations?: boolean } = {},
): Offence[] {
  const offences: Offence[] = [];
  for (const file of files) {
    const raw = readFileSync(file, "utf8");
    const blanked = withoutComments(raw);
    const lines = blanked.split(/\r?\n/);
    const rawLines = raw.split(/\r?\n/);
    const allowed = allowedLines(raw, blanked);
    // The unterminated head of a declaration begun on an earlier line, and the
    // line it began on. Both stay empty unless `joinContinuations` is on.
    let carry = "";
    let carryStart = 0;
    lines.forEach((text, index) => {
      const subject = stripVars ? text.replace(/var\(--[^)]*\)/g, "") : text;
      const probe = carry + subject;
      const startedAt = carry === "" ? index : carryStart;
      if (joinContinuations) {
        // Anything after the line's last `;`, `{` or `}` is still open.
        const closed = Math.max(
          subject.lastIndexOf(";"),
          subject.lastIndexOf("{"),
          subject.lastIndexOf("}"),
        );
        if (closed === -1) {
          if (carry === "") carryStart = index;
          carry = probe;
        } else {
          const rest = subject.slice(closed + 1);
          carry = rest.trim() === "" ? "" : rest;
          carryStart = index;
        }
      }
      if (allowed[index] || allowed[startedAt]) return;
      if (pattern.test(probe)) {
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

/**
 * The properties whose transition is MOVEMENT rather than a change of value.
 * One list, so the two motion checks below cannot drift apart.
 *
 * `all` is here so a future `transition: all var(--dur-quick)` trips the check
 * rather than silently gating nothing.
 */
const MOVEMENT =
  "transform|translate|rotate|scale|width|height|top|right|bottom|left|inset|" +
  "grid-template-rows|grid-template-columns|all";

/**
 * Each block in `css` whose head matches `head`, with the line that head sits
 * on. Found by brace counting, because `scan()` is line-based — it cannot see a
 * block at all.
 *
 * `head` matches anywhere in the head text and the body is taken from the next
 * `{`, which makes this work for a plain selector (`/:active/`) exactly as it
 * does for an at-rule. A selector list spanning several lines is still one
 * block, and a head regex that matches twice inside one selector list still
 * yields one block, because the scan resumes past the closing brace.
 */
function blocksHeadedBy(css: string, head: RegExp): { body: string; line: number }[] {
  const blocks: { body: string; line: number }[] = [];
  const finder = new RegExp(head.source, "g");
  let match = finder.exec(css);
  while (match !== null) {
    const open = css.indexOf("{", match.index);
    if (open === -1) break;
    let depth = 0;
    let end = open;
    for (let index = open; index < css.length; index++) {
      if (css[index] === "{") depth++;
      else if (css[index] === "}" && --depth === 0) {
        end = index;
        break;
      }
    }
    blocks.push({
      body: css.slice(open + 1, end),
      line: css.slice(0, match.index).split(/\r?\n/).length,
    });
    finder.lastIndex = end;
    match = finder.exec(css);
  }
  return blocks;
}

/** The semantic tokens every theme block must map. Named explicitly rather
 * than matched by prefix, so the Layer-1 `--lift-day` / `--lift-night`
 * primitives that live alongside them aren't mistaken for semantic keys.
 *
 * The list is long because the redesign made the palette answer more
 * questions — three planes rather than two-and-a-sink, a seven-step edge
 * ladder, and one elevation recipe per plane role — and every one of those
 * has to be stated in all four theme blocks or it silently keeps its light
 * value down one of the two dark paths. That silent-drift failure is exactly
 * what this assertion exists to catch, so the list grows with the palette
 * rather than being trimmed to the interesting few. */
const SEMANTIC = [
  // the three planes
  "--bg",
  "--surface",
  "--overlay",
  // the ink ladder
  "--text",
  "--text-read",
  "--text-soft",
  "--text-faint",
  // the one reserved green, and its glow
  "--accent-dot",
  "--glow",
  "--glow-out",
  // edges — a value ladder, named by what wears them
  "--edge-faint",
  "--edge",
  "--edge-rule",
  "--edge-strong",
  "--edge-open",
  "--edge-check",
  "--edge-dot",
  // fills that are not planes
  "--tint",
  "--track",
  "--highlight",
  "--menu-hover",
  "--token-active",
  "--token-hover",
  "--toggle-on",
  "--scrim",
  // elevation — one recipe per plane role, ring included
  "--lift",
  "--lift-card",
  "--lift-row",
  "--lift-chip",
  "--lift-chip-hover",
  "--lift-chip-open",
  "--lift-menu",
  "--lift-toolbar",
  "--lift-palette",
  "--lift-capture",
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

  /*
   * The reduced-motion guards.
   *
   * Reduced motion is a token remap on `--move` (design/tokens.css), not an
   * app-wide floor: movement is gated, colour and opacity keep their real
   * timing. That split used to be prose in docs/DESIGN_SYSTEM.md §4, and it
   * failed twice — a component shipped honouring only one of the two switches,
   * and a *duration* floor was used where a resting *shape* was needed, which
   * froze the spirit-mark's aura mid-drift. These five make it a gate.
   */

  it("gates every movement transition leg on --dur-move-*", () => {
    // `stripVars` is OFF: this has to SEE the var() to know which duration the
    // leg took, the same reason the font-family check turns it off. Line-scoped
    // is enough for both real spellings — a single-line `transition: transform
    // var(--dur-plane) …` and a multi-line shorthand with one leg per line each
    // keep the property and its duration on one line. `joinContinuations` stays
    // off, since with it on a leg's property could pair with the NEXT leg's
    // duration and the check would pass on a file it should fail.
    const movementLeg = new RegExp(
      `(?<![\\w-])(?:${MOVEMENT})\\s+var\\(--dur-(?!move-)`,
    );
    expect(scan(sheets, movementLeg, { stripVars: false })).toEqual([]);
  });

  it("gates every keyframe and starting-style transform on --move", () => {
    // BLOCK-scoped, not declaration-scoped, and that is the whole trick. Four
    // transform stops in the app are correctly ungated: three that are already
    // the identity (`scale(1)` at spirit-mark-breathe's rest, `rotate(0deg)` in
    // both blob keyframes' `from`), where `calc(0 * var(--move))` would be a lie
    // about why they are still; and `capture-bar`'s `scaleY(0.3)`, which is the
    // height the bars REST at rather than a movement. A per-declaration check
    // would need four escape hatches; a per-block check needs none and still
    // catches the real failure, which is a block that moves something and never
    // mentions the switch.
    //
    // `@starting-style` is in scope alongside `@keyframes` because that is
    // where three of the app's four displacement literals live.
    //
    // What this deliberately cannot see: a displacement in a PLAIN rule, i.e.
    // `.inbox__row--vanishing`'s `translateX(calc(-36px * var(--move)))`.
    // Widening it to any literal-bearing `transform` costs five escape hatches
    // on static geometry that is not motion at all (the checkbox tick, the
    // editor toolbar's tail, an optical 1px nudge), which is a worse trade.
    const moving = /(?<![\w-])(?:transform|translate|rotate|scale)\s*:/;
    const offences: Offence[] = [];
    for (const file of sheets) {
      const raw = readFileSync(file, "utf8");
      const blanked = withoutComments(raw);
      const allowed = allowedLines(raw, blanked);
      for (const block of blocksHeadedBy(blanked, /@(?:keyframes\s+[\w-]+|starting-style)/)) {
        if (!moving.test(block.body) || block.body.includes("var(--move)")) continue;
        if (allowed[block.line - 1]) continue;
        offences.push({
          file: relative(ROOT, file),
          line: block.line,
          text: raw.split(/\r?\n/)[block.line - 1]?.trim() ?? "",
        });
      }
    }
    expect(offences).toEqual([]);
  });

  it("gates every press transform on --press-scale", () => {
    // The press is the one state change allowed to move its own box without
    // moving the layout (docs/DESIGN_SYSTEM.md §2), and without this assertion
    // it is the one that could move it UNGATED. Neither check above can see it:
    // the leg check needs
    // a `transition` naming a movement property, and a press sets
    // `transition: none` precisely so the press-down lands immediately; the
    // block check walks only @keyframes and @starting-style, and a press is a
    // plain rule. A bare `:active { transform: scale(0.97) }` would have passed
    // the entire guard while ignoring reduced motion outright.
    //
    // Rather than widen the block check to every literal-bearing `transform` —
    // which its own note rejects, at a cost of five escape hatches on static
    // geometry that is not motion — this scopes to the one selector that can
    // carry a press. The amplitude gate lives inside `--press-scale` itself, so
    // requiring the token IS requiring the gate.
    //
    // BLOCK-scoped for the same reason as the check above: `:active` sits in the
    // head, `transform` in the body, and a line-based scan cannot relate them.
    const pressing = /(?<![\w-])transform\s*:/;
    const offences: Offence[] = [];
    for (const file of sheets) {
      const raw = readFileSync(file, "utf8");
      const blanked = withoutComments(raw);
      const allowed = allowedLines(raw, blanked);
      for (const block of blocksHeadedBy(blanked, /:active/)) {
        if (!pressing.test(block.body)) continue;
        if (block.body.includes("var(--press-scale)")) continue;
        if (allowed[block.line - 1]) continue;
        offences.push({
          file: relative(ROOT, file),
          line: block.line,
          text: raw.split(/\r?\n/)[block.line - 1]?.trim() ?? "",
        });
      }
    }
    expect(offences).toEqual([]);
  });

  it("never gates an animation's duration", () => {
    // The duration gate is for `transition` only. Zeroing an ANIMATION's
    // duration is the shape of the recorded bug: it shortens the motion without
    // restoring a resting shape, so the spirit-mark's lopsided aura lobes froze
    // mid-drift. Animations are gated by amplitude, inside their own @keyframes.
    //
    // This would NOT have caught the original, which was a literal `1ms` sitting
    // behind a `token-guard-allow`. No guard catches a deliberate escape hatch —
    // that is why the hatch is noisy to type and greppable. What this does is
    // stop the same mistake being made in the token vocabulary that replaced it.
    const gatedAnimation = /animation(?:-duration)?\s*:[^;{]*var\(--dur-move-/;
    expect(scan(sheets, gatedAnimation, { stripVars: false })).toEqual([]);
  });

  it("never smooth-scrolls", () => {
    // The old app-wide floor carried `scroll-behavior: auto !important` on `*`,
    // and it gated nothing: no stylesheet asks for `smooth`, and every
    // scrollIntoView call passes only `{ block: "nearest" }`. Deleting an inert
    // `!important` was right; leaving the claim unchecked was not. If smooth
    // scrolling ever arrives, the answer is a gate at that site, and this is
    // what will force someone to think about it.
    expect(scan(sheets, /scroll-behavior\s*:\s*smooth/)).toEqual([]);
  });

  it("declares no literal spacing outside design/tokens.css", () => {
    // Padding, margin and gap only, and PROPERTY-SCOPED on purpose: sizing
    // (`width: 17px`), edges (`border: 3px`), radii and offsets
    // (`outline-offset: calc(var(--focus-offset) + 1px)`) are not spacing
    // roles, and a bare length check would fail all of them. A value on the
    // 4px ladder comes from --space-*; per-view stance is named in Layer 4.
    //
    // It runs with `joinContinuations`, because token names are long enough
    // that a three-value `padding` wraps and the property would otherwise be on
    // a different line from the literal it governs.
    //
    // One thing it deliberately cannot see: the leading lookbehind means a
    // custom property named for a spacing role (`--card-gap: 20px`) reads as a
    // declaration of its own rather than a use of one; that is what keeps the
    // `--spacing` bridge in index.css legal, and it is why Layer 4 belongs in
    // tokens.css, which this never scans.
    const spacing =
      /(?<![\w-])(?:(?:padding|margin)(?:-(?:top|right|bottom|left|inline|block)(?:-(?:start|end))?)?|(?:row|column)-gap|gap)\s*:[^;{]*\d[\d.]*(?:px|r?em)(?![\w-])/;
    expect(scan(sheets, spacing, { joinContinuations: true })).toEqual([]);
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
