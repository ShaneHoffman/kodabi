---
name: release
description: Cut a tagged Windows release — bump both version fields, land them on main, push the tag, watch the signed build, write the title and notes onto the draft GitHub Release, and hand it back. Never publishes it.
disable-model-invocation: true
argument-hint: [version, e.g. 0.2.0 — omit and you'll be asked]
---

# Cut a release

Ship a tagged build of the current `main` through
[`.github/workflows/release.yml`](../../../.github/workflows/release.yml), following the steps
below precisely.

Hard rules:
- **Never publish the draft Release.** The workflow uploads to a *draft* on purpose — a human
  reviews the assets and the notes, then publishes. `gh release edit` must never carry
  `--draft=false`; that flag *is* the publish.
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
release-signing bullet in [`CLAUDE.md`](../../../CLAUDE.md). Four failures map to one cause each:
- **`Assert the updater signing configuration is real` fails** — the two `TAURI_SIGNING_*`
  secrets are missing, or the committed pubkey is still a placeholder. This one fires in the
  first seconds, deliberately: unlike Azure signing it is not allowed to skip, because a release
  without `latest.json` is invisible to every installed copy.
- **403 at `Azure login`** — the Azure federated credential's subject no longer matches
  `repo:<owner>/kodabi:environment:release`. Renaming the `release` environment does this.
- **403 during signing** — `AZURE_SIGNING_ENDPOINT`'s region does not match the signing account's,
  or the signer role assignment is missing.
- **`Verify Authenticode signatures` fails** — something reached the installer that
  `signCommand` never saw. Read which file it named; do not work around it by relaxing the check.

A failed run leaves the tag in place. Fix forward on `main` and cut the next patch version — do not
re-push the tag.

## 7. Verify what shipped

- `gh release view v<version>` — the draft exists and carries all three assets:
  `Kodabi_<version>_x64-setup.exe`, its `.sig`, and `latest.json`. A draft missing the last two
  would publish an update nobody in the field can see.
- `latest.json`'s `version` matches the tag without the `v`, and its `url` points at this tag's
  installer.
- Confirm the run's verification step reported `Valid` for the installer and every installed
  binary. If signing was skipped (no variables configured), say so plainly — an unsigned release
  is a legitimate outcome, but never a silent one.

## 8. Retitle the draft and write the notes

The workflow leaves a placeholder title (`Kodabi v<version>`) and auto-generated notes (a PR
list). Replace both so this release reads like the previous ones — the title format, body
skeleton, and writing rules live in
[`references/release-notes-format.md`](references/release-notes-format.md). The build wait in
step 6 is a fine time to draft the body; apply it only once step 7 has confirmed the draft.

1. Gather what shipped: the previous `v*` tag
   (`git -C <repo> tag --list 'v*' --sort=-v:refname`), the draft's auto-generated notes
   (`gh release view v<version> --json body`) as the PR inventory, and
   `git -C <repo> log --first-parent --oneline v<previous>..v<version>`. Read the PRs behind
   anything the title alone doesn't let you describe for a user.
2. Write the body with the Write tool to a file **outside the repo** (per
   [`shell-discipline`](../../rules/shell-discipline.md)), following the format reference.
3. Apply both together — the title is the bare version, and per the hard rule above the
   command never carries `--draft=false`:

   ```
   gh release edit v<version> --title "<version>" --notes-file <path>
   ```

4. `gh release view v<version>` — confirm the title, the body, and that it is still a draft.

## 9. Stop

Report the draft Release URL, the tag, and whether the build was signed. **Do not publish it** —
a human reviews the notes and assets and clicks publish.

Say plainly in that report that **publishing is what ships the update to everyone already running
Kodabi**: the updater reads `releases/latest/download/latest.json`, which ignores drafts, so the
publish click is the rollout, not just a listing.

Model releases are a separate path with their own tag series: `scripts/upload-models.ps1` publishes
to a `models-v*` release, driven by `crates/kodabi-core/src/models/manifest.json`. This skill does
not touch them.
