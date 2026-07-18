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
put a branch-bearing card in the Backlog column, **omit `column`**: the board's To Do lane *is*
"Backlog", so the task lands there as a proper board task with the branch honored. The create
response never echoes the branch, so verify with
`SELECT branch_name FROM tasks WHERE display_id = <N>` (via `kangentic_query_db`) — and note that
`kangentic_update_task` has no `branchName` field, so the only fix is delete + recreate.

## Board flow

`Backlog → Planning → Executing → Code Review → Open PR → Done`

(The Executing column is literally named **"Write Me Code"** on the board — use that name in any
`kangentic_move_task` call; "Executing" below refers to that column.)

- **Manual gate between every stage** — a human drags each card onward. The one exception:
  **Planning → Executing auto-advances on plan approval** (plan approval *is* the gate there).
- **Executing** implements the task and **commits on the task branch — but never pushes.**
- **Code Review** is a fresh, independent session running `/code-review high` on `git diff main...HEAD`.
- **Open PR** runs the `/pull-request` skill: push → `gh pr create --base main` → `kangentic_link_pr`.
  **It never merges** — a human merges on GitHub, then drags the card to Done.
- **Never drag a card back to Backlog** — that kills the session and removes its worktree.
  "Request changes" from Code Review goes back to **Executing**.

Commit subjects follow Conventional Commits: `<type>: <imperative summary>`, matching the branch's
`type` (branch `feat/scaffold-tauri-app` → `feat: scaffold Tauri app shell`).

## Engineering rules

- **Pre-commit gates (mirror CI exactly):** `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets --locked -- -D warnings`, and
  `cargo test --workspace --locked` must pass before every commit that touches Rust;
  `pnpm exec eslint . --max-warnings=0` and `pnpm build` before commits that touch the frontend.
  The clippy/test gates need `dist/` to exist — `src-tauri` embeds it via
  `tauri::generate_context!`, which fails the compile when it's missing — so in a fresh worktree
  run `pnpm install --frozen-lockfile && pnpm build` first (CI's Rust jobs do the same).
  `kodabi-transcribe`'s `parakeet` feature (sherpa-onnx) and `whisper` feature (whisper.cpp via
  whisper-rs) are off by default, so the gates above don't compile or lint them — before committing
  a change under `crates/kodabi-transcribe`, also run
  `cargo clippy -p kodabi-transcribe --features parakeet --all-targets --locked -- -D warnings` and
  `cargo clippy -p kodabi-transcribe --features whisper --all-targets --locked -- -D warnings`.
  The `whisper` feature compiles whisper.cpp from source (CMake + bindgen/libclang), so it needs an
  MSVC dev environment (`vcvars64.bat`) sourced and `LIBCLANG_PATH` set to an LLVM install — see
  `crates/kodabi-transcribe/src/whisper.rs`. `whisper-cuda` additionally needs the CUDA toolkit and
  is local-only (CI only checks the CPU `whisper` feature).
- **Core vs shell:** logic lives in `crates/kodabi-core` (pure, UI-agnostic, unit-testable);
  `src-tauri` commands stay thin wrappers around it. If a Tauri command grows a body, the body
  belongs in kodabi-core.
- **Design tokens:** never hard-code a color, font, or spacing value. `design/tokens.css` is the
  single source of truth, bridged into Tailwind by `src/index.css` — consume tokens, never
  duplicate them.
- **Spec agreement:** `docs/FRONTMATTER_SCHEMA.md` and `docs/MCP_TOOL_SURFACE.md` mirror each
  other (frontmatter fields ≡ the MCP `NoteSummary` shape). Editing one requires checking the
  other in the same change.
- **User-facing copy:** no em dashes (the `—` character) in any language the user reads: UI
  strings, labels, captions, tray text, and user-visible error or status messages. Rewrite with a
  period, comma, colon, or parentheses instead. This is a copy rule only; code comments and repo
  docs are unaffected.
