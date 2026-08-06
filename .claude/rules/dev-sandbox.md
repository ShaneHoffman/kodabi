# Dev sandbox

Kodabi's dev build reads and writes the **same vault, settings, index and
recordings the user actually uses**. There is no separate dev profile by
default, and on Windows all of it shares one folder. So an agent that launches
the app to check a change can capture audio into real sessions, flip a real
setting, or let a retention sweep delete real notes — none of which the user
asked for.

**Every agent-driven launch is sandboxed. `pnpm tauri dev` is the user's own
workflow and stays untouched.**

- **Launch with `pnpm dev:sandbox`,** never bare `pnpm tauri dev`. It sets
  `KODABI_SANDBOX` to a worktree-local, gitignored `.sandbox/`, seeds the fixture
  catalogue on first run, and leaves real data alone. This binds every
  agent-driven surface: the [`preview`](../skills/preview/SKILL.md) skill, a
  Kangentic task session, a screenshot run, anything that opens the window.
- **One switch, never the pair.** `KODABI_SANDBOX` derives the vault root, the
  index, the config dir and the WebView2 profile from a single base. Setting
  `KODABI_KB_ROOT` or `KODABI_INDEX_DB` alongside it is **refused at startup** —
  those two are only safe when moved together, and half-setting them makes the
  startup reconcile delete rows from the real index. For a base other than the
  default, set `KODABI_SANDBOX=<absolute path>` (or `1` for the app-data `-dev`
  sibling) and nothing else.
- **Refusal, not fallthrough.** A sandboxed launch that would resolve to the
  real vault or the real app dirs exits with a message naming what to change. If
  a launch refuses, fix the environment — never work around it by dropping the
  switch.
- **The e2e harness rides the same mechanism.** `launchKodabi` sets
  `KODABI_SANDBOX` to a fresh `mkdtemp` base; there is no second isolation path
  to keep in sync. Adding one is the thing this rule exists to prevent.
- **Chat distill is off in the sandbox** unless explicitly re-enabled
  (`KODABI_DISABLE_CHAT_DISTILL=""`), so a preview session never spends live
  Claude calls distilling fixture conversation.

The state map — every location, its resolver, and what is deliberately *not*
sandboxed — is [`docs/DEV_SANDBOX.md`](../../docs/DEV_SANDBOX.md). Enforcement is
this rule plus the skills that cite it plus review; the refusals are the only
mechanical part, and they only fire once a launch is already sandboxed.
