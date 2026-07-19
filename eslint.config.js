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
      "no-restricted-syntax": [
        "error",
        {
          selector:
            "CallExpression[callee.property.name=/^(useEffect|useLayoutEffect)$/]",
          message:
            "Effects live only in blessed bridge hooks — see .claude/rules/no-use-effect.md.",
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
