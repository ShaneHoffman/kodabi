# Contributing to Kodabi

Thanks for helping build Kodabi. The project is developed through a **Kangentic board** with a
lightweight git-flow. Please follow the conventions below — AI agents get the same rules from the
repo's root `CLAUDE.md`.

## Before you start

- **Bugs and ideas** go through the [issue tracker](https://github.com/ShaneHoffman/kodabi/issues),
  which offers a form for each. The bug form asks for the details that actually help: Kodabi ships
  no log file, so the version, the Windows build, the model state and the on-screen error text are
  the evidence.
- **Security vulnerabilities do not go in an issue.** Report them privately through
  [`SECURITY.md`](SECURITY.md)'s disclosure route, which also states which versions get fixes.
- **Everyone taking part** follows the [Code of Conduct](CODE_OF_CONDUCT.md).

## Branch names

Every branch is `type/slug`:

- **`type`** — a [Conventional Commit](https://www.conventionalcommits.org/) type:
  `feat | fix | chore | docs | refactor | ci`.
- **`slug`** — a short kebab-case description. **No ticket / task ID.**
- Examples: `feat/scaffold-tauri-app`, `fix/inbox-reroute`, `docs/frontmatter-schema`,
  `chore/adopt-git-flow`.

If you create work through the board, set the branch name **when you create the task** (the
`kangentic_create_task` `branchName` field, or New Task → Advanced in the UI). A task's branch
cannot be renamed after it starts without breaking PR tracking, so get it right up front.

## Commits

Use Conventional-Commit subjects: `<type>: <imperative summary>`, matching the branch's `type`
(e.g. branch `feat/scaffold-tauri-app` → `feat: scaffold Tauri app shell`).

## Pull requests & the stage gate

Work moves through `Ready → Planning → Executing → Code Review → Open PR → Merge PR → Done`, with a
**manual gate between stages** (Planning → Executing advances automatically when a plan is approved).

- **Executing** commits to the branch but **does not push.**
- **Code Review** independently reviews `git diff origin/main...HEAD`, fixes the in-scope findings, and
  commits them on the branch — it **does not push.**
- **Open PR** pushes the branch and opens a PR against `main` (`gh pr create --base main`) — it is
  **not** merged automatically. A maintainer reviews the green PR, then moves the card to Merge PR.
- **Merge PR** merges the reviewed PR into `main` with a merge commit (`gh pr merge --merge
  --admin` — admin covers only the self-approval gap, never failing checks) and deletes the remote
  branch. The card then moves to Done.
- One PR per branch. **Never force a card back to Ready** — request changes by sending it back to
  Executing.

See the root **`CLAUDE.md`** for the agent- and MCP-specific details.
