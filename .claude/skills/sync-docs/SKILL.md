---
name: sync-docs
description: Bring Kodabi's docs back in sync with the code — verify the mechanical anchors, audit the surrounding prose, and apply fixes. Use after a change that touches anything a doc enumerates, or to check the docs before a PR.
argument-hint: [optional scope — e.g. "schema docs only"]
---

# Sync docs

Reconcile `docs/`, `README.md`, and `CLAUDE.md` with the current code. The anchor
discipline is [`.claude/rules/docs-stay-in-sync.md`](../../rules/docs-stay-in-sync.md);
the canonical anchor list is
[`references/verification-procedures.md`](references/verification-procedures.md).

Scope from the caller (may be empty): $ARGUMENTS

## 1. Detect scope

`git diff --name-only main...HEAD` (or the caller's scope) → the docs at risk:

- `crates/kodabi-core/src/index/**` or note code → `FRONTMATTER_SCHEMA.md` +
  `MCP_TOOL_SURFACE.md`
- `.github/workflows/ci.yml` or gate changes → `CLAUDE.md` + `README.md`
- `src/components/ui/**` → `UI_CONVENTIONS.md`
- new top-level crate/dir → `README.md` layout block
- `Cargo.toml` `[features]` → `CLAUDE.md` feature-leg instructions

## 2. Verify anchors

Spawn the `doc-auditor` agent (it reads `references/verification-procedures.md`).
Collect its PASS/FAIL table.

## 3. Prose audit

Read the in-scope docs end to end for claims the diff invalidated. The auditor
covers the enumerable anchors; this step covers the surrounding prose (a description
of behavior that changed, a stale example).

## 4. Apply fixes

Edit only the docs (and `CLAUDE.md`/`README.md` as scoped) — never change code to
match a doc. After editing either schema doc, re-run
`node .claude/skills/frontmatter-validator/validate.mjs --check-schema` until it
passes. If the validator's own rules changed, run its `test.mjs`.

## 5. Report

The anchors table, the prose fixes applied, and anything deliberately left stale
(with the reason).
