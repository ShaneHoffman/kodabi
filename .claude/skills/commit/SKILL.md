---
name: commit
description: Commit working-tree changes with the right pre-commit gates run first — matches the changed surface to the CLAUDE.md/CI gates (including feature legs and the dist/ prerequisite), then makes a Conventional Commit matching the branch type. Never pushes.
argument-hint: [optional message hint or scope]
---

# Commit

Snapshot the working tree on the current branch, gates first.

Hard rules:
- **Never push.** Pushing happens only in `/pull-request` (the Open PR board column).
- **Never amend** — always a new commit.
- **Never skip a gate** that the changed surface requires.
- Subject is `<type>: <imperative summary>`, `<type>` matching the branch prefix
  (`feat/…` → `feat:`), per Conventional Commits.

Message hint from the caller (may be empty): $ARGUMENTS

## 1. Understand the change

`git status --short` and `git diff`. Never stage `target/`, `dist/`, or
`node_modules/`. Scan the diff for personal info (real emails, machine paths) per
[`.claude/rules/no-personal-info.md`](../../rules/no-personal-info.md).

## 2. Run the gates for the changed surface

These mirror CI exactly (`CLAUDE.md`). Match every surface the diff touches:

| Changed surface | Gates to run |
| --- | --- |
| Any Rust (`crates/**`, `src-tauri/**`) | Ensure `dist/` exists (else `pnpm install --frozen-lockfile && pnpm build`), then `cargo fmt --all --check` → `cargo clippy --workspace --all-targets --locked -- -D warnings` → `cargo test --workspace --locked` |
| `crates/kodabi-transcribe` | + `cargo clippy -p kodabi-transcribe --features parakeet …`, `--features vad …`, `--features whisper …` (each `--all-targets --locked -- -D warnings`) |
| `crates/kodabi-embed` **or** `crates/kodabi-core` | + `cargo clippy -p kodabi-embed --features bge --all-targets --locked -- -D warnings` |
| Frontend (`src/**`, `index.css`, frontend config) | `pnpm exec eslint . --max-warnings=0` + `pnpm test` + `pnpm build` |
| Docs / `.claude` only | No build gates. `validate.mjs --check-schema` if either schema doc changed; the validator's `test.mjs` if the validator itself changed |

Notes: the `whisper` leg needs an MSVC dev environment (`vcvars64.bat`) and
`LIBCLANG_PATH`. If that environment isn't available here, say so explicitly rather
than silently skipping — CI checks the CPU `whisper` leg regardless.

## 3. Fix and re-run

Fix any failure and re-run the affected gate until clean. Don't commit over a red
gate.

## 4. Commit

Stage explicitly (`git add <paths>`) and commit `<type>: <imperative summary>`. For
a multiline body, write the message with the Write tool to a temp file and
`git commit -F <file>`; end with the `Co-Authored-By: Claude …` trailer when the
repo convention calls for it.

## 5. Stop

Report the commit hash and subject. **Do not push.**
