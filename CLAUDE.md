# Kodabi — agent guide

Kodabi is a Windows desktop app (Tauri + Rust backend, React + Tailwind frontend, AGPL-3.0) that
turns meeting transcripts into routed, searchable Markdown notes, with Claude Code / MCP over the
knowledge base. Vision + architecture live in `docs/FOUNDING_DOC.md`; the working roadmap is
`docs/ROADMAP.md`.

Development runs on a **Kangentic board**, so how you create work and name branches matters.

## Creating tasks & branches

**Create board tasks with the `kangentic_create_task` MCP tool** — not the built-in todo system.
The board is the source of truth for work; a local todo list is not.

**ALWAYS pass `branchName`, formatted as `type/slug`:**

- `type` is a Conventional-Commit type: `feat | fix | chore | docs | refactor | ci`.
- `slug` is a short kebab-case summary. **No task ID.**
- Examples: `feat/scaffold-tauri-app`, `docs/frontmatter-schema`, `chore/adopt-git-flow`.

**Never omit `branchName`.** Kangentic ships no branch-name policy, so an omitted name
auto-defaults to `{slug}-{taskId8}` (e.g. `adopt-agpl-3-0-licen-5fa38daf`). That name **locks the
moment the task leaves To Do** (its worktree materializes) and **cannot be renamed** afterward
without desyncing PR tracking — `kangentic_link_pr` resolves PRs via `gh pr list --head <branch>`,
so a rename permanently breaks the link. Set it correctly up front; there is no clean fix later.

**Passing `branchName` is not enough — the `column: "Backlog"` argument silently drops it.**
`kangentic_create_task` routes `column: "Backlog"` (case-insensitive) through its backlog-creation
path, which **ignores `branchName`**, leaving `branch_name` NULL — the auto-default trap above. To
put a branch-bearing card in the board's To Do lane, **omit `column`**: that lane is named
**"Ready"** (`role: todo` in `kangentic.json`), and omitting `column` lands the task there as a
proper board task with the branch honored. Note the asymmetry: "Backlog" is *not* a column name at
all — it is the literal argument value that diverts creation into the separate backlog store. The create
response never echoes the branch, so verify with
`SELECT branch_name FROM tasks WHERE display_id = <N>` (via `kangentic_query_db`) — and note that
`kangentic_update_task` has no `branchName` field, so the only fix is delete + recreate.

## Board flow

`Ready → Planning → Executing → Code Review → Open PR → Done`

(Two column names differ from the stage names used below — pass the **board** name to any
`kangentic_move_task` call: the To Do lane is **"Ready"**, and the Executing column is literally
named **"Write Me Code"**.)

- **Manual gate between every stage** — a human drags each card onward. The one exception:
  **Planning → Executing auto-advances on plan approval** (plan approval *is* the gate there).
- **Executing** implements the task and **commits on the task branch — but never pushes.**
- **Code Review** is a fresh, independent session running the `/code-review-fix` skill: it reviews
  `git diff origin/main...HEAD` at high rigor, fixes the real in-scope findings, runs the gates, and
  **commits on the task branch — but never pushes.** Findings too large to fix during review are
  reported as skips for the human gate.
- **Open PR** runs the `/pull-request` skill: push → `gh pr create --base main` → `kangentic_link_pr`.
  **It never merges** — a human merges on GitHub, then drags the card to Done.
- **Never drag a card back to Ready** — that kills the session and removes its worktree.
  "Request changes" from Code Review goes back to **Executing**.

Commit subjects follow Conventional Commits: `<type>: <imperative summary>`, matching the branch's
`type` (branch `feat/scaffold-tauri-app` → `feat: scaffold Tauri app shell`). One sanctioned
exception: Code Review's own remediation commit is `fix: fix code-review findings` (`docs:` when
the fixes are docs-only) whatever the branch prefix, so review-driven corrections read as such.

## Engineering rules

- **Pre-commit gates (mirror CI exactly):** `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets --locked -- -D warnings`, and
  `cargo test --workspace --locked` must pass before every commit that touches Rust;
  `pnpm exec eslint . --max-warnings=0`, `pnpm test`, and `pnpm build` before commits that touch
  the frontend.
  The clippy/test gates need `dist/` to exist — `src-tauri` embeds it via
  `tauri::generate_context!`, which fails the compile when it's missing — so in a fresh worktree
  run `pnpm install --frozen-lockfile && pnpm build` first (CI's Rust jobs do the same).
  `kodabi-transcribe`'s `parakeet` feature (sherpa-onnx), `vad` feature, and `whisper` feature
  (whisper.cpp via whisper-rs) are off by default, so the gates above don't compile or lint them —
  before committing a change under `crates/kodabi-transcribe`, also run
  `cargo clippy -p kodabi-transcribe --features parakeet --all-targets --locked -- -D warnings`,
  `cargo clippy -p kodabi-transcribe --features vad --all-targets --locked -- -D warnings`, and
  `cargo clippy -p kodabi-transcribe --features whisper --all-targets --locked -- -D warnings`
  (CI checks all three legs as a matrix).
  The `whisper` feature compiles whisper.cpp from source (CMake + bindgen/libclang), so it needs an
  MSVC dev environment (`vcvars64.bat`) sourced and `LIBCLANG_PATH` set to an LLVM install — see
  `crates/kodabi-transcribe/src/whisper.rs`. `whisper-cuda` additionally needs the CUDA toolkit and
  is local-only (CI only checks the CPU `whisper` feature).
  Likewise `kodabi-embed`'s `bge` feature (bge-small via fastembed/ONNX Runtime) is off by default —
  before committing a change under `crates/kodabi-embed` **or `crates/kodabi-core`** (CI's embed job
  path-filters on both, since the `bge` backend has a `BGE_DIM == EMBEDDING_DIM` compile-time
  assert against core), also run
  `cargo clippy -p kodabi-embed --features bge --all-targets --locked -- -D warnings`. No MSVC or
  bindgen is needed, but the first build downloads the ONNX Runtime binary (`ort-download-binaries`).
  The model itself is never fetched at runtime — set `KODABI_EMBED_MODEL_DIR` to a local
  bge-small-en-v1.5 directory to exercise the `#[ignore]`d integration tests.
- **Core vs shell:** logic lives in `crates/kodabi-core` (pure, UI-agnostic, unit-testable);
  `src-tauri` commands stay thin wrappers around it. If a Tauri command grows a body, the body
  belongs in kodabi-core.
- **Frontend tests:** vitest + Testing Library under jsdom, colocated as
  `src/**/*.test.{ts,tsx}` and run by `pnpm test`. Mock **only** the Tauri IPC boundary — the
  `src/test/tauri.ts` harness stands in for `@tauri-apps/api`'s `invoke`/`listen`, and the
  component under test keeps its real hooks. Coverage is the load-bearing seams, not the whole UI.
- **Design tokens:** never hard-code a color, font, or spacing value. `design/tokens.css` is the
  single source of truth, bridged into Tailwind by `src/index.css` — consume tokens, never
  duplicate them.
- **Spec agreement:** `docs/FRONTMATTER_SCHEMA.md` and `docs/MCP_TOOL_SURFACE.md` mirror each
  other (frontmatter fields ≡ the MCP `NoteSummary` shape). Editing one requires checking the
  other in the same change.
- **UTC storage:** internal and derived timestamps are UTC RFC 3339 (`Z`); the frontmatter `date`
  field is the sanctioned exception (offset-preserving or local date-only). See
  `.claude/rules/utc-timestamps.md`.
- **Public repo:** no real personal data, emails, or machine paths in committed files, and tests
  write only under temp dirs. See `.claude/rules/no-personal-info.md`.

Topical rules that aren't repo-wide engineering constraints live as modular files under
`.claude/rules/`: `copy-style` (no em dashes in user-facing copy), `shell-discipline`,
`docs-stay-in-sync`, `tauri-command-parity`, `no-personal-info`,
`no-use-effect` (effects only in blessed bridge hooks), `skill-authoring`,
`typescript-style`, `utc-timestamps`.

## Skills & agents

Task-shaped workflows live under `.claude/skills/`:

- `frontmatter-validator` — validate a note's YAML frontmatter against the schema.
- `preview` — launch Tauri dev and smoke-test the app.
- `pull-request` — open a PR against main (Open PR board column; never merges).
- `add-tauri-command` — scaffold a command across all layers, then audit parity.
- `add-migration` — append a note-index migration safely, then audit.
- `commit` — run the gates for the changed surface, then commit (never pushes).
- `code-review-fix` — review the branch diff, fix the in-scope findings, then gate and commit via
  `commit` (Code Review board column; never pushes).
- `scaffold-feature` — plan and build a full-stack feature bottom-up.
- `sync-docs` — reconcile docs with code via the anchor list.
- `test` — run the tiers, or delegate audit/write to `test-builder`.

Read-only auditor agents live under `.claude/agents/` and are spawned by the skills above:

- `doc-auditor` — docs match code and each other.
- `tauri-command-auditor` — command layers stay in lockstep; wrappers stay thin.
- `migration-safety` — migrations are append-only, tested, and schema-aligned.
- `test-builder` — the one exception: Rust-first, and it can write tests.

The skill→agent delegation map lives in `.claude/rules/skill-authoring.md`.
