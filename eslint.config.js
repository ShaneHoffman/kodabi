import js from "@eslint/js";
import globals from "globals";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist", "target", "src-tauri"] },
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
      // The token guard's TSX half. `src/designTokens.test.ts` covers CSS
      // files; these cover the utility strings, which eslint can see and a CSS
      // parser cannot. Together they make "never hard-code a colour, font or
      // spacing value" (CLAUDE.md) a gate rather than a review convention.
      //
      // The numeric pattern deliberately ends with (?![\w-]) so the NAMED steps
      // that begin with a digit — gap-3xs, py-2xs, p-2xl — are not flagged;
      // only a bare `gap-4` or `px-2.5` is. Layout dimensions (w-48, max-h-80,
      // z-10) are absent from the prefix list on purpose: they are not spacing
      // roles, and docs/UI_CONVENTIONS.md allows the plain utility for them.
      "no-restricted-syntax": [
        "error",
        {
          selector:
            "CallExpression[callee.property.name=/^(useEffect|useLayoutEffect)$/]",
          message:
            "Effects live only in blessed bridge hooks — see .claude/rules/no-use-effect.md.",
        },
        {
          selector:
            'JSXAttribute[name.name="className"] Literal[value=/(^|\\s)-?(p|px|py|pt|pb|pl|pr|m|mx|my|mt|mb|ms|me|gap|gap-x|gap-y|space-x|space-y)-[0-9]+(\\.[0-9]+)?(?![\\w-])/]',
          message:
            "Use the named spacing steps (px-xs, py-2xs, gap-sm…), never the numeric utilities — docs/UI_CONVENTIONS.md.",
        },
        {
          selector:
            'JSXAttribute[name.name="className"] TemplateElement[value.raw=/(^|\\s)-?(p|px|py|pt|pb|pl|pr|m|mx|my|mt|mb|ms|me|gap|gap-x|gap-y|space-x|space-y)-[0-9]+(\\.[0-9]+)?(?![\\w-])/]',
          message:
            "Use the named spacing steps (px-xs, py-2xs, gap-sm…), never the numeric utilities — docs/UI_CONVENTIONS.md.",
        },
        {
          selector:
            'JSXAttribute[name.name="className"] Literal[value=/-\\[[^\\]]+\\]/]',
          message:
            "No arbitrary values in className (text-[13px], bg-[#fff]) — every value comes from design/tokens.css.",
        },
        {
          selector:
            'JSXAttribute[name.name="className"] TemplateElement[value.raw=/-\\[[^\\]]+\\]/]',
          message:
            "No arbitrary values in className (text-[13px], bg-[#fff]) — every value comes from design/tokens.css.",
        },
        {
          // The NoteEditorView pattern: a class string hoisted to a const, out
          // of reach of the className selectors above. It is how that screen
          // grew a parallel field system in the first place.
          selector:
            'VariableDeclarator[id.name=/(CLASS|CLASSES)$/] > Literal[value=/(^|\\s)-?(p|px|py|pt|pb|pl|pr|m|mx|my|mt|mb|ms|me|gap)-[0-9]+(\\.[0-9]+)?(?![\\w-])/]',
          message:
            "Use the named spacing steps in hoisted class strings too — docs/UI_CONVENTIONS.md.",
        },
      ],
    },
  },
  // The blessed bridge hooks — the only files that may call useEffect directly.
  // This list is the enforcement point (deliberately explicit rather than a
  // `src/use*.ts` glob, which would silently bless every new hook), and it
  // mirrors the list in .claude/rules/no-use-effect.md; keep the two in
  // lockstep. no-restricted-syntax currently holds only the effect selector —
  // if another selector is added above, re-declare it here rather than leaving
  // the blanket "off".
  {
    files: [
      "src/useCaptureState.ts",
      "src/useCommandPalette.ts",
      "src/useConsentNudge.ts",
      "src/useDebouncedValue.ts",
      "src/useDialogFocus.ts",
      "src/useDistillState.ts",
      "src/useOutsidePointerDown.ts",
      "src/useScrollIntoView.ts",
      "src/useSettings.ts",
      "src/useTauriEvent.ts",
      "src/useTimeout.ts",
      "src/useTranscriptionState.ts",
      "src/useVaultQuery.ts",
    ],
    rules: {
      "no-restricted-imports": "off",
      "no-restricted-syntax": "off",
    },
  },
);
