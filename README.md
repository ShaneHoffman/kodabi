# Kodabi

A self-organizing personal knowledge base. Kodabi turns meeting transcripts and quick notes into a
searchable, chat-able knowledge base: **transcribe → distill → auto-route → search & chat**.

## Status

Pre-alpha — early development, no releases yet. See [`ROADMAP.md`](docs/ROADMAP.md) for the phased plan.

## Stack

- **Tauri** (Rust) — Windows-first desktop shell
- **React + Tailwind** — frontend
- **SQLite** (FTS5 + `sqlite-vec`) — hybrid full-text + vector search
- **MCP server** — exposes the knowledge base to Claude Code for chat over real history

## Features (v1 planned)

- End-of-meeting pipeline: summary → action-item / decision extraction, with glossary cleanup
- Confidence-split routing into projects, with an Inbox and one-click re-route correction loop
- Quick-capture window (global hotkey → text box → same routing pipeline)
- Hybrid retrieval (full-text + vector, RRF merge) exposed as a `search_notes` MCP tool
- Chat over your history via an embedded Claude Code terminal wired to the MCP server

## Recording & privacy

Kodabi records your microphone and system audio, but **only while the listening indicator is green** —
capture is a deliberate act (a global hotkey or the tray menu), never silent. Before your very first
capture, a one-time in-app nudge asks you to **announce your recordings**: many places (Massachusetts
among them) require everyone on a call to consent before you record. Nothing is recorded until you
acknowledge it.

Everything stays on your machine as plain files — audio and transcripts never leave except through
your own Claude account. A **retention policy** (Settings → Privacy) governs how long raw session
transcripts are kept: keep all (the default — nothing is pruned until you choose), keep for a set
number of days, or discard each transcript as soon as it has been distilled into a note. At-rest
security relies on your OS disk encryption (e.g. BitLocker) plus this retention policy; app-level
encryption is a later consideration.

## Repository layout

```
docs/                   # Strategy & spec docs — roadmap, aesthetic direction, founding doc.
design/                 # Locked design tokens — design/tokens.css is the single source of
                        # truth for color, type, and spacing; imported by the app, never
                        # duplicated.
src/                    # React + TypeScript frontend. src/index.css bridges the tokens
                        # into Tailwind v4 utilities; src/fonts.ts self-hosts the Source
                        # typeface trio.
src-tauri/              # Tauri v2 binary crate — the desktop shell and window.
crates/kodabi-core/     # Pure, UI-agnostic, unit-testable data layer that the shell
                        # depends on. Future SQLite index and MCP query surface live here.
Cargo.toml              # Cargo workspace manifest (src-tauri + crates/kodabi-core).
package.json            # Frontend package manifest and scripts.
vite.config.ts, tsconfig*.json, eslint.config.js   # Frontend build/lint config.
target/, dist/          # Build output (git-ignored).
```

## Development

**Prerequisites:** Node 24+, Rust (stable, MSVC toolchain), Visual Studio Build Tools with
"Desktop development with C++", and the WebView2 runtime (bundled with Windows 11).

```sh
pnpm install       # install frontend dependencies
pnpm tauri dev     # run the desktop app in dev mode
pnpm tauri build   # build the installer bundle
pnpm dev           # frontend only, in a browser
pnpm build         # typecheck + Vite build
pnpm test          # frontend tests (vitest + Testing Library, jsdom)
pnpm lint          # frontend lint
```

Rust tests, lint, and format run from the repo root (the workspace covers both crates):

```sh
cargo test
cargo clippy --all-targets
cargo fmt --check
```

## License

Kodabi is free software licensed under the **GNU Affero General Public License, version 3**
(`AGPL-3.0-only`). You may redistribute and/or modify it under the terms of version 3 of the License
as published by the Free Software Foundation. See [`LICENSE`](LICENSE) for the full text.

Copyright (C) 2026 Shane Hoffman
