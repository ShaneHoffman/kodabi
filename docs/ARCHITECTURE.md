# Kodabi — Architecture

*This document describes the system **as built**. It is the trimmed working half of
[`FOUNDING_DOC.md`](FOUNDING_DOC.md), which remains the source of truth for vision, category claim,
and the history of how the design got here — including §3, the Phase-0 architecture as it was
envisioned. Where the two disagree, this file is the current one. Every section below links the
deep-dive doc that owns its detail; none of them are restated here.*

## 1. The shape of it

Kodabi is a **Tauri v2 desktop app**, Windows-first. A Rust backend does the work, a React webview
draws it, and the intelligence lives outside the process entirely — in Claude Code, reached over
MCP on the user's own account.

```
┌──────────────────────────────────────────────────────────────┐
│ Tauri app (Windows)                                          │
│                                                              │
│  ┌────────────────────────┐   ┌───────────────────────────┐  │
│  │ Rust backend           │   │ Frontend (WebView2)       │  │
│  │  src-tauri/ (shell)    │◄─►│  src/ — React + Tailwind  │  │
│  │  crates/kodabi-* (core)│IPC│  3 windows: main,         │  │
│  │                        │   │  quick capture, overlay   │  │
│  │  • audio capture       │   │  • chat view              │  │
│  │  • transcription       │   │  • embedded xterm.js      │  │
│  │  • distill + routing   │   └───────────────────────────┘  │
│  │  • SQLite index        │                                  │
│  │  • file watcher        │   ┌───────────────────────────┐  │
│  └────────────────────────┘   │ claude CLI                │  │
│            ▲                  │ (user-installed, their    │  │
│            │  stdio MCP       │  own subscription/key)    │  │
│    kodabi-mcp.exe ◄───────────┤                           │  │
│                               └───────────────────────────┘  │
│                                                              │
│  Storage: Markdown + YAML frontmatter on disk (truth)        │
│           SQLite FTS5 + sqlite-vec (derived, rebuildable)    │
└──────────────────────────────────────────────────────────────┘
```

Two invariants shape everything else:

- **Files are the truth; the database is furniture.** Every note is a Markdown file with YAML
  frontmatter in a per-project folder. The SQLite index is derived, and can be deleted and rebuilt
  from the files at any time. Glossaries and routing examples are files inside the vault too, so
  they sync with the knowledge rather than living in app state.
- **The app never calls the Anthropic API.** It exposes an MCP server and lets Claude Code be the
  brain. See §4.

The three windows are declared in `src-tauri/tauri.conf.json`: `main`, `quick-capture` (the global
hotkey's text box) and `capture-overlay` (the always-on-top status pill).

## 2. Core vs shell, and the crate graph

**The rule: logic lives in `crates/kodabi-core`; `src-tauri` commands stay thin wrappers.** A Tauri
command owns its serde IPC DTOs, resolves managed state and paths, calls one core function, and
translates the error into user-facing copy. If a command grows a body, the body belongs in
kodabi-core. The full layer contract — core function, thin wrapper, `generate_handler![…]`
registration, typed TS caller, events — is
[`.claude/rules/tauri-command-parity.md`](../.claude/rules/tauri-command-parity.md), and
`src/invokeParity.test.ts` makes the naming half of it a test gate.

The Cargo workspace is `src-tauri` plus seven library crates. The split is by *dependency weight*:
every heavy native dependency lives in its own crate, so `kodabi-core` stays unit-testable without
one. Two of those crates go further and sit behind **off-by-default cargo features** —
`kodabi-transcribe` and `kodabi-embed`, the two model runtimes — which is what keeps the default
`cargo test --workspace` free of model downloads and MSVC/bindgen setup. The audio crates are not
optional: capture is not a feature you can build without.

| Crate | What it owns |
| --- | --- |
| `kodabi-core` | The pure data layer: settings, the SQLite note index, the commitment ledger, capture bookkeeping, distill, routing, the vault writer, retention, the file watcher, and the query surface the MCP server serves. UI-agnostic, and free of any model-runtime FFI — what it compiles natively is bundled SQLite, the `sqlite-vec` extension, and `ring` (via `ureq`'s TLS), none of which need a toolchain beyond a C compiler. |
| `kodabi-audio` | WASAPI loopback (system audio) and microphone capture via `cpal`, the two-channel combiner and drift correction, the recovery spill, and the Settings mic test. |
| `kodabi-aec` | Acoustic echo cancellation — a safe wrapper over a vendored speexdsp canceller, cleaning speaker bleed off the mic channel. |
| `kodabi-transcribe` | Transcription engines that need FFI: `ParakeetEngine` (sherpa-onnx) behind the `parakeet` feature, `WhisperEngine` (whisper.cpp) behind `whisper`, and the Silero `VadGate` behind `vad`. The `TranscriptionEngine` trait itself lives in `kodabi_core::transcription`, so core can be tested without any of them. |
| `kodabi-embed` | The local embedding backend — bge-small-en-v1.5 via fastembed / ONNX Runtime, behind the `bge` feature. Offline at runtime. |
| `kodabi-llm` | The headless Claude Code runner every LLM call goes through: the one-shot runner behind the glossary cleanup pass and distill, and the long-lived streaming chat session. Routing is *not* one of these — it is deterministic and purely lexical, with no model call. |
| `kodabi-mcp` | The stdio MCP server (hand-rolled JSON-RPC) exposing the v1 tool surface over kodabi-core. Ships as its own `kodabi-mcp.exe`. |

`src-tauri` is the shell: window setup, the tray, the global shortcuts, the command modules
(`*_cmds.rs`), and the orchestration that stitches capture to transcription to distill. It forwards
the `parakeet`, `whisper` and `embed` features down to the crates that implement them.

## 3. The frontend

`src/` is React + TypeScript + Tailwind v4, strict mode, built by Vite. Two conventions matter
structurally:

- **Effects live only in blessed bridge hooks.** A component never calls `useEffect`; each external
  system (a Tauri event stream, a DOM listener, a timer) is owned by one single-purpose `useXxx`
  hook with mandatory cleanup, and eslint fails any other file that tries.
  See [`.claude/rules/no-use-effect.md`](../.claude/rules/no-use-effect.md).
- **One stylesheet.** `src/index.css` is the Grove theme: Tailwind `@theme` tokens, keyframes, and
  the `.day` / `.hc` variant blocks. Components are styled with utility classes plus `cva`/`clsx`.
  The doctrine is [`DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md); the mechanics are
  [`UI_CONVENTIONS.md`](UI_CONVENTIONS.md).

## 4. The MCP inversion

The durable architectural bet: **the Rust backend is a dumb, testable data layer, and the AI layer
upgrades itself every time Claude Code ships.**

`kodabi-mcp` exposes the knowledge base as an MCP server over stdio — eight v1 tools, six read and
two write:

- **Read:** `search_notes`, `get_note`, `get_meeting_transcript`, `list_outstanding_items`,
  `list_projects`, `get_project_context`
- **Write:** `file_note_to_project`, `add_glossary_term`

Their schemas are specified in [`MCP_TOOL_SURFACE.md`](MCP_TOOL_SURFACE.md) and mirrored verbatim as
committed JSON under `crates/kodabi-mcp/schemas/`. The read tools are pre-approved for the CLI; the
write tools always prompt.

Claude Code is invoked as the **headless CLI**, never the Agent SDK (which is TypeScript/Python only
and would force a Node sidecar into a pure-Rust backend). There are exactly three spawn sites:

1. **Distill** — `kodabi-llm`, one-shot per meeting or chat.
2. **Chat** — one long-lived `claude -p` in bidirectional stream-json mode per conversation, with
   token streaming and the `can_use_tool` control protocol driving the inline Allow/Deny card.
3. **The embedded terminal** — interactive `claude` over a ConPTY PTY, for power users.

All three set `CLAUDE_CODE_SKIP_PROMPT_HISTORY` on the child, so Claude Code keeps no transcript
copy outside Kodabi's own retention policy.

Auth is Claude Code's problem, not ours: a subscription login and an API key both work through its
own mechanisms. The `claude` CLI is a **user-installed prerequisite**; `kodabi-mcp.exe` is carried
by the installer. Future integrations (GitHub, Azure DevOps) become "add another MCP server to the
project's config", not custom API clients.

## 5. The pipeline: capture → transcribe → distill → route → index

### Capture

System audio and the microphone are captured in parallel and kept as **two channels**: mic = you,
system = them. That is speaker attribution at zero diarization cost. On speakers the mic
acoustically picks up what they play, so the mic channel runs through `kodabi-aec` referenced
against the system channel first; on a headset there is nothing to cancel and it degrades to a
passthrough.

Audio spills to a **transient** on-disk recovery spool during capture, so a crash mid-meeting loses
at most the last flush interval and memory stays bounded on long meetings. The spool is cleared once
the session distills; an orphan left by a crash is reclaimed on the next startup.

**Capture is never invisible.** There is no state in which audio is recording without an on-screen
indication of it — the in-window indicator, a capturing-vs-idle tray icon, a capture-start toast, and
the full-screen overlay pill. The full invariant and its reasoning are FOUNDING_DOC §3.7.

### Transcribe

Engines implement the `TranscriptionEngine` trait and are **selected at build time by mutually
exclusive cargo features** — sherpa-onnx's static (Parakeet) and shared (whisper.cpp) link modes
cannot coexist in one binary. Neither is on by default, so every cargo gate and `pnpm tauri dev`
run against `MockEngine` and stay free of native model dependencies.

**Parakeet TDT is the sole shipping engine**, locked in by a real-meeting benchmark
([`benchmarks/stt-engine-benchmark.md`](benchmarks/stt-engine-benchmark.md)): silence-safe, ~10×
faster, no content-accuracy deficit. The whisper.cpp path works but is deferred — its mandatory VAD
gate measured roughly 200× slower ([`RESOURCE_BUDGET.md`](RESOURCE_BUDGET.md)). A release-profile
build that picked no engine **fails to compile**, by the `compile_error!` guard in
`src-tauri/src/transcribe.rs`, so a mock build can never ship.

A per-project glossary biases the engine where it can (Whisper's initial prompt) and a cheap Claude
cleanup pass fixes the rest at transcription time, so distill always starts from a clean transcript.

### Distill

At meeting end — batch, not continuous — a **single headless-Claude call** returns summary, action
items, decisions and open questions as one structured result. A transcript over the input character
budget is chunked on segment boundaries and map-reduced: one call per chunk, then as many merge
rounds as the parts themselves need — they are model output, so nothing bounds their combined size —
each returning that same single shape. A two-hour meeting distills rather than erroring. A failure
in any pass writes no note, leaving the session retryable.

**Chats are documents too.** An ended chat's `chats/*.jsonl` transcript runs a chat-flavored distill
through the same chunking, routing and note-write path, producing a `type: chat` note.

### Route

**The scorer is deterministic and purely lexical — there is no model call anywhere in routing**
(`crates/kodabi-core/src/routing.rs`). A candidate project earns weight from distinct glossary
term/alias matches, mentions of its own name, and similarity to its recorded corrections; a margin
rule subtracts the runner-up's weight, so evidence split across two projects reads as *low*
confidence rather than a coin flip.

That score drives a **confidence split**: a confident match is filed straight into its project, an
uncertain one waits in the **Inbox** for a one-click human decision, with the score recorded as the
reason it landed there. Miscategorized notes are worse than uncategorized ones.

Every manual re-route **records** the correction as a routing example in the target project's
`_routing_examples.yml`, and routing **reads those examples back** as an additive lexical-similarity
signal — so a correction measurably changes where the next note on that topic lands. The signal is
capped below the auto-file threshold, so one correction never files a note single-handedly.

Quick capture and chat notes enter this same pipeline; there is no second routing path.

### Store & index

Notes are Markdown + YAML frontmatter in per-project folders. The frontmatter field set is
[`FRONTMATTER_SCHEMA.md`](FRONTMATTER_SCHEMA.md) (and mirrors the MCP `NoteSummary` shape — editing
one requires checking the other); filenames carry a timestamp and device ID so two machines can
never collide ([`FILENAME_SCHEME.md`](FILENAME_SCHEME.md)). Each project folder also carries its own
`_glossary.yml`, `_routing_examples.yml` and `_ledger.yml`, which is what keeps them per-project
isolated and makes them sync with the knowledge. None of the three names its own project — the
folder it sits in is the project — which is what lets a project rename move the folder and carry
them along untouched.

The derived index is SQLite: **FTS5** for full-text and **sqlite-vec** for semantic search over
384-dimensional bge-small embeddings, chunked per note. A file watcher re-indexes on change, and the
whole database can be nuked and rebuilt. Retrieval is hybrid — both arms fused with **Reciprocal
Rank Fusion** (`crates/kodabi-core/src/index/search.rs`) — and surfaced to Claude Code as the
`search_notes` MCP tool, which then follows up agentically by reading full source files.

Multi-device is bring-your-own-sync: sync the folder with anything, and each device rebuilds its own
index locally. The database is never synced.

### The commitment ledger

The one durable database, and deliberately **not** the index. `ledger.db` lives in the config dir
beside `settings.toml` (`kodabi_core::ledger`, resolved through `sandbox::config_dir`), because it
holds what Markdown cannot carry: a commitment's identity across edits of its own text, evidence
gathered outside the vault, and the states a checkbox has no spelling for (snoozed, waived, closed
*with provenance*, superseded by a later mention). The note's checkbox remains the sole truth for
done/not-done — the ledger stores no `done` column and a `- [x]` flip is invisible to it.

That durability is why it is a separate file. The index may be nuked and rebuilt at will; these are
judgements a person made that exist nowhere else, so the ledger has its own append-only migration
set whose doctrine forbids drop-and-recreate. Its backup is the vault: after each change, the
affected project's entries are mirrored to `_ledger.yml`, and a missing or empty database is
rebuilt from those snapshots at startup — a non-empty one always wins, since merging two divergent
histories is not something to guess at. Extracted items are referenced by their content-hashed `a_`
ids, which are re-minted whenever a line's text is edited, so entries carry their own durable ids
and re-link across those edits (`kodabi_core::ledger::sync`).

The Commitments view reads it through `ledger_cmds`, whose organizing principle is the ledger's own
`direction` column: what you owe and what you are waiting on are two groups on two planes, not a
filter. A row is a ledger entry joined to the index's row for its source line
(`kodabi_core::ledger::view`), because the two stores own different halves — identity and judgement
here, `done` and `due_date` in the note. That is also why ticking a box in the view writes the
Markdown and records `closed_via: manual` beside it, while snoozing and waiving touch no note at
all: waiving exists precisely so nobody has to edit a meeting note to pretend something was not
said. Since the ledger worker owns the database single-threaded, commands reach it through a
request/reply channel (`ledger_state::LedgerClient`) that reports an unavailable ledger rather than
dropping a person's judgement silently, and mutations announce themselves on `ledger:changed` —
distinct from `vault:changed`, which would be a lie for a write that touched no note.

## 6. The release path, and its two signatures

Release builds ship a real engine and the embedder. `pnpm tauri:build` — which passes
`--features parakeet,embed` and the `src-tauri/tauri.bundle.conf.json` overlay — **is the canonical
definition of what a release compiles**; `.github/workflows/release.yml` invokes it rather than
restating it. `parakeet` is guarded by the `compile_error!` above; `embed` has no such guard, so
that script is the only thing pinning it.

The installer stays small: models are **downloaded on first run** from this repo's own `models-v1`
release, verified against SHA-256 digests from a manifest compiled into the binary, never hotlinked
upstream. The bundle target is NSIS only, per-user, no admin rights.

Pushing a `v*` tag runs the release workflow, which asserts the tag matches `version` in both
`package.json` and `src-tauri/tauri.conf.json`, then builds and signs. **Two signatures apply, with
deliberately opposite failure rules:**

- **Authenticode, via Azure Artifact Signing.** Configured by `signCommand` in the bundle overlay,
  authenticated in CI with a secretless GitHub OIDC federated credential. With the `AZURE_SIGNING_*`
  environment unset it **prints a skip line and exits 0**, so a local build and a fork need no Azure
  account. (The credentials expire in about an hour, which is why the long compile runs first as
  `--no-bundle` and the bundle-and-sign pass follows the login.)
- **Minisign, for the updater.** `tauri-plugin-updater` checks
  `releases/latest/download/latest.json` on startup and offers the update; trust is the pubkey in
  `tauri.conf.json` against a private key held only in repository secrets. Here an unconfigured
  build **fails immediately**, before the hour-long compile: a release missing its `.sig` and
  `latest.json` is silently invisible to every installed copy, which is far worse than a red build.
  `createUpdaterArtifacts` therefore lives in a CI-only third overlay
  (`src-tauri/tauri.updater.conf.json`), because the flag makes the CLI demand a private key nobody
  should have locally.

The workflow uploads three assets to a **draft** release (installer, `.exe.sig`, `latest.json`).
Publishing that draft is what rolls the update out, since `releases/latest/download/` ignores drafts.

## 7. Going deeper

| Topic | Doc |
| --- | --- |
| Vision, category claim, design history | [`FOUNDING_DOC.md`](FOUNDING_DOC.md) |
| Remaining phases and their checklists | [`ROADMAP.md`](ROADMAP.md) |
| Note frontmatter fields and key order | [`FRONTMATTER_SCHEMA.md`](FRONTMATTER_SCHEMA.md) |
| MCP tool schemas and conventions | [`MCP_TOOL_SURFACE.md`](MCP_TOOL_SURFACE.md) |
| Filenames, device IDs, session artifacts | [`FILENAME_SCHEME.md`](FILENAME_SCHEME.md) |
| STT engine choice and measurements | [`benchmarks/stt-engine-benchmark.md`](benchmarks/stt-engine-benchmark.md) |
| CPU/memory budget and tuning knobs | [`RESOURCE_BUDGET.md`](RESOURCE_BUDGET.md) |
| Design doctrine and UI mechanics | [`DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md), [`UI_CONVENTIONS.md`](UI_CONVENTIONS.md) |
| Isolated dev state for automated launches | [`DEV_SANDBOX.md`](DEV_SANDBOX.md) |
| End-to-end harness over CDP | [`UI_E2E_HARNESS.md`](UI_E2E_HARNESS.md) |
| Build gates, feature legs, board flow | [`../CLAUDE.md`](../CLAUDE.md) |
