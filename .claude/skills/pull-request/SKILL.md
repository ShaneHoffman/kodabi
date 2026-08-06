---
name: pull-request
description: Open a GitHub pull request against main with a Conventional-Commit title and a structured, best-practice description. Used by the Open PR board column.
disable-model-invocation: true
argument-hint: [optional extra context — issue refs, emphasis, or a title hint]
---

# Open a pull request

Open a GitHub pull request for the **current branch** targeting **`main`**, following the steps below precisely.

Hard rules:
- **Never merge** the PR and **never push to `main`.** Your job ends when the PR is open.
- One PR per branch — if one already exists, update it rather than opening a duplicate.

Extra context from the caller (may be empty): $ARGUMENTS

## 1. Understand the change (read-only)
Run and actually read these before writing anything:
- `git branch --show-current` — the head branch.
- `git log --oneline main..HEAD` — the commits this PR introduces.
- `git diff --stat main...HEAD` — files touched and size.
- `git diff main...HEAD` — skim the real changes for intent, risky areas, and anything a reviewer must know.

If the current branch **is** `main`, or there are **no** commits vs `main`, STOP and report that there's nothing to open a PR for.

## 2. Check for unexpected uncommitted changes

Run `git status --short`. The tree should normally be **clean**: the Code Review session runs
`/code-review-fix`, which commits its own fixes on the branch (the `fix: fix code-review findings`
pattern in git history), so the commits you see in step 1 account for the whole branch.

Uncommitted changes here are therefore **unexpected** — a review session that crashed before
committing, an older review that only edited files, or stray edits from another session sharing
this worktree. Don't assume you forgot to commit your own work, and don't assume they're safe:
investigate before doing anything with them.

- If the tree is clean, continue to step 3.
- If there are uncommitted changes: `git diff` them and read them, then summarize what they do for
  the user and ask (via `AskUserQuestion`) whether to commit them as part of this PR. Never commit
  them unilaterally — only you originating a commit without asking is authorized by the board
  flow, not a second party's silent edits.
- If the user confirms, commit them as **their own commit**, separate from the original work, so
  the review trail stays legible — don't fold them into an earlier commit via amend. Use the
  sanctioned review-commit subject from [`commit`](../commit/SKILL.md): `fix: fix code-review
  findings` (`docs:` when the fixes are docs-only), regardless of the branch's own prefix.
- If the user declines, stop and ask how they'd like to proceed before continuing — don't silently
  push or open a PR that omits changes currently sitting in the working tree.

## 3. Verify it builds
CI's `app` job release-builds the desktop app, but only when a Rust surface changed. This step
confirms it compiles and links on Windows unconditionally, before anything is pushed:
- `pnpm install --frozen-lockfile` — installs frontend deps (worktrees start without `node_modules`).
- `pnpm tauri:build --no-bundle` — release compile + link of the desktop app in its shipping
  configuration (`--features parakeet,embed`; no NSIS packaging). This one command covers
  everything: it runs `pnpm build` itself (the `beforeBuildCommand` in `tauri.conf.json`,
  generating the `dist/` that `tauri::generate_context!` embeds) and release-compiles the full
  Rust workspace. Use the `tauri:build` script, not a bare `pnpm tauri build`: a release build
  without an engine feature is rejected by the `compile_error!` guard in
  `src-tauri/src/transcribe.rs`.

If this fails, STOP — fix the build first. Do not push or open/update a PR for code that
doesn't build.

## 4. Push the branch
`git push -u origin HEAD` (sets upstream; a no-op if already current).

## 5. Check for an existing PR
`gh pr list --head "<branch>" --state open`. If a PR already exists: report its URL, refresh the title/body only if clearly stale, then skip to step 9. Do **not** create a second PR.

## 6. Title — Conventional Commit
`<type>: <imperative summary>`
- `<type>` matches the branch prefix: `feat | fix | chore | docs | refactor | ci` (e.g. branch `feat/scaffold-tauri-app` → `feat:`). If the branch has no recognized prefix, infer the best-fitting type from the diff.
- Imperative mood, ≤ ~70 chars, no trailing period. Describe the change, not the files touched.

## 7. Description — write to a temp file, pass with `--body-file`
Use a temp file (robust across shells / multiline). Structure, omitting any section that would be empty:

```
## Summary
<1–3 sentences: what this PR does and, first, WHY. Lead with the motivation.>

## Changes
- <key change>
- <key change>

## Test plan
- <how you verified, or how a reviewer can: commands run, cases covered>
- <anything deliberately NOT covered>

## Notes
<optional: risks, breaking changes + migration, follow-ups, `Closes #N` for issues>

🤖 Generated with [Claude Code](https://claude.com/claude-code)
```

Best practices: explain intent over mechanics, keep it scannable, link issues with `Closes #N` when applicable, and call out breaking changes / migrations explicitly.

## 8. Create the PR
`gh pr create --base main --head "<branch>" --title "<title>" --body-file <tempfile>`
Add `--draft` only if the work is explicitly WIP. Capture and print the PR URL.

## 9. Link to the board task (if applicable)
If you're running inside a Kangentic task, link the new PR to it with the `kangentic_link_pr` tool so the board tracks it. If that tool isn't available, skip silently.

## 10. Check for and resolve merge conflicts
A clean-looking diff in step 1 does **not** guarantee the PR can merge: `git log --oneline main..HEAD`
only shows commits unique to your branch — it says nothing about commits your branch is *missing*
from `main` (check `git log --oneline HEAD..origin/main` if you want that view directly). A local
`main` that hasn't been fetched recently hides this; other PRs can land on `main` after your branch
diverged and still conflict with it.

- `git fetch origin main`
- `gh pr view --json mergeable,mergeStateStatus` (no argument — resolves the PR for the current
  branch, so you don't need the PR number). `mergeable` starts as `UNKNOWN` right after a push
  while GitHub computes it; wait a few seconds and retry. If it's still `UNKNOWN` after a couple of
  retries, don't assume it's clean — fall through to the `CONFLICTING` steps below (a local
  `git merge origin/main` will show the truth) or ask the user.
- If `mergeable` is `MERGEABLE`: no textual conflict. Continue to step 11. Note that a clean
  *textual* merge doesn't guarantee a clean *build* — if `main` has moved substantially since
  step 3 (e.g. a signature or symbol this branch uses changed without touching the same lines), a
  semantic break can slip through, and CI only builds the Tauri app when a Rust surface changed
  (see step 3). When main has advanced non-trivially, re-run step 3's build to be sure before
  continuing.
- If `mergeable` is `CONFLICTING`:
  1. `git merge origin/main --no-edit` to surface the conflicting files locally.
  2. Resolve each conflict on its actual merits — don't blindly keep "ours" or "theirs". A conflict
     where both sides purely *added* new, non-overlapping content (e.g. two new functions or test
     modules appended at the same location) usually means keeping both, in a sensible order. A
     conflict where the same logic changed on both sides needs a real judgment call — if the right
     resolution isn't clear, ask the user rather than guessing.
  3. Stage each resolved file (`git add <file>`), then confirm no markers remain anywhere:
     `grep -rn '^<<<<<<<\|^|||||||\|^=======\|^>>>>>>>'` (excluding build output). The `|||||||`
     alternative catches the base-section marker left by `diff3`/`zdiff3` conflict styles.
  4. Re-run **step 3's build verification** (`pnpm tauri:build --no-bundle`) — conflict resolution
     can silently break the compile/link step 3 already validated once. If your resolution touched
     Rust, also run the pre-commit gates from the repo `CLAUDE.md` (`cargo fmt --all --check`,
     `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace
     --locked`) before committing, since step 3's build alone doesn't run fmt, clippy, or tests.
  5. `git commit --no-edit` to complete the merge, then `git push`.
  6. Re-check `gh pr view --json mergeable` until `mergeable` reads `MERGEABLE`. If it flips back to
     `CONFLICTING` (another PR can land on `main` between your fetch and push), return to sub-step 1
     and resolve again. A `mergeStateStatus` of `UNSTABLE` once `mergeable` is `MERGEABLE` just
     means CI is still running on the fresh push — not a conflict.

## 11. Watch CI and fix failures
Once the PR is open (and mergeable), hand the human a green PR rather than a pending one:
- `gh pr checks --watch` — wait for the CI checks on this PR to settle.
- If everything passes, continue to step 12.
- If a check fails: find the failed run id with
  `gh run list --branch <branch> --limit 10 --json databaseId,conclusion,workflowName`
  (`<branch>` from `git branch --show-current`), then read its log with
  `gh run view <run-id> --log-failed` — the bare `gh run view --log-failed` needs a run id and
  errors non-interactively. Reproduce and fix
  locally, then run the relevant pre-commit gates for the surface you touched (the `CLAUDE.md`
  gates — `cargo fmt`/`clippy`/`test`, and `pnpm exec eslint . --max-warnings=0` + `pnpm test` +
  `pnpm build` for frontend). Commit the fix as a **new** commit (never amend), then `git push` (plain push;
  use `--force-with-lease` only if you had to rebase onto `main`, never a bare `--force`).
- **Max 3 fix cycles.** If checks are still red after the third, stop and report the failure to
  the user with the failing log — don't keep looping.
- **Never bypass a failing check and never merge.** `--admin`, merge-queue overrides, and any
  "skip checks" path are out of bounds; your job is a green PR, not a merged one.

## 12. Stop
Report the PR URL and a one-line summary of what shipped. Do not merge — a human reviews and merges.
