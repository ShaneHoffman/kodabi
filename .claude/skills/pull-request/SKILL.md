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

## 2. Check for uncommitted changes from an independent code review

Run `git status --short`. **Expect to sometimes find uncommitted changes here that you did not
make.** This worktree can be shared with a separate, independent Code Review session that applies
fixes directly to files without committing (per this repo's board flow — see the
"chore: fix code-review findings from ..." pattern in git history). Finding modified files you
don't recognize authoring is normal, not a sign you forgot to commit your own work.

- If the tree is clean, continue to step 3.
- If there are uncommitted changes: `git diff` them and read them, then summarize what they do for
  the user and ask (via `AskUserQuestion`) whether to commit them as part of this PR. Never commit
  them unilaterally — only you originating a commit without asking is authorized by the board
  flow, not a second party's silent edits.
- If the user confirms, commit them as **their own commit**, separate from the original work, so
  the review trail stays legible — don't fold them into an earlier commit via amend. Message:
  `<type>: fix code-review findings` (or a more specific summary if one clear theme stands out),
  typically `fix:` regardless of the branch's own prefix, since these are review-driven
  corrections rather than the original feature work.
- If the user declines, stop and ask how they'd like to proceed before continuing — don't silently
  push or open a PR that omits changes currently sitting in the working tree.

## 3. Verify it builds
CI never builds the Tauri desktop app. This step is the one place that actually confirms it
compiles and links on Windows, so it runs before anything is pushed:
- `pnpm install --frozen-lockfile` — installs frontend deps (worktrees start without `node_modules`).
- `pnpm tauri build --no-bundle` — release compile + link of the desktop app (no installer
  packaging). This one command covers everything: it runs `pnpm build` itself (the
  `beforeBuildCommand` in `tauri.conf.json`, generating the `dist/` that
  `tauri::generate_context!` embeds) and release-compiles the full Rust workspace.

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

## 10. Stop
Report the PR URL and a one-line summary of what shipped. Do not merge — a human reviews and merges.
