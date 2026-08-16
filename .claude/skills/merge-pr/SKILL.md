---
name: merge-pr
description: Merge the current branch's reviewed, green pull request into main with a merge commit, then delete the remote branch. Used by the Merge PR board column.
disable-model-invocation: true
argument-hint: [optional extra context — e.g. a PR number if branch resolution fails]
---

# Merge the pull request

Merge the **current branch's** open PR into **`main`**, following the steps below precisely. A
human dragging the card into the Merge PR column (or invoking this skill) **is** the merge
approval — the review already happened on the PR; your job is to verify the PR is actually ready
and then merge it.

Hard rules:
- **Merging is the whole job.** No code changes, no fixes, no new commits, no pushing new work —
  if the PR isn't ready to merge exactly as it stands, STOP and report instead.
- **`--admin` covers exactly one thing: the approval requirement.** The main ruleset demands one
  approving review, which a solo maintainer can never give their own PR; `--admin` bypasses that.
  It must **never** stand in for failing or pending checks — verify the checks yourself first
  (step 3) and stop if they aren't green.
- **Merge commit only** (`--merge`). The repo is merge-commit-only; never `--squash` or
  `--rebase`.
- **Never move the board card.** A human drags it to Done after the merge (that drag kills the
  session and removes the worktree).

Extra context from the caller (may be empty): $ARGUMENTS

## 1. Find the PR
- `git branch --show-current` — if it's `main`, STOP and report there's nothing to merge.
- `gh pr view --json number,url,title,state,isDraft,baseRefName,mergeable,mergeStateStatus` —
  resolves the PR for the current branch, so no number is needed.
- STOP and report if any of: no PR exists (the card needs `/pull-request` first), `state` isn't
  `OPEN`, `isDraft` is true, or `baseRefName` isn't `main`.

## 2. Confirm the PR is the branch's final state
- `git status --short` — expect a clean tree.
- `git fetch origin` then `git log --oneline origin/<branch>..HEAD` — expect no unpushed commits.

Uncommitted changes or unpushed commits mean the PR on GitHub is **not** what this worktree
holds, so merging would ship something other than what was reviewed. Don't commit or push them
yourself — STOP and report what you found; the card likely needs another pass through
Code Review / Open PR.

## 3. Verify the checks are green
`gh pr checks --watch` — wait for pending checks to settle rather than sampling once.

If any check fails: **STOP — do not merge and do not fix.** Report which check failed with its
log pointer (`gh run list --branch <branch> --limit 10 --json databaseId,conclusion,workflowName`,
then `gh run view <run-id> --log-failed`). Fixing belongs to Executing or the `/pull-request`
session, not this stage — suggest sending the card back to Executing.

## 4. Verify it merges cleanly
Re-run step 1's `gh pr view --json mergeable,mergeStateStatus` if it has been a while:
- `mergeable: MERGEABLE` — continue.
- `mergeable: UNKNOWN` — GitHub is still computing; wait a few seconds and retry a couple of
  times.
- `mergeable: CONFLICTING` — STOP. Conflict resolution is `/pull-request`'s job (its step 10);
  report and suggest re-running that stage.
- `mergeStateStatus: BLOCKED` is **expected** — that's the un-satisfiable approval rule `--admin`
  exists for. `BEHIND` still merges (the merge commit incorporates main), but say so in the final
  report: the green checks ran before main's newest commits, so they didn't test this exact
  merge result.

## 5. Merge
- `gh pr merge <number> --merge --admin`
- Confirm it landed: `gh pr view <number> --json state,mergedAt,mergeCommit` — `state` must read
  `MERGED`. If the merge command errored or the state isn't `MERGED`, report the exact error and
  stop; never retry with a different flag set.

## 6. Delete the remote branch
`git push origin --delete <branch>` — if it fails with "remote ref does not exist", GitHub
already auto-deleted it; that's fine. Never delete the **local** branch or the worktree —
dragging the card to Done handles local cleanup, and this session is standing in that worktree
right now.

(Deliberately not `gh pr merge --delete-branch`: that flag also tries to delete and switch the
local branch, which misbehaves inside a worktree.)

## 7. Report
- If running inside a Kangentic task and the PR isn't linked yet, link it with
  `kangentic_link_pr` (normally `/pull-request` already did; skip silently if the tool isn't
  available).
- Report: the PR URL, the merge commit SHA, a one-line summary of what shipped, any `BEHIND`
  caveat from step 4 — and remind the human that dragging the card to Done removes the worktree.
