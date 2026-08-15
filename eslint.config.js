import js from "@eslint/js";
import globals from "globals";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import tseslint from "typescript-eslint";

// The Grove guards.
//
// They replaced the pre-Grove token guard (a test that scanned stylesheets,
// plus eslint rules banning numeric spacing utilities and arbitrary values).
// Grove is styled with Tailwind utilities, so the numeric grid and arbitrary
// values are now the sanctioned spelling and both of those bans are gone. What
// is left worth enforcing is narrower and sharper:
//
//   1. Colour comes from the theme. A hex in a className is a value that no
//      theme block can re-map, which means it survives .day and .hc unchanged
//      and quietly breaks both variants. This is the one rule where a literal
//      is not merely untidy but wrong.
//   2. Styles live in src/index.css. It is now the app's only stylesheet, so a
//      NEW one has to be an argued exception rather than a quiet reflex.
//   3. A movement carries its reduced-motion partner. DESIGN_SYSTEM §4 swaps a
//      transform-bearing animation for an opacity-only one at the call site,
//      and the failure is invisible on screen to anyone not running with
//      reduced motion on — which is how `animate-breathe` reached a placeholder
//      dot and stayed there. The doc's swap table is the token list, and
//      src/motionGuardParity.test.ts keeps the two in lockstep.
//
// All three are `no-restricted-syntax`, hoisted to a const because the
// bridge-hook override below re-declares that rule to allow useEffect — and a
// blanket "off" there would take these with it, silently un-guarding sixteen
// files.

const COLOUR_LITERAL = "(#[0-9a-fA-F]{3,8}\\b|\\b(rgba?|hsla?|oklch|oklab|color-mix)\\()";
const COLOUR_MESSAGE =
  "No colour literals in className — Grove colours come from the theme (bg-ground, text-ink, " +
  "border-edge, text-kodama…) so .day and .hc can re-map them. See docs/DESIGN_SYSTEM.md §7.";

// The transform-bearing half of DESIGN_SYSTEM §4's swap table — every animation
// whose reduced-motion form is a different animation (or none at all). The
// opacity-only tokens are absent on purpose: fade-in, fade-out, halo-still,
// caret and pending are already what a swap swaps *to*.
//
// src/motionGuardParity.test.ts asserts this list equals the doc's table, in
// both directions, so a row added to §4 without a guard fails `pnpm test`.
const MOTION_TOKENS = [
  "materialize",
  "rise-in",
  "dissolve",
  "halo",
  "ring",
  "breathe",
  "drift",
  "drift-back",
];
// Matches both spellings a call site may use: the utility (`animate-dissolve`)
// and the arbitrary value that names the same keyframe with its own timing
// (`animate-[dissolve_110ms_ease_forwards]`), hence the optional `[`. The
// trailing lookahead is what keeps `animate-halo-still` — an opacity-only
// partner — from reading as `halo`: a letter or a hyphen after the token means
// it is a different token, while `_` and `]` are the arbitrary-value delimiters
// and must still match.
const MOTION_LITERAL = `animate-\\[?(?:${MOTION_TOKENS.join("|")})(?![a-zA-Z-])`;
const MOTION_MESSAGE =
  "A transform-bearing animate-* class needs its motion-reduce: partner in the same class string — " +
  "the swap table is docs/DESIGN_SYSTEM.md §4 (materialize/rise-in → fade-in, dissolve → fade-out, " +
  "halo/ring → halo-still, breathe/drift/drift-back → none). Writing the swap at the call site is " +
  "what makes it visible in review.";

const groveGuardSelectors = [
  {
    selector: `JSXAttribute[name.name="className"] Literal[value=/${COLOUR_LITERAL}/]`,
    message: COLOUR_MESSAGE,
  },
  {
    selector: `JSXAttribute[name.name="className"] TemplateElement[value.raw=/${COLOUR_LITERAL}/]`,
    message: COLOUR_MESSAGE,
  },
  {
    // A class string hoisted to a const, out of reach of the className
    // selectors above. That pattern is how NoteEditorView grew a parallel
    // field system under the old guard; it stays covered under this one.
    selector: `VariableDeclarator[id.name=/(CLASS|CLASSES)$/] > Literal[value=/${COLOUR_LITERAL}/]`,
    message: COLOUR_MESSAGE,
  },
  {
    // Grove has one stylesheet, and there are no exceptions left in the app's
    // own code — the last pre-Grove sheet went with the legacy layer. The only
    // surviving disable is a third-party import (xterm's), which is the shape
    // a future one has to argue itself into.
    selector:
      'ImportDeclaration[source.value=/\\.css$/]:not([source.value="./index.css"])',
    message:
      "Styles are Tailwind utilities in the component, and the one stylesheet is src/index.css. " +
      "A .css import needs an eslint-disable comment justifying it — see docs/UI_CONVENTIONS.md.",
  },
  // The reduced-motion swap, in the four shapes a class string is written in.
  // The contract each selector enforces is that the pair is CO-LOCATED: the
  // partner lives in the same string literal as the thing it swaps, which is
  // how Dialog and Menu already spell it. That is the only version a static
  // check can hold, and it is the version the doctrine asks for anyway.
  {
    selector: `JSXAttribute[name.name="className"] Literal[value=/${MOTION_LITERAL}/]:not([value=/motion-reduce:/])`,
    message: MOTION_MESSAGE,
  },
  {
    selector: `JSXAttribute[name.name="className"] TemplateElement[value.raw=/${MOTION_LITERAL}/]:not([value.raw=/motion-reduce:/])`,
    message: MOTION_MESSAGE,
  },
  {
    selector: `VariableDeclarator[id.name=/(CLASS|CLASSES)$/] > Literal[value=/${MOTION_LITERAL}/]:not([value=/motion-reduce:/])`,
    message: MOTION_MESSAGE,
  },
  {
    // The hole the three above leave, and the one the app actually writes in:
    // a class string hoisted into a clsx()/cva() const (PALETTE_SURFACE, the
    // variant maps) is neither inside a className attribute nor named *CLASS,
    // so nothing else here can see it. This is the selector that catches a
    // pair split across two adjacent array elements.
    selector: `CallExpression[callee.name=/^(clsx|cva)$/] Literal[value=/${MOTION_LITERAL}/]:not([value=/motion-reduce:/])`,
    message: MOTION_MESSAGE,
  },
];

// The copy guard, separate from the Grove guards because its scope is
// different: `.claude/rules/copy-style.md` is about language the user READS, so
// the test harness and the dev-only gallery drop it (see the override at the
// bottom) while keeping every Grove guard. Comments are out of reach by
// construction — `no-restricted-syntax` matches AST nodes, and a comment is not
// one — which is exactly the line copy-style already draws.
const COPY_MESSAGE =
  "No em dashes in copy the user reads — rewrite with a period, comma, colon, or parentheses. " +
  "See .claude/rules/copy-style.md.";

const copyGuardSelectors = [
  { selector: "JSXText[value=/—/]", message: COPY_MESSAGE },
  { selector: "Literal[value=/—/]", message: COPY_MESSAGE },
  { selector: "TemplateElement[value.raw=/—/]", message: COPY_MESSAGE },
];

const noEffectSelector = {
  selector: "CallExpression[callee.property.name=/^(useEffect|useLayoutEffect)$/]",
  message:
    "Effects live only in blessed bridge hooks — see .claude/rules/no-use-effect.md.",
};

export default tseslint.config(
  { ignores: ["dist", "target", "src-tauri"] },
  // The end-to-end harness and the dev scripts that share its libraries
  // (`scripts/dev-sandbox.mjs` imports `e2e/lib/vault.mjs`): plain Node ESM,
  // deliberately outside the `**/*.{ts,tsx}` block below (neither is
  // typechecked by `tsc -b`, whose project covers `src` only). Linted rather
  // than left alone so they do not become unlinted islands next to
  // preview-mock.js — a file matching no `files` block here is linted with
  // zero rules, which reads as passing.
  {
    files: ["e2e/**/*.mjs", "scripts/**/*.mjs"],
    extends: [js.configs.recommended],
    languageOptions: { ecmaVersion: 2022, globals: globals.node },
  },
  {
    files: ["**/*.{ts,tsx}"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    languageOptions: { ecmaVersion: 2020, globals: globals.browser },
    plugins: { "react-hooks": reactHooks, "react-refresh": reactRefresh },
    rules: {
      ...reactHooks.configs.recommended.rules,
      "react-refresh/only-export-components": ["warn", { allowConstantExport: true }],
      // Effects belong in blessed bridge hooks, never in components or feature
      // logic (.claude/rules/no-use-effect.md). The import ban catches the
      // named import; the syntax ban closes the namespace hole
      // (`import * as React from "react"; React.useEffect(...)`).
      "no-restricted-imports": [
        "error",
        {
          paths: [
            {
              name: "react",
              importNames: ["useEffect", "useLayoutEffect"],
              message:
                "Effects live only in blessed bridge hooks — see .claude/rules/no-use-effect.md.",
            },
          ],
        },
      ],
      // The effect ban plus the Grove guards plus the copy guard (all defined
      // at the top of this file, so the overrides below can re-state whichever
      // half stays on, verbatim).
      "no-restricted-syntax": [
        "error",
        noEffectSelector,
        ...groveGuardSelectors,
        ...copyGuardSelectors,
      ],
    },
  },
  // The blessed bridge hooks — the only files that may call useEffect directly.
  // This list is the enforcement point (deliberately explicit rather than a
  // `src/use*.ts` glob, which would silently bless every new hook), and it
  // mirrors the list in .claude/rules/no-use-effect.md; keep the two in
  // lockstep.
  //
  // These files are exempt from the EFFECT selector only. `no-restricted-syntax`
  // is a single rule, so the exemption has to re-declare everything that stays
  // on — a blanket "off" here would take the Grove guards down with it in every
  // file listed below, silently and without a diff to notice.
  {
    files: [
      "src/useCaptureState.ts",
      "src/useChatSession.ts",
      "src/useCommandPalette.ts",
      "src/useConsentNudge.ts",
      "src/useDebouncedValue.ts",
      "src/useDistillState.ts",
      "src/useElapsed.ts",
      "src/useModelDownload.ts",
      "src/useOutsidePointerDown.ts",
      "src/useRoutePreview.ts",
      "src/useScrollIntoView.ts",
      "src/useSettings.ts",
      "src/useTauriEvent.ts",
      "src/useTimeout.ts",
      "src/useTranscriptionState.ts",
      "src/useUpdater.ts",
      "src/useVaultQuery.ts",
      "src/useWindowMaximized.ts",
      "src/useXterm.ts",
    ],
    rules: {
      "no-restricted-imports": "off",
      "no-restricted-syntax": ["error", ...groveGuardSelectors, ...copyGuardSelectors],
    },
  },
  // The test harness and the dev-only gallery — exempt from the COPY guard
  // only. `.claude/rules/copy-style.md` scopes itself to language the user
  // reads; a describe() title, an assertion message, and the gallery's
  // annotations beside each primitive are none of those, and the gallery ships
  // in no build (gallery.html is deliberately absent from
  // vite.config.ts's rollupOptions.input).
  //
  // Same re-statement discipline as the bridge-hook block above, for the same
  // reason: `no-restricted-syntax` is one rule, so lifting part of it means
  // naming everything that stays — a blanket "off" here would quietly take the
  // effect ban and every Grove guard down with it. The motion guard in
  // particular stays on: a test or a gallery entry that animates is still
  // making a claim about reduced motion.
  {
    files: [
      "src/**/*.test.{ts,tsx}",
      "src/test/**",
      "src/gallery.tsx",
      "src/components/dev/**",
    ],
    rules: {
      "no-restricted-syntax": ["error", noEffectSelector, ...groveGuardSelectors],
    },
  },
);
