# Kodabi — Founding Document

*This document is the source of truth for vision and architecture; the working roadmap derived
from it is [`ROADMAP.md`](ROADMAP.md). It lives in the `docs/` folder and gets amended, not
abandoned.*

**Why "Kodabi":** Kodabi (ko-DAH-bee) is coined from *kodama* (木霊, the forest spirit that hears
you and echoes back) and *yamabiko* (山彦, the mountain's answering voice — its `-biko` suffix is
the "one who answers" morpheme). It names a small spirit that lives in the trees of your own
forest: it listens quietly (ambient capture), remembers everything it hears (local transcription
memory), and answers when you speak to it (chat with your knowledge) — and it never leaves its
forest (local-first). Ghibli's Princess Mononoke gave the kodama gentle, watchful associations —
good mascot energy for the listening indicator, carried over unchanged (see §4). Namespace check
(July 2026): "Kodama" is heavily claimed — the bare crates.io and npm names are squatted, most
kodama.\* domains are gone, and Kodama Systems is a funded startup holding kodama.ai — so we
switched to the Kodabi coinage pre-launch, while it's cheap. Availability verified 2026-07-14:
crates.io, npm, kodabi.app, and kodabi.dev are all free.

---

## 1. Vision

A fully local, open-source desktop app for **personal knowledge management with AI as the default**, not bolted on. Kodabi is where all of your working knowledge lives — and its flagship capability is that meetings feed it automatically: it listens to meetings, huddles, and videos, transcribes them on-device, and routes the distilled content to the right project. But it is equally your home for typed notes, quick captures, and every conversation you have with your own knowledge. Many mouths, one knowledge base: meetings, notes, chats (and later voice memos, GitHub activity, imported docs) all become markdown, all route, all join one conversation through Claude.

**One sentence:** A self-organizing personal knowledge base — your meetings, notes, and project activity organize and maintain themselves, powered by the Claude subscription you already pay for.

**Category claim: self-organizing PKM.** Every other PKM is self-organized — by you, forever: filing, tagging, linking, grooming, until you stop and the garden rots. Kodabi's thesis is zero gardening: knowledge flows in, organizes itself, and maintains itself. You never file; you only correct — and every correction makes it quieter (routing loop, self-maintaining glossary, voice profiles). **The test every feature must pass: does it reduce organizing work, or create it?**

**Positioning:** Not an AI meeting-notes app (that competes with Granola). A self-organizing PKM (competing with Obsidian-class tools) whose unfair advantage is that spoken knowledge flows in automatically.

**Who it's for (v1):** Me — a tech lead / consultant juggling multiple client projects with hard boundaries between them. Open-sourced for the niche that shares this shape: developers and consultants who already pay for Claude and want local-first tooling.

**Core beliefs:**

- Meeting content is where project knowledge dies. Capture it passively, organize it automatically.
- Local-first is a feature, not a compromise. Audio and transcripts never leave the machine except through the user's own Claude account.
- Project isolation is a first-class primitive. Client A can never leak into Client B's context.
- "Free with your Claude sub" removes pricing, hosting, and trust problems in one move.
- Reliability of the core loop (capture → categorize → chat) beats feature count. If routing is wrong or annoying, nothing above it matters.

## 2. Differentiation

The space is crowded (Granola, Notion AI, Obsidian + plugins, Reflect, Mem, Tana, Logseq). Individually, each strength below exists somewhere; the identity claim is the thing none of them have:

**The core gap: every PKM ever made assumes you'll be the gardener — and everyone eventually stops.** Manual filing, tagging, linking, and review is the structural failure of the category; AI-added-on tools put chat on top of a garden you still tend. Kodabi is **self-organizing**: it never asks you to organize, only to occasionally correct, and corrections compound into it needing fewer of them.

**Flagship expression (post-v1): the commitment ledger.** Not a task manager — a to-do list that writes and erases itself:

- *Captured:* commitments extract themselves from meetings and notes ("I'll send the spec Friday", "Tyler's taking the webhook") with who/what/when and a link to the moment it was said
- *Tracked:* open commitments per project, yours vs. others', aging visibly
- *Closed by evidence:* a PR merges, a later meeting says "done", a note mentions shipping — the ledger closes items itself (said-vs-shipped generalized beyond GitHub)
- *Resurfaced:* pre-meeting briefs surface what you owe and what's owed to you

Todoist-class tools require entering tasks; Motion schedules what you entered; Granola's action items die in the summary. Nobody occupies "sourced from what was said, closed by what happened."

Supporting differentiators:

1. **Fully local capture and transcription** — no cloud audio, no bot joining calls, works offline.
2. **Consulting-shaped project isolation** — per-project knowledge bases, glossaries, and (later) integration connectors, physically separated.
3. **Bring-your-own Claude** — no subscription to this tool, no vendor economics, no data custody questions.
4. **MCP-native architecture** — the app *is* an MCP server; Claude Code is the brain. Users inherit the entire Claude Code ecosystem (skills, hooks, other MCP servers) for free.
5. **It's beautiful** — most open-source tools in this category look like admin panels. Design is the moat for word-of-mouth and GitHub screenshots.

## 3. Architecture

### 3.1 High-level shape

```
┌────────────────────────────────────────────────────────┐
│ Tauri App (Windows-first)                              │
│                                                        │
│  ┌──────────────┐   ┌───────────────────────────────┐  │
│  │ Rust Backend │   │ Frontend (webview)            │  │
│  │              │   │                               │  │
│  │ • WASAPI     │   │ • Designed UI (calm surface,  │  │
│  │   loopback + │   │   meeting review, search)     │  │
│  │   mic capture│   │ • Chat UI (drives Claude Code │  │
│  │ • whisper.cpp│   │   headless underneath)        │  │
│  │   (CUDA)     │   │ • Embedded xterm.js terminal  │  │
│  │ • SQLite     │   │   (power-user escape hatch)   │  │
│  │   FTS5 +     │   │                               │  │
│  │   sqlite-vec │   └───────────────────────────────┘  │
│  │ • File       │                                      │
│  │   watcher    │   ┌───────────────────────────────┐  │
│  │ • MCP server │◄──┤ Claude Code (user's sub or    │  │
│  │   (stdio)    │   │ API key) — the AI brain       │  │
│  └──────────────┘   └───────────────────────────────┘  │
│                                                        │
│  Storage: plain markdown + frontmatter on disk         │
│           (source of truth; SQLite is derived cache)   │
└────────────────────────────────────────────────────────┘
```

### 3.2 The MCP inversion

The app never calls the Anthropic API directly. Instead:

- The Rust backend exposes an **MCP server** with tools such as: `search_notes`, `get_meeting_transcript`, `list_outstanding_items`, `file_note_to_project`, `add_glossary_term`, `list_projects`, `get_project_context`.
- **Claude Code is the intelligence layer.** It runs either headless (driving the designed chat UI) or interactively (in the embedded terminal), with the app's MCP server in its config.
- Auth is Claude Code's problem, not ours: the user's existing subscription login or an API key both work through Claude Code's own mechanisms. Verify current capabilities against the official docs before implementation: https://docs.claude.com/en/docs/claude-code/overview
- Later integrations (GitHub, Azure DevOps boards) become "add another MCP server to the project's config," not custom API clients.

**Why this is durable:** the Rust backend stays a dumb, testable data layer. The AI layer upgrades itself every time Claude Code ships. Sub usage is legitimate because it literally *is* Claude Code.

### 3.3 Audio capture (Windows-first)

- **System audio:** WASAPI loopback via the `cpal` crate — captures whatever the speakers play (Teams, Zoom, YouTube, recorded talks). Mature and well-supported.
- **Microphone:** captured in parallel. On a headset call, your voice never hits system audio, so both streams are required for complete meetings.
- **Two-channel bonus:** mic channel = you, system channel = them. Crude but useful two-way speaker attribution with zero diarization cost.
- **Controls:** global hotkey + tray/menu toggle to start/stop. Unambiguous visual "listening" state (also the consent story — see 3.7).
- macOS (ScreenCaptureKit) and Linux (PipeWire monitor) are post-launch, ideally community-contributed.

### 3.4 Transcription

- **Engine is a trait, not a dependency.** The Rust backend defines a `TranscriptionEngine` trait; concrete engines are swappable. This is the design decision that makes the model choice low-stakes. Engines are **selected at build time via mutually exclusive cargo features** — sherpa-onnx's static (Parakeet) and shared (whisper.cpp) link modes cannot coexist in one binary — so a release build ships exactly one native engine (Parakeet; a multilingual build would ship whisper.cpp instead, but that path is deferred for v1 — see the fallback bullet below). The feature-less build compiles no native engine and falls back to a mock engine for UI development only; it can no longer be released, because a release-profile build with no engine feature **fails to compile** (the `compile_error!` guard in `src-tauri/src/transcribe.rs`). CI builds and exercises the shipping Parakeet configuration on every Rust change.
- **Default engine: NVIDIA Parakeet TDT via sherpa-onnx** (Apache 2.0). Rationale: near-Whisper-large English accuracy, dramatically faster, streaming-friendly, and — critically for meetings — its architecture doesn't hallucinate phantom text during silence, which Whisper is notorious for on pause-heavy audio. sherpa-onnx provides Rust bindings and bundled VAD with no Python/NeMo dependency.
- **Fallback engine: whisper.cpp (large-v3-turbo)** for multilingual needs and the strongest glossary-biasing mechanism (initial prompt) — selected at build time in place of Parakeet, not a runtime fallback (the two link modes can't coexist in one binary). Must be paired with **Silero VAD** to chop silence before it hallucinates. **Deferred for v1:** that mandatory VAD path crashes on Windows on a sherpa-onnx/ONNX Runtime API-version mismatch, and no fixed sherpa-onnx has shipped, so Parakeet is the sole shipping engine until one does (board task #53; decision recorded in `docs/RESOURCE_BUDGET.md`).
- **Benchmarked both on one real recorded meeting (2026-07-15); default locked to Parakeet TDT** — silence-safe, ~10× faster, and no content-accuracy deficit (its lone proper-noun miss is exactly what the post-pass fixes). See `docs/benchmarks/stt-engine-benchmark.md`.
- **Per-project glossary** — project names (OKIES, ForeUp), client names, teammate names, domain terms. Applied via initial-prompt bias where the engine supports it (Whisper); otherwise enforced entirely by the post-pass. This is the defense against mangled proper nouns poisoning categorization and search.
- **Post-pass cleanup:** at meeting end, a cheap Claude pass ("here's the glossary; fix obvious misrecognitions") catches what biasing missed — and is engine-agnostic.
- **Self-maintaining glossary:** when corrections happen, the tool proposes glossary additions.
- **No diarization in v1.** Timestamps + the two-channel you/them split are enough.

### 3.5 Categorization & the distill pipeline

At meeting end (batch, not continuous — keeps token usage sane):

1. Glossary cleanup already happened at transcription time (the Phase 1 post-pass), so distill starts from a clean transcript.
2. A **single headless-Claude distill call** returns summary, action items, decisions, and open questions as one structured result. Transcripts over the input character budget are chunked on segment boundaries and map-reduced — one call per chunk plus a merge call returning the same single JSON shape (in several batched rounds when the chunk results themselves overflow the budget) — so a long meeting distills rather than erroring (landed as task #59, `feat/distill-token-budget`; a failure in any pass writes no note, so the session stays retryable, and a transcript past the chunk cap fails immediately rather than tying up the pipeline for hours).
3. Route to a project with a **confidence split**: confident → filed directly; uncertain → an **Inbox** for one-click human routing. Miscategorized notes are worse than uncategorized ones.
4. **Correction loop:** every manual re-route **records** the correction as a routing example (`_routing_examples.yml` in the project folder), and routing **reads** those examples back as an additive lexical-similarity signal, so a correction measurably changes future routing — a note about a corrected topic files itself next time (landed as task #56, `feat/routing-examples-signal`; capped below the auto-file threshold on its own so one correction never files a note single-handedly). Wiring routing into the distill pipeline is tracked as task #55 (`feat/wire-distill-routing`).

### 3.6 Storage & indexing

- **Source of truth: plain markdown + YAML frontmatter** in a per-project folder structure. Buys Obsidian compatibility, git backups, and zero lock-in.
- **Derived index: SQLite** in the same app —
  - **FTS5** for full-text search (covers most real queries, especially with a clean glossary),
  - **sqlite-vec** for semantic search,
  - **local embeddings** (small model, e.g. bge-small / nomic-embed via fastembed or ONNX, CPU is fine).
- **Rebuildable by design:** file watcher re-chunks/re-embeds on change; the index can be nuked and rebuilt from markdown at any time. The database is furniture, not foundation.
- **Chats are documents too:** conversations with the notes generate new knowledge; distill and file chat sessions just like meetings, and index them.
- **Retrieval:** hybrid FTS5 + vector with reciprocal rank fusion → top chunks to Claude → Claude follows up agentically by reading full source files (it has MCP tools for that).

### 3.6b Multi-device (sync-friendly by design, bring your own sync)

- The knowledge base is a plain folder — sync it with anything: Syncthing (pure-local ethos), OneDrive/Dropbox (pragmatic), or a git repo.
- **Each device rebuilds its own SQLite index locally** from the synced files. The database is never synced (synced SQLite corrupts; the index is rebuildable by design).
- **Glossaries, project config, and routing examples live as files inside the folder** so they sync with the knowledge.
  - Concretely, each project folder carries a `_glossary.yml` at its root — a plain YAML list of `{ term, definition, aliases }` entries (`crates/kodabi-core`'s `glossary` module owns load/save/upsert). The project itself is never a field in the file; it's implicit in which project folder the file sits in, which is what keeps the glossary per-project-isolated. The MCP `GlossaryTerm` shape's `project` field is filled in from that folder path when the API surfaces a term.
  - Routing examples follow the same per-folder pattern: each project folder carries a `_routing_examples.yml` (`crates/kodabi-core`'s `routing_examples` module owns load/save/upsert), and a re-route moves the recorded example from the previous project's file into the target's. Like the glossary, the project is implicit in the folder, so the examples sync and stay project-isolated with the knowledge. Each example stores a prose-only excerpt (Markdown structure is stripped, so the section headings every distilled note shares can't read as topical similarity), and a project's log is capped at `MAX_EXAMPLES` with the oldest evicted first — the scorer only ever consults the single best-matching example, so an uncapped log would grow the per-capture cost without ever changing a decision.
- **Filenames include timestamp + device ID** so simultaneous capture on two machines can never collide; the append-mostly design makes conflicts nearly impossible (scheme: [`FILENAME_SCHEME.md`](FILENAME_SCHEME.md)).
- V1 ships zero sync code — one README paragraph. Built-in git-backed sync is a Phase 5 candidate.
- **Import/export (settings):** export = zip of the knowledge base (or a single project) with notes, glossaries, config, routing examples + a version manifest; import = *merge*, never overwrite (timestamp+device-ID filenames prevent collisions; index rebuilds after). Import doubles as the schema-migration hook for old archives. Single-project scope covers the consulting cases: archive a finished engagement, hand a project to a colleague. Import-from-Obsidian/plain-markdown is a Phase 5 onboarding ramp.

### 3.7 Trust, consent, and hygiene

- **Recording consent:** Massachusetts (and many states) require two-party consent. One-time in-app nudge ("announce your recordings"), unambiguous listening indicator, and a clear statement in the README. This is both legal hygiene and a trust signal.
- **Retention policy:** governs the stored transcript — optional "distill then discard the raw transcript after N days." Raw client-call transcripts accumulating forever is a liability. **No audio is *retained* in v1** — only the transcript + timestamps become a lasting on-disk artifact, so there is no retained audio for the retention policy to prune yet. (The task #57 incremental-capture spool flushes audio to disk *transiently* during a meeting for crash recovery, but that spool is cleared once the session distills, and an orphaned spool left by a crash is reclaimed on the next startup — not by retention.) When an opt-in audio-retention toggle is eventually pulled by a use case, the retention policy must cover it too. **One gap the in-app policy cannot reach:** the `claude` CLI that `kodabi-llm` spawns for distill keeps its *own* Claude Code session logs, which contain the transcript text passed to it — outside our retention control. Document this, and disable that logging where the CLI allows it, so the policy's promise is complete.
- **Security at rest (v1 posture):** rely on OS disk encryption (BitLocker) + the retention policy. Say so explicitly in docs. App-level encryption is a later consideration.
- **Resource budget:** idle ≈ zero; capturing under a target CPU ceiling (tune on real hardware); no fan spin-up during meetings. Treat as a requirement, not a bug report. Measurement procedure, tuning knobs, and the recorded numbers live in [`RESOURCE_BUDGET.md`](RESOURCE_BUDGET.md).

## 4. Design Principles

Design is a feature, not polish. Bar: Linear / Things 3 quality, not admin-panel.

**Thesis: calm by default, dense on demand.** The app is open all day next to real work. Resting state is nearly invisible — quiet capture indicator, today's meetings, nothing shouting. Engagement states (search, chat, meeting review) open into density.

**Aesthetic direction: the name is the design brief.** Minimalist and beautiful, drawn from Japanese aesthetics and the forest-spirit folklore:

- **Ma (間)** — negative space as an active element. Whitespace-heavy layouts, one thing per view. Emptiness is the design, not the absence of it.
- **Forest palette** — moss, fern, mist, stone, washi-paper cream, sumi-ink charcoal. Muted and natural, never SaaS blue/purple. One quiet green as the sole accent (the listening state).
- **Wabi-sabi restraint** — soft edges, paper-and-wood feel; no gloss, gradients, or glassmorphism trend-chasing.
- **The listening indicator IS the kodama** — an original minimal spirit-mark (evoke the archetype, never trace Ghibli's character): gently animate while listening, still when idle. Logo, trust signal, and screenshot in one element.
- **The kodama rattle** — an optional, off-by-default soft wooden "tick" on capture start/stop and distill-complete. A signature detail for those who enable it.
- **Moodboard references (Phase 0):** Japanese stationery, Ghibli *backgrounds*, Muji, washi paper, misty forest photography.

**Guardrail — theme as restraint, not decoration.** The kodama influence lives almost entirely in palette, spacing, and the one animated spirit-mark. No leaf icons on buttons, no wood-grain backgrounds, no spirit illustrations scattered through the UI. Success test: a user thinks "this is unusually calm and beautiful" without consciously registering "forest." If the theme is noticeable as a theme, it's overdone.

Principles:

- **Typography-first, chrome-last.** One humanist sans (clean, slightly soft, wide spacing) + a serif reserved for reading views. Hierarchy through type and spacing, almost never color. Generous line height, real margins. No borders and boxes.
- **Motion as feedback, not decoration.** The meeting-end distill-and-route moment gets one satisfying transition — the note drifting into its project like a spirit settling into a tree. Everything else is near-instant.
- **The listening state deserves love.** Subtle waveform / breathing indicator. It's the screenshot, and it's the trust signal.
- **⌘K / Ctrl-K command palette as primary navigation.** Fits the audience, keeps chrome minimal.
- **Two AI surfaces, one architecture:** the designed chat UI (Claude Code headless underneath) is the front door; the embedded xterm.js terminal is the power-user escape hatch. Terminal ships first (nearly free), pretty chat follows.
- **Process discipline:** lock the aesthetic before building screens — moodboard, type scale, spacing tokens, color system. "Clean" retrofitted onto grown UI never lands.

## 5. V1 Definition

### Definition of done (the sentence that prevents scope creep)

> I can capture a real client meeting via hotkey, and within ~2 minutes of it ending I have a correctly-routed, glossary-clean summary with action items filed as markdown in the right project folder — I can jot a quick note that routes the same way — and I can answer a question about last week from chat.

### V1 scope

- Windows only, WASAPI loopback + mic capture, hotkey + tray toggle
- Local transcription via the engine trait (Parakeet default, whisper.cpp fallback; engine chosen at build time via mutually exclusive cargo features) with per-project glossary
- End-of-meeting distill: a single headless-Claude call returning summary, action items, and decisions as one structured result (glossary cleanup already ran as the Phase 1 transcription post-pass)
- **Manual notes as first-class:** global quick-capture hotkey (type a thought → routes like a meeting) + create/edit notes within a project. Same storage, routing, and indexing machinery — a text box in front of the existing pipeline
- Confidence-split routing with Inbox + one-click correction
- Markdown + frontmatter storage, per-project folders
- SQLite FTS5 + sqlite-vec hybrid index, local embeddings, rebuildable
- MCP server exposing the knowledge base
- Claude Code integration: embedded terminal first, designed chat UI second
- Retention setting, consent nudge, listening indicator

### V1 anti-scope (explicitly not building)

- ❌ Speaker diarization (two-channel you/them split only)
- ❌ Automated workflows / automations
- ❌ GitHub / ADO / any integrations (Phase 3)
- ❌ Team features, sync, sharing
- ❌ macOS / Linux
- ❌ Calendar-driven auto start/stop (hotkey is the v1 answer)
- ❌ App-level encryption

## 6. Roadmap

*The **working roadmap** is [`ROADMAP.md`](ROADMAP.md); Phases 0–1 are broken into individual
tickets on the Kangentic board. This section keeps only what lives nowhere else: each phase's goal
and milestone, and the full detail behind the Phase 5 candidates.*

### Phase 0 — Foundations (decisions + skeleton) — ✅ complete

Shipped: license (**AGPL-3.0-only**), design system ([`DESIGN.md`](DESIGN.md),
`design/tokens.css`, [`SPIRIT_MARK.md`](SPIRIT_MARK.md)), Tauri + Rust workspace scaffold with CI,
the frontmatter schema ([`FRONTMATTER_SCHEMA.md`](FRONTMATTER_SCHEMA.md)), and the MCP tool
surface ([`MCP_TOOL_SURFACE.md`](MCP_TOOL_SURFACE.md)). One item still open, tracked in the
backlog: reserve the domain variant (kodabi.app / kodabi.dev) + crates.io/npm names, and backorder
kodabi.com (parked at GoDaddy, renew-prohibited, expires 2026-10-04). The GitHub repo/org rename to
`kodabi` happens after this rename branch merges, so tooling isn't disrupted mid-task (currently
`github.com/ShaneHoffman/kodama`).

### Phase 1 — Capture & transcribe (the hard 20%)

Capture (WASAPI loopback + mic, hotkey/tray, listening indicator), the `TranscriptionEngine`
trait with Parakeet + whisper.cpp engines selected at build time via mutually exclusive cargo
features (release builds ship Parakeet), the real-meeting benchmark that locks the default,
glossaries, and raw session storage (transcript + timestamps; audio is not persisted in v1 — an
opt-in audio-retention toggle is deferred until a use case pulls it, at which point the retention
policy must cover it) — tracked as individual tickets in the backlog.

**Milestone:** a full Teams meeting produces a clean, timestamped transcript with correct project nouns, hands-free after one hotkey.

### Phase 2 — Distill, route, store, index

Checklist in [`ROADMAP.md`](ROADMAP.md).
**Milestone:** the definition-of-done sentence is true, minus chat.

### Phase 3 — The brain (MCP + Claude Code)

Checklist in [`ROADMAP.md`](ROADMAP.md).
**Milestone:** "What's outstanding on Paradise Golf?" answered correctly in-app from real meeting history. **← Dogfood daily from here.**

### Phase 4 — Polish & open-source launch

Checklist in [`ROADMAP.md`](ROADMAP.md).
**Milestone:** a signed, onboarded, documented Windows release, launched publicly.

### Phase 5 — Growth (pulled by daily use, not pushed by roadmap)

[`ROADMAP.md`](ROADMAP.md) lists these by name only; the detail lives here. Candidates, in rough
order of expected value — each earns its place only after the core loop proves reliable:

- **Commitment ledger (flagship)** — the self-writing, self-erasing to-do: extraction is already in the v1 pipeline; the ledger adds tracked state per project, closure by evidence (starting with GitHub MCP: commitments reconciled against PRs/commits — said vs. shipped), aging, and pre-meeting resurfacing
- Azure DevOps board integration (per-project connectors, isolated; feeds ledger closure)
- Weekly digests per project; "what did I commit to this week"
- Decision log queries ("when and why did we choose X")
- Pre-meeting prep briefs, **auto-triggered by meeting detection**: by the time you've joined, a brief is waiting — last decisions with these people, your open commitments to them, theirs to you, unresolved questions, what changed since you last spoke. Pure retrieval over existing data; early post-v1 candidate
- **Live meeting assistance** — streaming transcription + periodic Claude pass (rolling window, every ~60–90s) grounded in your role and project history, surfacing questions worth asking ("they mentioned changing auth flow — March notes say that needs client sign-off"). Constraints: *glanceable, never interrupting* (quiet panel, no notifications/motion), opt-in per meeting, token budget, and a high quality bar — one sharp history-grounded question beats a stream of filler; suppress anything generic. Needs mature project history to be sharp; sequence late
- **Background workflows** (scheduled/triggered Claude Code runs over the MCP tools — prompts + a scheduler, no new infrastructure). Rule: **draft and annotate, never silently destroy** (supersede-and-link, not delete; drafts, not sent emails). Candidates:
  1. *Contradiction reconciliation:* new content checked against existing knowledge; superseded decisions auto-marked with a link to what replaced them — the KB stays true, not just full
  2. *Status report drafting:* weekly per-project draft from meetings, notes, closed commitments, commits — in the user's voice, ready to edit
  3. *Self-building dossiers:* pages for people/clients/systems that assemble from mentions (what Tyler owns, everything about the payment webhook, chronologically)
  4. *Open-question tracking:* questions raised in meetings tracked until later content answers them; unanswered ones resurface in prep briefs
  5. *Inbox self-draining:* low-confidence items re-evaluated as context accumulates; the inbox trends toward empty
  6. *Living onboarding doc:* per-project "explain this to a newcomer" document that regenerates as the project evolves
- Dictated voice memos (hotkey → speak a thought → transcribed and routed like any note)
- Meeting context feeding Claude Code coding sessions (meetings as ambient context for the coding agent)
- **Meeting auto-detection** (opt-in per app: Teams, Meet, Zoom…):
  - Primary signal: mic-in-use by process (Windows audio session APIs / ConsentStore) — `ms-teams.exe` grabs mic = meeting started; mic released = ended
  - Disambiguator: window/tab title patterns (browser mic use → confirm "Meet – …" title). Each enabled app = one (process + title) rule; new apps are config, not features
  - Calendar cross-reference labels the capture ("OKIES standup") and primes the categorizer — labeling, not detection
  - UX rule: **auto-detect, never auto-silently-record** — detection fires a "Meeting detected — capturing" notification with one-tap cancel (or ask-first mode)
- **Speaker identity** (staged; each stage stands alone):
  1. *Active-speaker scraping:* UI Automation reads who's highlighted in the Teams/Meet window, timestamped and aligned with the transcript → real names, zero audio ML. Brittle per app-redesign (scraper rules), but beats diarization when it works
  2. *Claude attribution in the post-pass:* given the roster (calendar invite / window), infer speakers from context ("Thanks, Tyler", self-introductions) and reconcile anonymous clusters with names
  3. *Local diarization:* speaker clustering via sherpa-onnx pipelines (pyannote/Sortformer-class); note mixed system audio is one lossy channel — harder than room-mic diarization
  4. *Post-meeting naming + voice profiles:* review UI shows transcript grouped by speaker (snippet or audio clip each) → tap to name (autocomplete from project people), all lines rename; naming saves a local per-person voice embedding so future meetings auto-recognize that voice. Every correction is training data — same philosophy as the routing loop and glossary. Include "this is me" (merges with mic-channel signal). Voice profiles are biometric-ish: local only, stored in the knowledge-base folder (sync with it), per-person delete button
- macOS / Linux ports (community)

## 7. Open Decisions

| Decision | Options | Lean | Deadline |
|---|---|---|---|
| ~~Name~~ | — | **DECIDED: Kodabi** (renamed from "Kodama", 2026-07 — namespace conflicts; see §Why) | ✅ Closed |
| ~~License~~ | — | **DECIDED: AGPL-3.0-only** | ✅ Closed |
| ~~Frontend stack~~ | — | **DECIDED: React + Tailwind** | ✅ Closed |
| ~~Default STT engine~~ | — | **DECIDED: Parakeet TDT (sherpa-onnx)** (real-meeting benchmark 2026-07-15 — silence-safe, ~10× faster, no content-accuracy deficit; whisper.cpp stays the fallback. See `docs/benchmarks/stt-engine-benchmark.md`) | ✅ Closed |
| Embedding model | bge-small / nomic-embed / other | Benchmark retrieval quality on real notes | Phase 2 |
| ~~Audio retention default~~ | — | **DECIDED: audio is not persisted in v1** (only transcript + timestamps); an opt-in audio-retention toggle is deferred until a use case pulls it, at which point the retention policy must cover it. Transcript retention (distill then discard after N days) stays the v1 policy. | ✅ Closed |
| Claude Code invocation | headless CLI vs Agent SDK | Verify current docs at implementation time | Phase 3 |

## 8. Risks & mitigations

- **Subscription-auth terms shift** → API keys are a first-class equal path through Claude Code's own auth; the tool never depends on one mechanism.
- **Categorization annoys instead of delights** → confidence split + Inbox + correction loop from day one; never silently misfile.
- **Scope sprawl ("second brain" junk drawer)** → anti-scope list is binding; Phase 5 items require a month of reliable daily use first.
- **Transcription quality on real calls** → glossary biasing + cleanup pass + real-meeting benchmarking of both engines, early.
- **Whisper silence hallucination** (phantom text during pauses — endemic to meeting audio) → Parakeet default; VAD mandatory on any Whisper path.
- **Design debt** → aesthetic locked in Phase 0 before screens exist.
- **Legal exposure (recording)** → consent nudge, unambiguous indicator, README disclosure, retention policy.
