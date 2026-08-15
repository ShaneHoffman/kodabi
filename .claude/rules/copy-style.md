# Copy style

Applies to **user-facing copy**: any language the user reads. This includes UI
strings, labels, button text, captions, tray text, the bundle/product
description, and user-visible error or status messages. It does **not** apply to
code comments or repo docs (README, `docs/`).

- No em dashes (the `—` character). Rewrite with a period, comma, colon, or
  parentheses instead.

Enforced for the frontend, not just reviewed: the copy guard in `eslint.config.js`
(`no-restricted-syntax`) fails an em dash in JSX text or a string literal anywhere
under `src/`. The scope above is the scope of the guard — the test harness and the
dev-only gallery are exempt (a `describe()` title is not language the user reads),
and code comments are out of reach by construction, since a comment is not an AST
node. Everything outside `src/` — the Rust side's user-visible strings, the bundle
description, tray text — is still review's job.
