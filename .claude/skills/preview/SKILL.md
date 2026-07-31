---
name: preview
description: Preview the Kodabi desktop app from the current (work)tree — launch Tauri dev mode and smoke-test it. Use to see a change working in the real app, or before committing a change that touches runtime behavior.
argument-hint: [optional focus — e.g. "check the listening indicator"]
---

# Preview Kodabi from this tree

Launch the desktop app from the **current working tree** (usually a Kangentic task worktree under
`.kangentic/worktrees/<slug>`) and confirm it is healthy before/after a change. Extra focus from
the caller (may be empty): $ARGUMENTS

## 1. Pick the vault

By default the app opens the real vault, whose contents are whatever they happen
to be. For a change to a note, session or retention surface, seed a throwaway one
first so every state is reachable:

```powershell
pnpm seed:vault -- --list                    # the scenario catalogue (pnpm eats bare flags)
pnpm seed:vault C:\kodabi-fixture            # all of them
pnpm seed:vault C:\kodabi-fixture retention/recording-only sessions/needs-attention
```

**Set both variables it prints, in the shell you launch from.** Setting only
`KODABI_KB_ROOT` is destructive: the startup reconcile job converges the real
index against the fixture and drops every row for the notes it can no longer see.
`Remove-Item Env:KODABI_KB_ROOT, Env:KODABI_INDEX_DB` when done, or the next
preview silently keeps using the fixture.

Scenario list, the marker-file rule, and why the seeder writes files rather than
index rows: [`e2e/README.md`](../../../e2e/README.md).

## 2. Launch

```sh
pnpm install          # worktrees start without node_modules — run on first preview or after package.json changes
pnpm tauri dev        # compiles the Rust workspace, starts Vite, opens the app window
```

Run it in the background and watch the output. First compile in a fresh worktree is slow (full
Rust build); later runs are incremental. If Vite's port is busy, another checkout (the main repo
or a sibling worktree) is probably already running — stop that preview first rather than fighting
over the port.

**Frontend-only fallback:** `pnpm dev` serves the React UI in a browser — useful for pure-UI
changes, but it exercises none of the Rust backend, so it never substitutes for a Tauri preview
on backend-touching changes.

## 3. What "healthy" looks like

- The dev command reaches Vite "ready" with **no Rust compile errors** in the output. (Dev mode
  transpiles without typechecking, runs no linter, and runs no tests, so a clean console says
  nothing about TS/eslint/test failures — those are only caught by the pre-commit gates:
  `pnpm build`, `pnpm exec eslint . --max-warnings=0`, and `pnpm test`.)
- The app window opens and renders the UI (not a blank/white webview — a blank window usually
  means the frontend crashed; check the webview devtools console).
- No panic or error spam in the terminal while idling.

## 4. Smoke-test the change

Exercise the specific flow the current change touches (capture toggle, indicator state, etc.) —
observing the affected behavior in the running app, not just a clean compile. If the change has
no visible surface, say so explicitly rather than claiming it was verified.

## 5. Shut down

Stop the dev process cleanly (Ctrl-C / kill the background task) so the port and lock files are
released, and clear the vault variables if step 1 set them. Report what was exercised and what was
observed.
