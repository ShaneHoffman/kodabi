---
paths:
  - .claude/skills/**
  - .claude/agents/**
---

# Skill authoring

Guidance for adding or changing anything under `.claude/skills/` and
`.claude/agents/`.

- **Deterministic checks belong in a bundled script; judgment belongs in prose.**
  The `frontmatter-validator` skill is the model: the rules live in
  `validate.mjs` (zero-dependency Node, testable via `test.mjs`), and the SKILL.md
  just drives it. Never re-implement a bundled script's logic as prose steps — the
  two would drift.
- **Match the house style.** Frontmatter keys are exactly `name`, `description`,
  `argument-hint` (add `disable-model-invocation: true` only for a human/board-column
  skill like `code-review-fix`). The body is an H1 title, hard rules bolded near the
  top, a `…from the caller (may be empty): $ARGUMENTS` line, then numbered
  `## N. Step` sections. Agents use `name`, `description` (with when-to-use
  examples), a single `model` key, and `tools`.
- **Never route mutating work to a read-only agent.** The built-in `Explore` and
  `Plan` agents can't edit and don't load `CLAUDE.md`, so a fix or a scaffold sent
  there silently drops the repo's conventions. Mutation happens in the main session
  or a tool-equipped agent.
- **Verify by delegating to the matching auditor.** Active skills spawn a fresh
  read-only agent to check their work:
  `add-tauri-command` → `tauri-command-auditor`,
  `add-migration` → `migration-safety`,
  `test` (audit/write) → `test-builder`,
  `sync-docs` → `doc-auditor`.
  The one exception is `code-review-fix`: it *is* the independent review pass, so it spawns no
  auditor of its own — its own findings are the check.
- **Keep skills lean.** Link `CLAUDE.md`, the docs, and the rules rather than
  restating them; a skill that duplicates a rule becomes the second thing to update.
