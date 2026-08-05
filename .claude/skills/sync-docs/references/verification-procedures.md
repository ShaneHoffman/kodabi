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
  changes under `crates/kodabi-embed` **or** `crates/kodabi-core`, and the shipping
  configuration is covered by **two** parallel jobs sharing one identical path filter:
  `app-dev` runs the `--test parakeet_real --ignored` real-model run and
  `cargo clippy -p kodabi --features parakeet …`, while `app` runs the
  `--release --features parakeet` build and the release-guard check. Only the clippy
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
- **Mirror:** §4 of `docs/UI_CONVENTIONS.md` — the primitive table (name → variants) and
  the "contracts worth not breaking" list beneath it: `Button` (`loading` vs `disabled`,
  and the focus reason), `Select` (`busy` vs `disabled`, the combobox keyboard set,
  `hideLabel`, `emptyLabel`), `Switch` (`busy` as its only inert form — no `disabled` prop
  exists; `label` is the visible words verbatim; the knob's travel is duration-gated, so it
  still arrives under reduced motion),
  `Field` (`error` + `aria-invalid` travel together, `hint` and
  `error` both → `aria-describedby`, error described first),
  `StatusMessage` (variant → ARIA role), `ViewFrame` (eight variants; `summary` a **type
  error** outside `queue`/`library`/`health`, `action` a **type error** on `doc`/`search`,
  neither a silent no-op), `Overlay` (click-not-pointerdown, both ends on the backdrop, no
  focus trap), `DestructiveConfirmDialog` (presentational, never self-closing; the copy
  structure — title, `subject` strip, consequence, the dialog's own permanence line, quiet
  Cancel before the danger confirm).
- **Do not look for `Textarea`, `ListRow` or `PlaceholderView`.** All three were
  deleted (they had no call sites); §4 keeps a "there is no X primitive" note saying what
  to do instead. A live reference to any of them anywhere is itself a failure.
- **Verify:** read each primitive's exported prop types and compare against the documented
  variants and the contract list. The contracts are behavioural, so check the
  implementation and the component's tests, not just the type — `loading` and `busy` in
  particular are only correct if the control stays focusable and declines activation.
- **Failure:** a documented variant or prop the component no longer has, a new primitive
  §4 omits, or a contract in the list that the component no longer honours.
- **§4 documents behaviour, not styling.** The primitives §4 still calls pre-Grove carry their
  own stylesheets, and §4 says which. Do not report their styling as drift against
  `DESIGN_SYSTEM.md` — each is restyled by a ticket of its own, and until then the two
  documents are describing different layers on purpose. `Select` has had that ticket (its
  chrome is Grove and `Select.css` is gone, minus the anchor-positioning block that moved to
  `src/index.css` §3); its **behaviour** contract above is unchanged by it, which is the point.

> This list is the thing an auditor works from, so **it goes stale the moment a
> primitive is added or removed** and nothing else will catch that. Updating it is
> part of the same change, exactly like updating the doc it points at.

## Anchor 5 — Feature legs ↔ Cargo features

- **Source of truth:** each crate's `Cargo.toml` `[features]` (e.g.
  `kodabi-transcribe`'s `parakeet`/`vad`/`whisper`, `kodabi-embed`'s `bge`,
  `src-tauri`'s forwarded `parakeet`/`whisper`/`embed`) and the ci.yml matrix plus
  the `app-dev`/`app` jobs.
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

## Anchor 6 — The Grove theme ↔ docs/DESIGN_SYSTEM.md

- **Source of truth:** the `@theme` block and the `.day` / `.hc` blocks in
  `src/index.css` (the tokens, the keyframes, the two variants), plus the two Grove
  guards in `eslint.config.js`'s `no-restricted-syntax` block.
- **Mirror:** `docs/DESIGN_SYSTEM.md` — the type table in §1, the radius ladder in §5,
  the motion vocabulary and the reduced-motion swap table in §4, the measured contrast
  tables in §6, and the enforcement claims in §7. Also `docs/UI_CONVENTIONS.md` §3,
  whose "want → write" table names the utilities the tokens generate.
- **Verify:**
  1. Every `--animate-*` in `@theme` appears in §4's swap table with a partner, and
     every `@keyframes` has an `--animate-*` that references it (a keyframe inside
     `@theme` with no companion variable is never emitted).
  2. Every `--radius-*` appears in §5's ladder, and every `--font-*` in §1's table.
  3. **The contrast figures are recomputed, not eyeballed.** §6 quotes ratios against
     both the ground AND the composited glass panel; recompute both columns from the
     token values with a WCAG 2.1 relative-luminance check, compositing alpha first.
     The panel is the tighter number and the one the floor is set on — an auditor who
     checks only the ground column will report PASS on a `ink-faint` that fails where
     it actually renders. That is the exact defect §6 records having caught once.
  4. Every token the `.day` block sets also exists in `@theme` (a day value for a
     token that no longer exists is dead, and reads as coverage).
  5. §7's description of what each guard catches matches what the guards assert.
- **Failure:** an animation or radius the doc omits, a contrast figure that no longer
  matches the tokens in either column, a `.day` override with no base token, or an
  enforcement claim the guards do not make.
- **The `.hc` block is a closed set, and §6 says so.** It moves exactly `ink-faint`,
  `edge` and `switch-on`. If it has grown a fourth token, §6's "nothing else moves,
  because the table shows nothing else needs to" is now false and the table should show
  why — the doctrine's own test is that an addition here is justified by a ratio the
  table is *missing*, which is the argument §6 makes for `switch-on` (a switch's track
  reports a state and clears 3:1 at no alpha that belongs on a card, so the knob's
  travel is the readout and the fill is a hint worth strengthening).
- **Nothing here is machine-checked, unlike the pre-Grove anchor.** The old token
  guard asserted theme-block coverage in `pnpm test`; Grove retired it along with the
  stylesheets it scanned. Every check above is this auditor's job in full.

> **Migration note (Phase 4).** `design/tokens.css`, the `@theme inline` bridge at the
> bottom of `src/index.css`, and the per-component `*.css` files are frozen legacy for
> unmigrated screens. They are **not** an anchor: do not audit them against
> `DESIGN_SYSTEM.md`, which no longer describes them. Delete this note when the final
> cleanup ticket removes them.

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
