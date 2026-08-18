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
  `cargo clippy -p kodabi --features parakeet,embed …`, while `app` runs the
  `--release --features parakeet,embed` build and the release-guard check. Only the clippy
  leg is a per-commit gate in `CLAUDE.md`; the release build is `/pull-request`'s.
  `release.yml` is mostly out of scope for this anchor — it runs no *pre-commit* gate,
  only the tagged installer build (see anchor 5 for the feature set it inherits from
  `tauri:build`). It carries two assertions, neither a pre-commit gate: the tag must match
  `version` in **both** `package.json` and `src-tauri/tauri.conf.json`, so confirm those
  two still agree with each other whenever either is bumped; and the updater signing check,
  which requires the two `TAURI_SIGNING_*` secrets plus a non-placeholder
  `plugins.updater.pubkey`. Two things in `release.yml`
  *are* in scope, because it copies them from `ci.yml` rather than deriving them: its
  toolchain setup (`pnpm/action-setup` version, `actions/setup-node` `node-version`, the
  `dtolnay/rust-toolchain` channel) must still match `ci.yml`'s, and its own comment says
  so. Nothing enforces it, and a mismatch surfaces only after a tag is pushed and the
  hour-long build has already started.
- **Failure:** a gate CI runs that `CLAUDE.md` omits (or vice versa). The pre-commit
  gates promise to "mirror CI exactly", so any difference is a gap. Also a failure: a
  toolchain version that differs between `ci.yml` and `release.yml`.

## Anchor 3 — Repository layout ↔ tree

- **Source of truth:** the actual top-level tree (crates under `crates/`, top-level
  directories).
- **Mirror:** the "Repository layout" block in `README.md`, **and** the crate table in
  §2 of `docs/ARCHITECTURE.md`, which enumerates the same seven `crates/kodabi-*`
  members with what each owns. The two mirrors are independent, so a new crate can
  reach one and miss the other.
- **Verify:** Glob the top level and each `crates/*`; confirm every path the README
  lists exists, and that new top-level directories or crates appear in the block.
  Then confirm ARCHITECTURE.md §2's table names the same crate set (it lists crates
  only, not top-level directories), that the crate count stated in the prose above the
  table still matches the number of rows, and that no row contradicts the README's
  one-line description of the same crate.
- **Failure:** a listed path that no longer exists, a new crate/dir the README block
  doesn't mention, a crate missing from — or misdescribed in — ARCHITECTURE.md §2, or a
  crate count in §2's prose that no longer matches its table.

## Anchor 4 — UI primitives ↔ docs/UI_CONVENTIONS.md

- **Source of truth:** `src/components/ui/` (the exported primitives and their props).
- **Mirror:** §4 of `docs/UI_CONVENTIONS.md` — the primitive table (name → variants) and
  the "contracts worth not breaking" list beneath it: `Button` (`loading` vs `disabled`,
  and the focus reason), `Select` (`busy` vs `disabled`, the combobox keyboard set,
  `hideLabel`, `emptyLabel`), `Switch` (`busy` as its only inert form — no `disabled` prop
  exists; `label` is the visible words verbatim; the knob's travel is duration-gated, so it
  still arrives under reduced motion),
  `Checkbox` (a box and its label; still no variants, but it now carries a §4 contract bullet
  of its own: `busy` is the inert form that is *not* `disabled` — `aria-busy` + `aria-disabled`,
  focusable, swallowing its own change, pulsing with `animate-pending` (opacity-only, so
  correct unpaired under reduced motion) — while `disabled` keeps meaning a box there is
  nothing to tick; `hideLabel` is `Select`'s, same reason),
  `Field` (`error` + `aria-invalid` travel together, `hint` and
  `error` both → `aria-describedby`, error described first),
  `StatusMessage` (variant → ARIA role), `ViewFrame` (seven variants; `summary` a **type
  error** outside `queue`/`library`/`health`, `action` a **type error** on `doc`/`search`,
  neither a silent no-op; `label` names the section landmark when `title` is composed of
  elements rather than a plain string), `Dialog` (base-ui owns the focus trap, Escape, the outside
  press and the scroll lock; `initialFocus` where the first tabbable control is destructive;
  margin centering, since `materialize` animates `transform`),
  `Menu` (base-ui anchoring; `Menu.Item`'s three variants `default`/`suggested`/`foot` are
  variants precisely *because* a call-site `className` loses the cascade, so a row carries
  exactly one size utility and one colour utility with nothing to resolve — `Menu.test.tsx`
  counts them; `Menu.Trigger` **composes, it does not wrap**: the control goes through
  `render` so one `<button>` carries both the Grove chrome and base-ui's wiring),
  `DestructiveConfirmDialog` (presentational, never self-closing; the copy
  structure — title, `subject` strip, consequence, the dialog's own permanence line, quiet
  Cancel before the danger confirm).
- **Do not look for `Textarea`, `ListRow` or `PlaceholderView`.** All three were
  deleted (they had no call sites); §4 keeps a "there is no X primitive" note saying what
  to do instead. A live reference to any of them anywhere is itself a failure.
- **Nor for `Overlay`.** The pre-Grove modal shell was deleted by the Grove cleanup once
  `Dialog` had taken its last caller, and `src/useDialogFocus.ts` went with it. Surviving
  mentions are past-tense comparisons in `Dialog`'s contract and are correct; a *live*
  reference is a failure.
- **Verify:** read each primitive's exported prop types and compare against the documented
  variants and the contract list. The contracts are behavioural, so check the
  implementation and the component's tests, not just the type — `loading` and `busy` in
  particular are only correct if the control stays focusable and declines activation.
- **Failure:** a documented variant or prop the component no longer has, a new primitive
  §4 omits, or a contract in the list that the component no longer honours.
- **§4 documents behaviour, not styling.** Every primitive is Grove now and none carries a
  stylesheet, so §4's job is the contracts: what a prop promises, which variant fixes an ARIA
  role, what is a type error. A restyle that preserves every contract is not drift against
  this anchor, and a styling question belongs to anchor 6.

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
  (whisper needs MSVC + `LIBCLANG_PATH`). Also confirm the feature set release builds ship
  with (`parakeet,embed`, via `package.json`'s `tauri:build` script — the canonical
  definition, which `.github/workflows/release.yml` invokes rather than restating) matches
  what `CLAUDE.md`, `README.md` and the two app jobs in `ci.yml` claim, and that the
  `compile_error!` guard in `src-tauri/src/transcribe.rs` still names an engine.
  `embed` has **no** guard of its own, so the script is the only thing pinning it: a
  `tauri:build` that lost `embed` would ship an exe with no semantic search and nothing
  would fail. The release's bundle step adds one flag the script does not carry,
  `--config src-tauri/tauri.updater.conf.json`; that overlay is CI-only on purpose
  (`createUpdaterArtifacts` makes the CLI demand `TAURI_SIGNING_PRIVATE_KEY`), so confirm
  it has not migrated into `tauri.conf.json` or `tauri.bundle.conf.json`, which would
  break every local `pnpm tauri:build`.
- **Failure:** a feature CI checks that `CLAUDE.md`'s commit instructions don't
  mention.

## Anchor 6 — The Grove theme ↔ docs/DESIGN_SYSTEM.md

- **Source of truth:** the `@theme` block and the `.day` / `.hc` blocks in
  `src/index.css` (the tokens, the keyframes, the two variants), plus the three Grove
  guards and the copy guard in `eslint.config.js`'s `no-restricted-syntax` block
  (colour literals, `.css` imports, the reduced-motion partner, the em dash).
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
- **Almost nothing here is machine-checked, unlike the pre-Grove anchor.** The old
  token guard asserted theme-block coverage in `pnpm test`; Grove retired it along with
  the stylesheets it scanned. The one exception is half of verify-item 1:
  `src/motionGuardParity.test.ts` pins the *moving* column of §4's swap table to the
  eslint motion guard's token list, in both directions, so a moving animation added to
  the table without a guard (or vice versa) fails `pnpm test`. It says nothing about
  the partner column, nothing about the opacity-only tokens, and nothing about
  `@keyframes`-to-`--animate-*` coverage — so item 1's other halves, and every check
  above, are still this auditor's job in full.
- **`src/index.css` should be the only stylesheet in the repo.** The pre-Grove layer
  (`design/tokens.css`, the `@theme inline` bridge, the per-component `*.css` files,
  `src/fonts.ts` and the `@fontsource` dependencies) was deleted by the Grove cleanup.
  A second stylesheet reappearing is itself a finding: it reopens the token-shadowing
  trap in §7, since unlayered CSS outranks `@layer theme`. `design/` keeps only the
  Phase-0 moodboard and spirit-mark pages, which no build reads and which
  `docs/DESIGN.md` already labels as drifted.

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

## Anchor 8 — Dev sandbox state map ↔ the resolvers

- **Source of truth:** `crates/kodabi-core/src/sandbox.rs` (the layout constants
  `INDEX_SUBDIR`/`INDEX_FILE`/`WEBVIEW2_SUBDIR`/`DEV_SUFFIX`, the switch keywords, and
  the three `SandboxError` variants), `src-tauri/src/sandbox.rs` (which environment
  variables `install` writes, and `config_dir`), and every `KODABI_*` env-var const in
  `src-tauri/` and `crates/`.
- **Mirror:** `docs/DEV_SANDBOX.md` — the state-map table (state → default location →
  resolver → sandboxed location), the switch's value grammar, and the refusal table's
  three messages. Also the `pnpm dev:sandbox` line in `README.md`'s script list and its
  "Dev sandbox" section, and the *Isolation* section of `e2e/README.md`. And
  `README.md`'s "Where your data lives, and what uninstalling does" section — the
  user-facing half of the same map (the `%APPDATA%\com.kodabi.app` folder and its
  contents table), which states the *unsandboxed* defaults only.
- **Verify:**
  1. Every path in the state-map table still resolves the way the table says: grep the
     four `sandbox::config_dir` call sites (`lib.rs` setup, `terminal_cmds`'s
     `write_mcp_config` and `write_config_files`, and `ledger_state`'s `open_ledger`)
     and confirm no `app_config_dir()` call has reappeared outside `sandbox.rs` itself.
     That seam is the *only* thing sandboxing the config dir — there is no
     relocate-by-directory sweep — so a new file resolved any other way silently writes
     into the user's real data under `KODABI_SANDBOX`.
  2. The derived subpaths in the doc match the constants (`.index/index.db`,
     `.webview2`, the `-dev` suffix), and `indexDbFor()` in `e2e/lib/vault.mjs` still
     computes the same index path as `INDEX_SUBDIR`/`INDEX_FILE` — the two
     cross-reference each other and are the pair most likely to drift.
  3. The three refusal messages quoted in the doc match the `#[error(...)]` strings.
  4. `e2e/lib/app.mjs` still sets `KODABI_SANDBOX` and no second isolation mechanism has
     grown beside it.
  5. `README.md`'s data-location table still names every user-visible thing the state map
     puts under `app_data_dir()` (notes, `sessions/`, `chats/`, `index.db`, `ledger.db`
     and its `_ledger.yml` snapshots, `settings.toml`, `MODELS_SUBDIR`), and still names
     the WebView2 profile as the one thing living outside it. It also still distinguishes
     the *derived* `index.db` (safe to delete) from the *durable* `ledger.db` (not
     derived; backed up as `_ledger.yml` in each project folder) — collapsing the two
     into one "derived" claim would tell a user it is safe to delete their commitments. Its uninstall warning still matches the bundler: no
     `bundle.windows.nsis` block carries a custom `template` or `installerHooks` — check
     `src-tauri/tauri.conf.json` **and** the release overlays merged over it
     (`tauri.bundle.conf.json`, plus CI's `tauri.updater.conf.json`), since the bundle
     overlay already carries a `bundle.windows` block and is where a bundle-time hook
     would naturally land — so what ships is the stock NSIS template, whose
     delete-app-data checkbox `RmDir /r`s **both** `$APPDATA\<identifier>` and
     `$LOCALAPPDATA\<identifier>` — the vault with them. A custom template, an installer
     hook, or a Tauri upgrade that changes either the checkbox or the forced webview
     `data_directory` invalidates that section.
- **Failure:** a state location the table omits or misattributes, a layout constant the
  doc contradicts, a refusal message that has been reworded in only one place, a
  config-dir call site bypassing the seam, or a README data-location claim that has
  drifted from the resolvers.

---

**Adding an anchor:** add its section here *and* the one-line entry in the
doc-auditor agent's anchor summary — the agent works from that summary, so an anchor
present in only one of the two files goes unchecked.
