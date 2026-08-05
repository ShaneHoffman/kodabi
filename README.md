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
- Chat over your history: a designed chat view driving Claude Code headless, plus an embedded
  terminal for power users — both wired to the MCP server

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
design/                 # Historical Phase-0 artefacts — the moodboard and spirit-mark
                        # pages. No build reads them; the live design system is the
                        # Grove theme in src/index.css.
src/                    # React + TypeScript frontend. src/index.css is the app's one
                        # and only stylesheet: the Grove theme (Tailwind v4 @theme
                        # tokens, keyframes, the .day/.hc variants) and the short list
                        # of things a utility cannot express. Grove's three faces ship
                        # with Windows, so the app fetches no font.
src-tauri/              # Tauri v2 binary crate — the desktop shell and its three
                        # windows (main, quick capture, capture overlay pill).
crates/kodabi-core/     # Pure, UI-agnostic, unit-testable data layer: settings, the
                        # SQLite note index, distill, and the MCP query surface.
crates/kodabi-audio/    # WASAPI loopback (system audio) and microphone capture via cpal,
                        # plus the two-channel combiner and the Settings mic test.
crates/kodabi-aec/      # Acoustic echo cancellation — a safe wrapper over a vendored
                        # speexdsp echo canceller, cleaning speaker bleed off the mic channel.
crates/kodabi-transcribe/ # Transcription engines: Parakeet TDT via sherpa-onnx (shipped),
                        # whisper.cpp (fallback), both cargo-feature-gated.
crates/kodabi-embed/    # Local embedding backend — bge-small-en-v1.5 via fastembed/ONNX
                        # Runtime, fully offline at runtime; cargo-feature-gated.
crates/kodabi-llm/      # The headless Claude Code runner every LLM call (cleanup, distill,
                        # routing, chat sessions) goes through.
crates/kodabi-mcp/      # Stdio MCP server (hand-rolled JSON-RPC) exposing the v1 tool
                        # surface of docs/MCP_TOOL_SURFACE.md over kodabi-core.
e2e/                    # End-to-end harness — drives the real app window over CDP, across
                        # the real IPC bridge (zero dependencies; see docs/UI_E2E_HARNESS.md).
.claude/                # Agentic dev workflow — task skills, read-only auditor agents, and
                        # the rules they enforce.
Cargo.toml              # Cargo workspace manifest (src-tauri + every crates/kodabi-* member).
package.json            # Frontend package manifest and scripts.
vite.config.ts, tsconfig*.json, eslint.config.js   # Frontend build/lint config.
CLAUDE.md, CONTRIBUTING.md, kangentic.json   # Agent guide, contributor guide, and the
                        # Kangentic board/workflow definition.
.github/                # CI workflows (GitHub Actions) — the gate matrix run on every PR.
scripts/                # PowerShell dev/build helpers (tray icons, resource profiling).
target/, dist/          # Build output (git-ignored).
```

## Development

**Prerequisites:** Node 24+, Rust (stable, MSVC toolchain), Visual Studio Build Tools with
"Desktop development with C++", and the WebView2 runtime (bundled with Windows 11).

```sh
pnpm install       # install frontend dependencies
pnpm tauri dev     # run the desktop app in dev mode
pnpm tauri:build   # build the installer bundle (real Parakeet engine)
pnpm dev           # frontend only, in a browser
pnpm build         # typecheck + Vite build
pnpm test          # frontend tests (vitest + Testing Library, jsdom)
pnpm lint          # frontend lint
pnpm e2e:build     # build the app for the end-to-end harness (must precede test:e2e)
pnpm test:e2e      # end-to-end tests against the real app window (Windows only)
pnpm seed:vault    # write a fixture vault of named scenarios, for previewing
```

### Speech-to-text engines

The STT engine is selected at build time by mutually exclusive cargo features
(`parakeet` or `whisper`), because their sherpa-onnx link modes cannot coexist in one
binary. Neither is on by default, so `pnpm tauri dev` runs a stub engine that emits
placeholder text — that keeps the dev loop and the test gates free of native model
dependencies. `pnpm tauri:build` passes `--features parakeet`, and a release build with
no engine feature **fails to compile on purpose**, so a stub build can never ship.

To run the real engine in dev mode, build with the feature and point the five model
variables at a locally downloaded [`sherpa-onnx-nemo-parakeet-tdt-0.6b-v2`
(int8)](https://github.com/k2-fsa/sherpa-onnx/releases/tag/asr-models) plus
`silero_vad.onnx`:

```sh
PARAKEET_ENCODER=.../encoder.int8.onnx PARAKEET_DECODER=.../decoder.int8.onnx \
PARAKEET_JOINER=.../joiner.int8.onnx PARAKEET_TOKENS=.../tokens.txt \
PARAKEET_VAD_MODEL=.../silero_vad.onnx \
pnpm tauri dev --features parakeet
```

Model download and settings wiring for end users is a later ticket. See
[`docs/benchmarks/stt-engine-benchmark.md`](docs/benchmarks/stt-engine-benchmark.md) for
why Parakeet is the shipping engine and
[`docs/RESOURCE_BUDGET.md`](docs/RESOURCE_BUDGET.md) for the deferred Whisper fallback.

Rust tests, lint, and format run from the repo root (the workspace covers all crates). A quick
local loop before pushing:

```sh
# Quick local loop (frontend + Rust):
pnpm test && pnpm lint
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all --check
```

The full CI gates are stricter (`--locked`, `-D warnings`, and per-crate feature legs for
`parakeet` / `whisper` / `vad` / `bge`). See [`CLAUDE.md`](CLAUDE.md) and
[`CONTRIBUTING.md`](CONTRIBUTING.md) for the complete matrix.

## Contributing

Kodabi is pre-alpha and AGPL-3.0 licensed; issues and discussion are welcome. Development runs on
a Kangentic board, with a `type/slug` branch-name convention and Conventional Commits. See
[`CONTRIBUTING.md`](CONTRIBUTING.md) for the branch/commit rules and the board flow, and
[`CLAUDE.md`](CLAUDE.md) for the full engineering gates.

## License

Kodabi is free software licensed under the **GNU Affero General Public License, version 3**
(`AGPL-3.0-only`). You may redistribute and/or modify it under the terms of version 3 of the License
as published by the Free Software Foundation. See [`LICENSE`](LICENSE) for the full text.

Copyright (C) 2026 Shane Hoffman
