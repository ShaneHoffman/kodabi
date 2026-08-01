---
paths:
  - src/**
---

# TypeScript style

The frontend is TypeScript strict, and the strictness is a gate, not a suggestion.

- **Strict mode is on and enforced.** `tsconfig.json` sets `strict: true` (plus
  `noUnusedLocals`, `noUnusedParameters`, `noFallthroughCasesInSwitch`), and
  `pnpm build` runs `tsc -b` before Vite — so a type error fails the build gate.
  Don't loosen the config to get code through.
- **No `any`.** `eslint.config.js` runs typescript-eslint's recommended set at
  `--max-warnings=0`, so `@typescript-eslint/no-explicit-any` fails the lint gate.
  Reach for `unknown` plus narrowing, a precise type, or a generic constraint. Don't
  paper over a type with `@ts-expect-error` or an `eslint-disable` unless you add an
  inline comment justifying it.
- **Descriptive names.** `currentProject`, not `cp`; `previousValue`, not `prev`.
- **Wire types mirror the Rust DTOs.** A `type` describing an `invoke<T>()` result
  or payload restates the serde shape of its `#[tauri::command]` wrapper, with a doc
  comment naming the Rust source (the `useNotes.ts` pattern). See
  [`tauri-command-parity`](tauri-command-parity.md).
- **The UI stack is curated, and it is closed.** Grove builds on six packages and
  no others: `cva` + `clsx` (variants and conditional classes, for every new
  component), `@base-ui/react` (headless menu / dialog / popover / tooltip),
  `cmdk` (the command palette), `sonner` (toasts), and `motion` (motion CSS
  cannot express: gestures, layout animation, interruptible transitions). A
  seventh needs the same conversation the first six got.
  This **supersedes the zero-UI-dependency posture** and
  [`docs/decisions/popover-primitive.md`](../../docs/decisions/popover-primitive.md),
  which held against base-ui in 2026-07 on the strength of one primitive; Grove
  needs six, and the arithmetic is different at six.
  **Installed is not adopted.** The hand-rolled `Select` is a working, tested,
  accessible combobox: it gets replaced when someone has read what
  `@base-ui/react` gives in exchange, in its own ticket, not on sight. Same for
  the palette and the toast. See [`docs/UI_CONVENTIONS.md`](../../docs/UI_CONVENTIONS.md) §4.
