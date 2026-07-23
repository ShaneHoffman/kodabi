---
name: frontmatter-validator
description: Validate a note's YAML frontmatter against docs/FRONTMATTER_SCHEMA.md — required fields, id pattern, type enum, ISO-8601 date, tags/confidence rules, and canonical key order. Run on a single note or sweep a folder to keep the Phase 2 writer and hand-edited notes honest before the index and MCP tools trust them.
argument-hint: [path to a note or a folder of notes, or --check-schema]
---

# Validate note frontmatter

Check that a note's YAML frontmatter conforms to [`docs/FRONTMATTER_SCHEMA.md`](../../../docs/FRONTMATTER_SCHEMA.md)
— the guardrail that keeps the Phase 2 markdown writer and any hand-edited note honest before the
file watcher indexes them and the MCP tools route over them. Path or focus from the caller (may be
empty): $ARGUMENTS

The logic lives in a zero-dependency Node script bundled with this skill; drive it, don't
re-implement the checks by hand.

## 1. Run it

```sh
# a single note
node .claude/skills/frontmatter-validator/validate.mjs path/to/note.md

# sweep every .md under a folder (recurses; skips node_modules and .git)
node .claude/skills/frontmatter-validator/validate.mjs path/to/vault/
```

It prints one `PASS`/`FAIL` line per file, indented findings under each, then a summary. It
**exits non-zero if any file has an error**, so it can gate the writer or a commit.

## 2. What it checks

Against the canonical field set `id, type, title, project, date, tags, source, confidence`:

- **Required** — `id`, `type`, `project`, `date`, `source` must be present.
- **`id`** — matches `^n_[0-9a-z]{6,}$`.
- **`type`** — one of `meeting | note | chat`.
- **`title`** — optional free-text display title; omit the key when absent.
- **`date`** — ISO-8601: either date-only `YYYY-MM-DD` or a timestamp with offset
  (`2026-07-09T14:00:00-07:00` / `…Z`); a real calendar date/time.
- **`tags`** — omit the key when empty (an empty `tags:` / `tags: []` is an error); otherwise a
  list of lowercase kebab-case tags with no leading `#`.
- **`confidence`** — if present, a number in `0.0`–`1.0`; **required when `project: Inbox`** (the
  routing score is why the note landed there). A present `confidence` on a `source: import|manual`
  note is a warning, since hand-filed/imported notes normally omit it.
- **Canonical key order** — the present keys must appear in the order above.
- **Schema drift** — any field outside the canonical set is flagged.

## 3. Check the two specs still agree

```sh
node .claude/skills/frontmatter-validator/validate.mjs --check-schema
```

Cross-checks the validator's encoded rules against both source docs — the canonical key order and
`id` pattern in `FRONTMATTER_SCHEMA.md`, and the mirrored field set, `NoteId` pattern, and
`NoteType` enum in the `NoteSummary` shape of `docs/MCP_TOOL_SURFACE.md`. Run it after editing
either doc (per the repo's spec-agreement rule) to catch the two drifting apart. Exits non-zero on
drift.

## 4. Reading the output

- **`ERROR`** — a hard schema violation; the file fails and the run exits non-zero.
- **`WARN`** — a likely-but-not-certain issue (e.g. the confidence-on-manual-note heuristic); does
  not fail the run. Judge it in context.
- Each finding names the offending field and, where determinable, the line number.

Report which files passed, quote the exact findings for any that failed, and — when validating the
writer's output — say whether the writer or the note needs the fix.

## 5. Verify the validator itself

```sh
node .claude/skills/frontmatter-validator/test.mjs
```

Runs the bundled fixtures (the three schema examples as valid cases, four injected violations as
failing cases) plus the schema-mirror check, and exits non-zero on any regression. Run it after
changing `validate.mjs` or the schema.
