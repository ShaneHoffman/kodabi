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
- **No new UI runtime dependencies without discussion.** The app holds a
  zero-UI-dependency posture — the hand-rolled `Select` primitive (a full
  combobox with no headless library) is the precedent. Add a dependency only after
  agreeing it's worth the weight.
