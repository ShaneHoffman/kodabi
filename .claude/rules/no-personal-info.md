# No personal info

Kodabi is a public, AGPL-3.0 repository. Nothing personal or machine-specific
belongs in committed files.

- **Never commit:** real email addresses, real names (beyond the deliberate git
  commit identity), machine paths that embed a username (`C:\Users\<realname>\…`),
  API tokens or keys, or real meeting/transcript/note content captured from actual
  use.
- **Use placeholder values** that match the existing fixtures: `jane@example.com`,
  sample project names like `Paradise Golf`, note ids like `n_a1b2c3`. The
  `frontmatter-validator` fixtures under
  `.claude/skills/frontmatter-validator/fixtures/` are the reference for tone.
- **Tests and fixtures write only under a temp directory** — `std::env::temp_dir()`,
  the `tempfile` crate, or an equivalent — never into the repo tree, the user
  profile, or a real knowledge-base vault. A test that hard-codes an absolute path
  outside temp is a bug even when it passes locally.
- **Derive machine-specific locations at runtime** (Tauri's app/config dirs, env
  vars, a passed-in root) rather than hard-coding them.

Enforcement is review-based: the `/commit` skill scans the diff for stray real
paths and emails before committing, and the Code Review stage (`/code-review-fix`)
flags them. There is no CI scan.
