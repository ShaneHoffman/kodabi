---
name: release
description: Cut a tagged Windows release — bump both version fields, land them on main, push the tag, watch the signed build, and hand back the draft GitHub Release. Never publishes it.
disable-model-invocation: true
argument-hint: [version, e.g. 0.2.0 — omit and you'll be asked]
---

# Cut a release

Ship a tagged build of the current `main` through
[`.github/workflows/release.yml`](../../../.github/workflows/release.yml), following the steps
below precisely.

Hard rules:
- **Never publish the draft Release.** The workflow uploads to a *draft* on purpose — a human
  reviews the assets and the generated notes, then publishes.
- **Never tag a commit that isn't on `origin/main`,** and never tag from a feature branch.
- **Never move, delete, or force-push a tag that has already built.** A bad release is superseded
  by the next patch version, never rewritten — installers in the wild are pinned to release assets.
- **Never bypass the branch ruleset** (`--admin`, merge-queue overrides). The version bump goes
  through a PR like any other change.

Target version from the caller (may be empty): $ARGUMENTS

## 1. Preflight (read-only)

The tag must equal `version` in **both** `package.json` and `src-tauri/tauri.conf.json`;
`release.yml` asserts it, but only after a tag has been pushed and can no longer be reused. Check
first:

```
node .claude/skills/release/version.mjs check
```

Then confirm the ground state:
- `git -C <repo> status --short` — the tree must be clean.
- `git -C <repo> fetch origin` then `git -C <repo> log --oneline origin/main -5` — what you are about
  to ship.
- `gh run list --workflow ci.yml --branch main --limit 1` — `main` should be green. Shipping a red
  `main` is a decision for the user, not a default.

If anything is unmerged that the release is supposed to contain, STOP and say so.

## 2. Decide the version

Use `$ARGUMENTS` when given (a leading `v` is accepted and stripped). Otherwise ask via
`AskUserQuestion`, offering the next patch / minor / major from the current value.

Versions are plain `MAJOR.MINOR.PATCH` — no prerelease suffixes. `tauri.conf.json`'s `version`
becomes the Windows installer's numeric version resource, which cannot carry one.

## 3. Bump both fields together

Never on `main` (the ruleset requires a PR) and never one file at a time:

```
git -C <repo> checkout -b chore/release-v<version> origin/main
node .claude/skills/release/version.mjs set <version>
node .claude/skills/release/version.mjs check --version <version>
```

The second command must print `READY` before you continue.

## 4. Commit and open the PR

- Commit with [`commit`](../commit/SKILL.md) — it runs the gates the changed surface needs
  (`tauri.conf.json` lives under `src-tauri/`, so the Rust gates apply). Subject:
  `chore: release v<version>`.
- Open it with [`pull-request`](../pull-request/SKILL.md).
- **A human reviews and merges.** Stop here and tell the user the PR is waiting; resume at step 5
  once it has landed.

## 5. Tag `main`

Only after the PR is merged:

```
git -C <repo> checkout main && git -C <repo> pull --ff-only
node .claude/skills/release/version.mjs check --version <version>
git -C <repo> tag v<version>
git -C <repo> push origin v<version>
```

Re-running `check` here is not redundant: it is the last gate before a tag becomes permanent, and
it now also refuses a tag that already exists.

## 6. Watch the build

`gh run watch` (or `gh run list --workflow release.yml --limit 1` for the id, then
`gh run view <id> --log-failed` on failure). Expect 40–90 minutes; the compile dominates.

Signing is configured through six repository variables and gated on `AZURE_CLIENT_ID` — see the
release-signing bullet in [`CLAUDE.md`](../../../CLAUDE.md). Three failures map to one cause each:
- **403 at `Azure login`** — the Azure federated credential's subject no longer matches
  `repo:<owner>/kodabi:environment:release`. Renaming the `release` environment does this.
- **403 during signing** — `AZURE_SIGNING_ENDPOINT`'s region does not match the signing account's,
  or the signer role assignment is missing.
- **`Verify Authenticode signatures` fails** — something reached the installer that
  `signCommand` never saw. Read which file it named; do not work around it by relaxing the check.

A failed run leaves the tag in place. Fix forward on `main` and cut the next patch version — do not
re-push the tag.

## 7. Verify what shipped

- `gh release view v<version>` — the draft exists and carries `Kodabi_<version>_x64-setup.exe`.
- Confirm the run's verification step reported `Valid` for the installer and every installed
  binary. If signing was skipped (no variables configured), say so plainly — an unsigned release
  is a legitimate outcome, but never a silent one.

## 8. Stop

Report the draft Release URL, the tag, and whether the build was signed. **Do not publish it** —
a human reviews the notes and assets and clicks publish.

Model releases are a separate path with their own tag series: `scripts/upload-models.ps1` publishes
to a `models-v*` release, driven by `crates/kodabi-core/src/models/manifest.json`. This skill does
not touch them.
