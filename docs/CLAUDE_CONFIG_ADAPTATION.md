# Adapting Kangentic's Claude config for Kodabi

*Status: Done (2026-07-18). Records how the Kangentic repo's `.claude/` config was triaged for
Kodabi.*

Kodabi's development runs on a [Kangentic](https://github.com/Kangentic/kangentic) board, and that
repo carries a mature set of Claude Code workflows: `settings.json`, 9 agents, 29 rules, and 17
skills (plus one reference file). This document triages every item — **adopt**, **amend**, or
**skip** — with a one-line reason.

**Method.** Each file's actual content was read, not just its name (names mislead: `ipc-7-layer-parity`
is about Kangentic's Electron IPC stack, but its *pattern* — keep N mirrored layers in lockstep —
maps onto Kodabi's Tauri command layers). Nothing was copied blind. Adopted and amended items were
rewritten entirely in Kodabi's vocabulary (Tauri + Rust workspace, the `CLAUDE.md` pre-commit
gates, the core-vs-shell rule, design tokens, the board flow). No Kangentic internals survive an
adaptation. Existing Kodabi config (`rules/copy-style.md`; skills `frontmatter-validator`,
`preview`, `pull-request`) was **merged, not clobbered**.

**Legend.**

- **Adopt** — rewritten for Kodabi as a new file under `.claude/`.
- **Amend** — merged into an existing Kodabi file, or renamed/reshaped to Kodabi's stack.
- **Skip** — not used; reason (and any revisit trigger) given. Skipped items appear only here.

The stack mismatch drives most skips: Kangentic is Electron + Vite + esbuild + better-sqlite3 with
a 7-layer IPC bridge, a Zustand/HMR renderer, a PTY/session state machine, an activity engine, and
a kanban board engine. Kodabi is Tauri + a Rust workspace + React, Windows-only for v1. Kangentic
also enforces most rules with a `tests/unit/*.test.ts` (vitest) CI scan; Kodabi had no JS test
runner when this adaptation was made, so every adopted rule states honest enforcement (the
frontmatter validator, the cargo/eslint gates, an auditor agent, or review) rather than a ported
test. Kodabi has since gained vitest + Testing Library for the frontend, but it covers the
load-bearing UI seams rather than scanning the config, so the dispositions below still stand.

## Agents (9)

| Kangentic item | Disposition | Kodabi destination / reason |
| --- | --- | --- |
| doc-auditor | Adopt | `.claude/agents/doc-auditor.md` — anchors retargeted to Kodabi's five (schema mirror, gates↔CI, README layout, UI primitives, feature legs). |
| hmr-integrity | Skip | Their Zustand/Electron HMR re-sync registry; Kodabi's state lives in the Rust backend. |
| hmr-parity | Skip | Their HMR primitive catalog; no dev-mode HMR-parity surface. |
| ipc-auditor | Amend | `.claude/agents/tauri-command-auditor.md` — audits Kodabi's command layers and flags fat wrappers (core-vs-shell). |
| marketing-captures | Skip | Their Playwright screenshot framework. Revisit in Phase 4 for launch assets. |
| migration-safety | Adopt | `.claude/agents/migration-safety.md` — rewritten for the append-only `migrations()` vec + `user_version` model. |
| platform-guard | Skip | Cross-platform pitfall scanner; Kodabi is Windows-only for v1 (anti-scope). |
| session-debugger | Skip | Their PTY/session state machine. (Also shipped with duplicate `model:` keys — a "don't copy blind" tell.) |
| test-builder | Adopt | `.claude/agents/test-builder.md` — kept the audit/write modes and red-green tier discipline; retargeted to Rust, dropped all Playwright/ConPTY content. |

## Rules (29)

| Kangentic item | Disposition | Kodabi destination / reason |
| --- | --- | --- |
| activity-state-classification | Skip | Their session-activity enum internals. |
| agent-adapters-boundary | Skip | Their agent-CLI adapter layer; Kodabi's engine traits have no name-branching problem today. |
| bash-single-command | Amend | `.claude/rules/shell-discipline.md` — slimmed: dedicated tools + `git -C` kept; no hook and no hard single-command ban (this harness legitimately chains in PowerShell). |
| board-completing-task-chokepoint | Skip | Their kanban drag/flight internals. |
| board-config-parity | Skip | Their board-config serialization. |
| browser-automation-driver | Skip | Their CDP/webview automation surface. |
| central-embedding-engine | Skip | Their embed engine. Pattern noted if Kodabi's file watcher/index scheduler ever needs a single work-owner. |
| cli-features-over-custom-layers | Skip | Their agent-spawn philosophy. |
| cross-platform-parity | Amend (partial) | Windows-only anti-scope; its "tests write only under temp dirs / no machine paths in fixtures" folded into `.claude/rules/no-personal-info.md`. |
| dev-tooling-build-exclusion | Skip | No devtools tree; the esbuild `define` mechanism doesn't map to Vite/Tauri. |
| docs-stay-in-sync | Adopt | `.claude/rules/docs-stay-in-sync.md` — anchor discipline; canonical anchor list in the sync-docs reference. |
| esbuild-cjs-imports | Skip | Kodabi bundles with Vite; no CJS main-process bundle. |
| external-scripts-parity | Skip | No unbundled bridge scripts. |
| hmr-patterns | Skip | Their Vite/Zustand HMR primitive catalog. |
| ipc-7-layer-parity | Amend | `.claude/rules/tauri-command-parity.md` — Kodabi's real 5 layers (core fn → thin wrapper → registration → typed TS caller → events). |
| keybindings-registry | Skip | Kodabi has a command palette and few shortcuts; no registry to guard yet. |
| mcp-tool-list-parity | Skip | Kodabi's MCP server isn't built yet. Revisit in **Phase 3** — the manifest + honest read/mutating annotations pattern applies to `MCP_TOOL_SURFACE.md` then. |
| no-personal-info | Adopt | `.claude/rules/no-personal-info.md` — public AGPL repo + temp-dir test discipline. |
| pop-out-surface-registry | Skip | Electron multi-window registry; Kodabi's three windows are static Tauri config. |
| project-scoped-ipc | Skip | No multi-project ambient-context switch hazard in a single-vault app. |
| restore-no-animation-replay | Skip | Their workspace-restore animation system. |
| skill-authoring | Adopt | `.claude/rules/skill-authoring.md` — fork-vs-inline framework + the Kodabi skill→agent delegation map. |
| spawn-entry-point-parity | Skip | Their session-spawn engine. Pattern noted for a future index-write chokepoint rule if the watcher adds a second writer. |
| synchronous-shutdown | Skip | Electron `before-quit` specifics; Tauri's lifecycle differs and no zombie problem is observed. |
| task-lifecycle-lock | Skip | Their per-task async mutex. |
| text-formatting | Skip | `rules/copy-style.md` already governs the user-facing surface; a repo-wide em-dash ban would contradict Kodabi's own docs style. |
| typescript-style | Adopt | `.claude/rules/typescript-style.md` — grounded in the real `tsconfig` strict flags + eslint gate. |
| ui-conventions | Amend | Transferable bits merged into `docs/UI_CONVENTIONS.md` (no raw `<select>`; type floor; no hover-only affordances; `data-testid`), not a parallel rule. |
| utc-timestamps | Adopt | `.claude/rules/utc-timestamps.md` — with the frontmatter-`date` carve-out so it doesn't flag correct quick-capture code. |

## Skills (17 + 1 reference)

| Kangentic item | Disposition | Kodabi destination / reason |
| --- | --- | --- |
| add-ipc-endpoint | Amend | `.claude/skills/add-tauri-command/` — scaffold-then-verify; ends by spawning `tauri-command-auditor`. |
| add-migration | Adopt | `.claude/skills/add-migration/` — Kodabi's append-only model, upgrade-test requirement; spawns `migration-safety`. |
| code-review | Skip → superseded | Originally skipped as a shadow of the built-in `/code-review high` the column ran. The column now runs Kodabi's own `.claude/skills/code-review-fix/` (review → fix → gates → commit). The built-in is invocable, but it stops at reporting: it never fixes, never runs the gate matrix, and never commits, so the stage could not be self-contained. |
| commit | Amend | `.claude/skills/commit/` — inverted: theirs skips gates, Kodabi's `CLAUDE.md` mandates them, so the skill runs a surface→gate matrix. Never pushes. |
| cross-platform | Skip | Windows-only v1 anti-scope. |
| debug-activity | Skip | Their activity-engine playbook. |
| ipc-bridge | Amend (fold) | Knowledge folded into `rules/tauri-command-parity.md` + the `add-tauri-command` skill; no separate knowledge skill. |
| merge-back | Skip | Pushes straight to main, bypassing the PR gate — contradicts the board flow. |
| merge-pull-request | Skip | Admin-merges the PR — Kodabi's flow has a human merge on GitHub. Revisit only if a "Ship It" column is ever added. |
| preview | Skip | Kodabi's own `preview` skill already covers Tauri dev; theirs is bound to their worktree-junction script. |
| pull-request | Amend | Kept Kodabi's board-integrated skill; added a "watch CI and fix failures" step (bounded 3-round loop, never bypass, never merge). |
| release-protocol | Skip | Publishes their `@kangentic/protocol` npm package; Kodabi has none. |
| release | Skip | No release/installer infrastructure yet. Revisit in **Phase 4** (installer + signing). |
| scaffold-feature | Adopt | `.claude/skills/scaffold-feature/` — plan → confirm → core → wrapper → register → TS → UI. |
| session-lifecycle | Skip | Their session subsystem reference. |
| sync-docs (+ references/verification-procedures.md) | Adopt | `.claude/skills/sync-docs/` + a Kodabi-specific verification-procedures reference (the canonical anchor table); spawns `doc-auditor`. |
| test | Adopt | `.claude/skills/test/` — thin driver over Kodabi's tiers (quick/full/frontend/audit/write). |

## Settings (1)

| Kangentic item | Disposition | Kodabi destination / reason |
| --- | --- | --- |
| settings.json | Amend | New `.claude/settings.json` — a conservative permission allowlist (cargo/pnpm/git/gh read paths + doc `WebFetch` domains), no hooks, empty deny. The `bash-guard.js` `PreToolUse` hook was **not** ported (the script doesn't exist here and the strict single-command rule it enforces conflicts with this harness's PowerShell chaining). `git push`, `gh pr create`/`merge`, and `rm` are deliberately left to prompt. The machine-local `settings.local.json` is untouched. |

## Revisit later

- **Phase 3 (MCP server):** `mcp-tool-list-parity` — when Kodabi's stdio MCP server lands, adopt
  the manifest + honest read-vs-mutating annotation pattern so the tool list and
  `docs/MCP_TOOL_SURFACE.md` can't drift.
- **Phase 4 (launch / installer):** `marketing-captures` (launch screenshots), and `release` +
  `release-protocol` (once there is an installer, signing, and a release workflow to script
  against).
