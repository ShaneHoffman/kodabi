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
//
// Both are `no-restricted-syntax`, hoisted to a const because the bridge-hook
// override below re-declares that rule to allow useEffect — and a blanket
// "off" there would take these with it, silently un-guarding sixteen files.

const COLOUR_LITERAL = "(#[0-9a-fA-F]{3,8}\\b|\\b(rgba?|hsla?|oklch|oklab|color-mix)\\()";
const COLOUR_MESSAGE =
  "No colour literals in className — Grove colours come from the theme (bg-ground, text-ink, " +
  "border-edge, text-kodama…) so .day and .hc can re-map them. See docs/DESIGN_SYSTEM.md §6.";

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
      // The effect ban plus the Grove guards (both defined at the top of this
      // file, so the override below can re-state the Grove half verbatim).
      "no-restricted-syntax": ["error", noEffectSelector, ...groveGuardSelectors],
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
      "no-restricted-syntax": ["error", ...groveGuardSelectors],
    },
  },
);
