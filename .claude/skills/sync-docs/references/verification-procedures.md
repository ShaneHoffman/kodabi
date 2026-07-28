# Verification procedures — Kodabi doc anchors

This is the **canonical anchor list**: the enumerable promises Kodabi's docs make
about the code, and exactly how to check each one. The `doc-auditor` agent and the
`/sync-docs` skill both read this file; the summary in
[`.claude/agents/doc-auditor.md`](../../../agents/doc-auditor.md) must be kept in
step with it.

An *anchor* is a place where a doc enumerates something the code (or another doc)
also defines, so the two can silently disagree. Prose that merely describes
behavior is audited separately (the sync-docs "prose audit" step), not here.

---

## Anchor 1 — Frontmatter schema ↔ MCP tool surface

- **Source of truth:** `docs/FRONTMATTER_SCHEMA.md` (the note frontmatter field set,
  key order `id, type, project, date, tags, source, confidence`, the `NoteId`
  pattern `^n_[0-9a-z]{6,}$`, the `NoteType` enum).
- **Mirror:** `docs/MCP_TOOL_SURFACE.md`, the `$defs.NoteSummary` shape.
- **Verify:** run
  `node .claude/skills/frontmatter-validator/validate.mjs --check-schema`.
  It reads both docs and cross-checks field set, `NoteId` pattern, `NoteType` enum,
  and key order.
- **Failure:** non-zero exit = the two docs have drifted (or the validator's encoded
  rules no longer match them). This is the one anchor `CLAUDE.md` calls a hard gate.

## Anchor 2 — Pre-commit gates ↔ CI

- **Source of truth:** `.github/workflows/ci.yml` (the actual `run:` lines each job
  executes).
- **Mirror:** the "Pre-commit gates" paragraph in `CLAUDE.md`.
- **Verify:** read the workflow's `run:` steps and confirm `CLAUDE.md` lists the same
  commands. In particular the transcribe matrix runs **three** feature legs
  (`parakeet`, `vad`, `whisper`), the embed `bge` leg is path-filtered to
  changes under `crates/kodabi-embed` **or** `crates/kodabi-core`, and the `app` job
  runs four steps for the shipping configuration (the `--test parakeet_real --ignored`
  real-model run, `cargo clippy -p kodabi --features parakeet …`, the
  `--release --features parakeet` build, and the release-guard check). Only the clippy
  leg is a per-commit gate in `CLAUDE.md`; the release build is `/pull-request`'s.
- **Failure:** a gate CI runs that `CLAUDE.md` omits (or vice versa). The pre-commit
  gates promise to "mirror CI exactly", so any difference is a gap.

## Anchor 3 — Repository layout ↔ tree

- **Source of truth:** the actual top-level tree (crates under `crates/`, top-level
  directories).
- **Mirror:** the "Repository layout" block in `README.md`.
- **Verify:** Glob the top level and each `crates/*`; confirm every path the README
  lists exists, and that new top-level directories or crates appear in the block.
- **Failure:** a listed path that no longer exists, or a new crate/dir the block
  doesn't mention.

## Anchor 4 — UI primitives ↔ docs/UI_CONVENTIONS.md

- **Source of truth:** `src/components/ui/` (the exported primitives and their props).
- **Mirror:** the "Primitives" section of `docs/UI_CONVENTIONS.md` — `Button` (variants,
  `loading`), `TextField` (`error`, `hint`), `Select` (`disabled` vs **`busy`**,
  `emptyLabel`, keyboard behavior), `Checkbox` (and its `--check-*` coupling to the
  shared markdown surface's task list), `ViewFrame` (eight variants; `summary` is a
  **type error** outside `queue`/`library`/`health`, not a silent no-op),
  `StatusMessage` (variant → ARIA
  role), `Overlay` — plus the "What consumes these today" table.
- **Do not look for `Textarea`, `ListRow` or `PlaceholderView`.** All three were
  deleted (they had no call sites); `UI_CONVENTIONS.md` keeps a "there is no X
  primitive" note for the first two saying what to copy instead. A live reference to
  any of them anywhere is itself a failure.
- **Verify:** read each primitive's exported prop types and compare against the
  documented variants/props/behavior claims. Also confirm the consumers table names
  the primitives each surface actually imports.
- **Failure:** a documented variant or prop the component no longer has, a new
  primitive the doc omits, or a surface whose imports contradict the table.

> This list is the thing an auditor works from, so **it goes stale the moment a
> primitive is added or removed** and nothing else will catch that. Updating it is
> part of the same change, exactly like updating the doc it points at.

## Anchor 5 — Feature legs ↔ Cargo features

- **Source of truth:** each crate's `Cargo.toml` `[features]` (e.g.
  `kodabi-transcribe`'s `parakeet`/`vad`/`whisper`, `kodabi-embed`'s `bge`,
  `src-tauri`'s forwarded `parakeet`/`whisper`/`embed`) and the ci.yml matrix plus
  the `app` job.
- **Mirror:** the feature-leg instructions in `CLAUDE.md` (which crates need which
  `cargo clippy --features …` legs before commit).
- **Verify:** confirm every off-by-default feature that CI clippy-checks is named in
  `CLAUDE.md`'s pre-commit instructions, with the same build-environment notes
  (whisper needs MSVC + `LIBCLANG_PATH`). Also confirm the feature release builds ship
  with (`parakeet`, via `package.json`'s `tauri:build` script) matches what `CLAUDE.md`
  and `README.md` claim, and that the `compile_error!` guard in
  `src-tauri/src/transcribe.rs` still names it.
- **Failure:** a feature CI checks that `CLAUDE.md`'s commit instructions don't
  mention.

## Anchor 6 — Design tokens ↔ docs/DESIGN_SYSTEM.md

- **Source of truth:** `design/tokens.css` (the token families) and the two guards,
  `src/designTokens.test.ts` plus the `no-restricted-syntax` block in `eslint.config.js`.
- **Mirror:** `docs/DESIGN_SYSTEM.md` — the motion table (`--dur-*` / `--ease-*`), the
  layer names, the contrast matrix in §6, and the enforcement claims in §7.
- **Verify:** every `--dur-*` and `--ease-*` token in `tokens.css` appears in the §4 table
  and vice versa; the contrast figures match a recomputation from the Layer-1 pigments;
  §7's description of what each guard catches matches what the guard actually asserts.
- **Failure:** a motion token the table omits, a contrast figure that no longer matches
  the pigments, or an enforcement claim the guards do not make.
  (`pnpm test` covers the token/theme structure itself — this anchor covers the prose.)

## Anchor 7 — MCP tool surface ↔ the server's committed schemas

- **Source of truth:** `docs/MCP_TOOL_SURFACE.md` — each tool's `title`,
  `description`, `inputSchema`, `outputSchema`, and `annotations`, plus the shared
  `$defs` library.
- **Mirror:** `crates/kodabi-mcp/schemas/<tool>.{input,output}.json` (one file per
  tool per direction) and the `TOOLS` table plus description consts in
  `crates/kodabi-mcp/src/schemas.rs`. That module's own doc comment promises each
  file is "a verbatim copy of the matching block" in the spec.
- **Verify:** for every entry in `TOOLS`, diff its two schema files against the
  spec's blocks — property sets, `required` lists, defaults, bounds, and
  descriptions must match verbatim, with only the transitive `$defs` subset inlined
  (the spec says the server inlines rather than references). Confirm the tool's
  `title` and description const match the spec's `- **title:**` /
  `- **description:**` lines, and that `read_only` matches its `readOnlyHint`.
  Also confirm every tool the spec's Tool-index table lists has a `TOOLS` entry.
- **Failure:** a schema file that has drifted from its spec block, a tool in the
  spec with no `TOOLS` entry (or vice versa), or a `read_only` flag disagreeing with
  the documented annotation.
  (The crate's own tests cover `$ref` resolution, the open-`NoteSummary` invariant,
  the 2 KB description cap, and — via `kodabi_core::terminal::READ_TOOL_PERMISSIONS`
  — that every read tool is pre-approved. None of them compare against the spec,
  which is what this anchor is for.)

---

**Adding an anchor:** add its section here *and* the one-line entry in the
doc-auditor agent's anchor summary — the agent works from that summary, so an anchor
present in only one of the two files goes unchecked.
