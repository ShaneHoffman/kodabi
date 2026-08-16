# Summary

<!-- What this change does, and why. Link the issue or board task if there is one. -->

## How it was tested

<!-- The gates you ran, plus anything you checked by hand in the app. -->

## Checklist

Conventions are in
[`CONTRIBUTING.md`](https://github.com/ShaneHoffman/kodabi/blob/main/CONTRIBUTING.md); the full
engineering rules, including the gate commands for each surface, are in
[`CLAUDE.md`](https://github.com/ShaneHoffman/kodabi/blob/main/CLAUDE.md).

- [ ] Branch is named `type/slug`, with a Conventional-Commit type and no task ID.
- [ ] Commit subjects are `<type>: <imperative summary>`, matching the branch type.
- [ ] The pre-commit gates for the surfaces this change touches all pass locally. Rust changes need
      `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`,
      and `cargo test --workspace --locked`; frontend changes need
      `pnpm exec eslint . --max-warnings=0`, `pnpm test`, and `pnpm build`. Feature-gated crates
      (`kodabi-transcribe`, `kodabi-embed`, `src-tauri`) have extra legs listed in `CLAUDE.md`.
- [ ] Docs that enumerate anything this change touches are updated in the same change.
- [ ] No real personal data, real email addresses, machine paths with a username, or captured
      meeting content. Tests write only under a temp directory.
- [ ] No em dashes in user-facing copy.
