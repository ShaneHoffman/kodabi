---
name: test
description: Run or improve Kodabi's tests — quick/full/frontend/e2e modes run the right commands; audit/write modes delegate to the test-builder agent. Use to check tests before a commit, to drive the real app window end-to-end, or to fill a coverage gap.
argument-hint: [mode: quick | full | frontend | e2e | audit | write — plus optional focus]
---

# Test

A thin driver over Kodabi's test tiers. Writing and coverage analysis are delegated
to the `test-builder` agent.

Mode and focus from the caller (may be empty): $ARGUMENTS. If no mode is given,
default to **quick** for a diff-scoped check.

## Modes

- **quick** — `cargo test -p <crate>` for each crate the current diff touches. No
  `dist/` needed when `src-tauri` isn't among them.
- **full** — ensure `dist/` exists (`pnpm install --frozen-lockfile && pnpm build`),
  then `cargo test --workspace --locked`.
- **frontend** — `pnpm exec eslint . --max-warnings=0`, then `pnpm test`, then
  `pnpm build`. `pnpm test` is vitest + Testing Library under jsdom, over
  `src/**/*.test.{ts,tsx}`; coverage is the load-bearing seams (the distill/consent
  state machines, the Inbox re-route, quick capture), not the whole UI, so a green
  run is not a claim that everything is covered.
- **e2e** — `pnpm e2e:build`, then `pnpm test:e2e`. Drives the real app window over
  CDP against a temp vault, so it is the only tier that crosses the real IPC bridge.
  Windows-only; **never gates a commit**. Run it after a change to a control's wiring,
  a command's name, or a DTO's shape. `pnpm e2e:build` is not optional — `dist/` is
  embedded at compile time, so `cargo build` alone tests a stale frontend. A slice
  that needs notes or sessions on disk seeds them from the named catalogue in
  `e2e/lib/vault.mjs` (`launchKodabi({ seed: [...] })`) rather than building state by
  hand. See [`docs/UI_E2E_HARNESS.md`](../../../docs/UI_E2E_HARNESS.md).
- **audit** — spawn `test-builder` in **audit** mode with the diff scope; it returns
  a read-only coverage-gap table.
- **write** — spawn `test-builder` in **write** mode for the target; then run
  **full** to confirm green.

## Notes

- `#[ignore]`d "real" tests (needing models/hardware or env like
  `KODABI_EMBED_MODEL_DIR`) run only on explicit request, with their prerequisites.
  They never gate a commit.
- Report as a plain table (tier · command · result). No emojis.
