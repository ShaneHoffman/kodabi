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

## 2. Verify it builds
CI only runs lint and tests — it does not build the Tauri app. This step is the one place that
actually confirms the desktop app compiles and links on Windows, so it runs before anything is
pushed:
- `pnpm install --frozen-lockfile && pnpm build` — installs frontend deps and generates `dist/`
  (required before Rust can compile `src-tauri`, which embeds it via `tauri::generate_context!`).
- `cargo build --workspace --locked` — compiles the full Rust workspace.
- `pnpm tauri build --no-bundle` — release compile + link of the desktop app (no installer
  packaging), confirming it actually builds on Windows.

If any of these fail, STOP — fix the build first. Do not push or open/update a PR for code that
doesn't build.

## 3. Push the branch
`git push -u origin HEAD` (sets upstream; a no-op if already current).

## 4. Check for an existing PR
`gh pr list --head "<branch>" --state open`. If a PR already exists: report its URL, refresh the title/body only if clearly stale, then skip to step 8. Do **not** create a second PR.

## 5. Title — Conventional Commit
`<type>: <imperative summary>`
- `<type>` matches the branch prefix: `feat | fix | chore | docs | refactor | ci` (e.g. branch `feat/scaffold-tauri-app` → `feat:`). If the branch has no recognized prefix, infer the best-fitting type from the diff.
- Imperative mood, ≤ ~70 chars, no trailing period. Describe the change, not the files touched.

## 6. Description — write to a temp file, pass with `--body-file`
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

## 7. Create the PR
`gh pr create --base main --head "<branch>" --title "<title>" --body-file <tempfile>`
Add `--draft` only if the work is explicitly WIP. Capture and print the PR URL.

## 8. Link to the board task (if applicable)
If you're running inside a Kangentic task, link the new PR to it with the `kangentic_link_pr` tool so the board tracks it. If that tool isn't available, skip silently.

## 9. Stop
Report the PR URL and a one-line summary of what shipped. Do not merge — a human reviews and merges.
