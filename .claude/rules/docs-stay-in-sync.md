# Docs stay in sync

Several Kodabi docs make *mechanical* promises about the code — a field set, a
list of gate commands, a repository layout. These **anchors** must not drift from
what they describe.

- **The canonical anchor list lives in one place:**
  [`.claude/skills/sync-docs/references/verification-procedures.md`](../skills/sync-docs/references/verification-procedures.md).
  It names each anchor, its source of truth, its mirror, and how to check it.
  Consult that file — don't re-derive the list here.
- **The one hard gate** (also in `CLAUDE.md`): editing `docs/FRONTMATTER_SCHEMA.md`
  or `docs/MCP_TOOL_SURFACE.md` requires
  `node .claude/skills/frontmatter-validator/validate.mjs --check-schema` to pass
  in the **same change**. The two docs mirror each other (frontmatter fields ≡ the
  MCP `NoteSummary` shape); the validator enforces it programmatically.
- **A code change that invalidates a doc claim fixes the doc in the same commit.**
  Don't leave the README's layout block, a CLAUDE.md gate command, or a documented
  schema field describing code that no longer matches.
- **When in doubt, run the audit.** Spawn the `doc-auditor` agent (or run
  `/sync-docs`) after a change that touches anything a doc enumerates. Enforcement
  here is the validator plus that agent plus review — there is no CI scan.
