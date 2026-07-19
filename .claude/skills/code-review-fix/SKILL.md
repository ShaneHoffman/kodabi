---
name: code-review-fix
description: Review this branch's diff against main at high rigor, fix the real in-scope findings, run the pre-commit gates, and commit on the task branch. Used by the Code Review board column. Never pushes.
disable-model-invocation: true
argument-hint: [optional focus areas or extra context]
---

# Review and fix the branch

Independently review `git diff origin/main...HEAD`, then remediate it — in one session, so the
branch reaches Open PR in its final state with a history that session can explain.

Hard rules:
- **Never push.** Pushing happens only in `/pull-request` (the Open PR board column).
- **Never amend** — review fixes are always a new commit, so the review trail stays
  legible.
- **Fix only what the review found, only within this branch's scope.** Anything larger
  is recorded as a skip with a reason, for the human gate to send back to Executing.
- **Report only findings that survive adversarial verification** — every finding needs
  a concrete failure scenario you actually traced, not a suspicion.
- **Never commit over a red gate** (`/commit` enforces this; it is load-bearing here).
- **Commit only files you changed.** This column runs unattended with auto-accepted
  permissions, so an edit you did not make must never ride along in your commit.
- **Do not move the card.** The human gate decides what happens next.

Extra context from the caller (may be empty): $ARGUMENTS

## 1. Scope the diff (read-only)

**Pick the base ref first.** This worktree's local `main` is often behind `origin/main`,
and diffing against a stale base silently pulls other branches' already-merged commits
into your review scope. Run `git fetch origin main`, then use **`origin/main`** as the
base (fall back to local `main` only if the fetch fails — and say so in the report).
`$BASE` below means that ref.

`git branch --show-current`, `git log --oneline $BASE..HEAD`,
`git diff --stat $BASE...HEAD`, then read `git diff $BASE...HEAD` in full.

If the branch **is** `main`, or there are **no** commits vs `$BASE`, STOP and report
there is nothing to review.

Also run `git status --short`. The tree should be clean — Executing commits its work.
Anything uncommitted is unexpected: **record the exact paths** — they are not yours,
they are not branch work, and step 7 must keep them out of your commit. Note them in
the final report rather than reviewing them as if they were branch work.

## 2. Review at high rigor

Correctness first: real bugs, unhandled edge cases, error paths, and regressions in
code the diff touches or meaningfully interacts with. Trace the actual code paths —
reading the diff alone is not a review.

Then the repo's rule surface (follow each link; don't re-derive it here):

- [`no-personal-info`](../../rules/no-personal-info.md) — real emails, real names,
  machine paths, tests writing outside a temp dir. This stage is that rule's
  enforcement point.
- [`utc-timestamps`](../../rules/utc-timestamps.md) — including the frontmatter-`date`
  carve-out, so the sanctioned `Local::now()` in quick capture is not flagged.
- [`copy-style`](../../rules/copy-style.md) — user-facing copy only.
- [`tauri-command-parity`](../../rules/tauri-command-parity.md) — when `src-tauri/**`
  or `src/**` changed.
- [`typescript-style`](../../rules/typescript-style.md) — when `src/**` changed. The
  eslint gate does not encode this rule's judgment calls, so review is where it lands.
- [`shell-discipline`](../../rules/shell-discipline.md) — when the diff adds scripts or
  agent-facing command guidance.
- [`docs-stay-in-sync`](../../rules/docs-stay-in-sync.md) — does the diff invalidate a
  doc claim or anchor?
- `CLAUDE.md` engineering rules — design tokens (no hard-coded color/font/spacing) and
  core vs shell (a Tauri command that grew a body is a finding).
- Tests — new behavior without coverage at the tier the `/test` skill would use.

Order findings by severity.

## 3. Verify adversarially

For each candidate, try to falsify it: re-read the surrounding code, construct the
concrete input or sequence that triggers the failure, and check whether an existing
guard or test already covers it. Drop what does not survive.

Give each survivor a verdict: `CONFIRMED` (failure path fully traced) or `PLAUSIBLE`
(strong, not fully traced).

## 4. Report the findings

Call the `ReportFindings` tool with the verified list, most severe first. If that tool
is unavailable (running outside the board), emit the same list as a table instead.

**No findings:** report the review clean and stop here — a clean review makes no commit,
so steps 5-8 do not apply. Do not call `ReportFindings` again.

## 5. Triage

Decide *fix* or *skip* per finding, and record a reason for every skip:

- **Fix** when it is real and inside this branch's scope.
- **Skip** when it is pre-existing on `main` or otherwise out of this branch's scope;
  when it is too large or design-shaped to fix safely during review — say so
  explicitly and recommend **Request changes → Executing**; or when it is intentional
  per a documented carve-out.

Never widen the branch's scope to chase a finding.

## 6. Fix

Apply the fixes under the same conventions Executing works under (`CLAUDE.md` and the
rules above). A behavioral fix gets a test where that surface has a test home.

## 7. Gates and commit — delegate to `/commit`

First, list the paths **you** edited in step 6. If step 1 found foreign uncommitted
files, `/commit` cannot tell them from your fixes — it reads the whole working tree —
so you must name your paths explicitly and tell it to stage only those, leaving the
foreign edits in the tree for the human gate. If step 1 found none, your paths are the
whole tree and this is a no-op.

Invoke the `commit` skill with the message hint `fix: fix code-review findings`
(`docs:` when the fixes are docs-only) **plus that path list**. It owns the
surface→gate matrix, the fix-and-re-run loop, explicit staging, and the never-push
rule — do not restate or duplicate any of it here.

Afterwards, `git status --short` must still show every foreign path from step 1,
untouched. If it doesn't, you committed someone else's work: say so prominently in
step 8.

## 8. Re-report and stop

Call `ReportFindings` a second time with each finding's `outcome` (`fixed`, `skipped`,
or `no_change_needed`) — or, if that tool is unavailable, carry the same outcomes in the
written summary. Then report: findings found, fixed, skipped with their reasons,
and the commit hash and subject — or that the review was clean and no commit was made.
Call out any skip that warrants sending the card back to Executing.

**Do not push. Do not move the card.**
