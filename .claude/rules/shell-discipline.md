# Shell discipline

How agents run commands in this repo. The goal is auditable, reproducible tool
use, not a hard syntax ban.

- **Prefer dedicated tools over shell text-wrangling.** Read/Grep/Glob/Edit beat
  `cat`/`head`/`grep`/`sed`/`awk`/`echo >` — they surface in the permission UI,
  produce clickable results, and don't depend on the shell's quoting rules. Reach
  for a shell only when no dedicated tool fits (running `cargo`, `pnpm`, `git`,
  `gh`, `node`).
- **`git -C <path> …`, never `cd <path> && git …`.** An agent's working directory
  resets between calls, so a leading `cd` is both unreliable and, in some shells,
  a permission-prompt trigger. Pass the repo path to git directly.
- **Simple, auditable commands.** One logical action per invocation where it reads
  clearly. Dependent steps *may* chain (`pnpm install --frozen-lockfile && pnpm build`)
  — there is **no** single-command rule and **no** enforcement hook here. This
  harness runs PowerShell as the primary shell, where `&&`/`||` and pipelines are
  first-class; use them when they make a step clearer, not to cram unrelated work
  into one call.
- **Never write files through the shell.** No `>`/`>>` redirection, no here-docs to
  create or edit files — file writes go through the Write and Edit tools so the
  change is reviewable.
- **Mind the shell split.** PowerShell is primary; the Bash tool is available for
  POSIX scripts. They take different syntax (`$env:VAR` vs `$VAR`, `Remove-Item`
  vs `rm`). Quote Windows paths that contain spaces.
