# Kodabi — Roadmap (Phases 2–5)

Phases 0 and 1 are tracked as individual, agent-actionable tickets in the Kangentic backlog.
This file holds the **later phases as unrefined planning material** — goals, milestones, and
checklists that get broken into tickets when their phase comes up. Vision + architecture source
of truth is `docs/FOUNDING_DOC.md`; this is the working roadmap derived from it.

**Decisions already locked that affect later phases:** License = AGPL-3.0-only · Frontend = React +
Tailwind · Transcription = per-channel (you/them attribution) · Default STT engine = Parakeet TDT,
selected at build time via mutually exclusive cargo features (release builds ship Parakeet and fail
to compile without a real engine; the whisper.cpp fallback is deferred for v1 — blocked on Windows
by the sherpa-onnx/ORT mismatch, board #53, see `docs/RESOURCE_BUDGET.md`) · Glossary-cleanup
post-pass pulled forward into Phase 1.

---

## Phase 2 — Distill, route, store, index
**Goal:** Turn a raw transcript into a routed, stored, searchable note — plus quick-capture notes through the same pipeline.
**Milestone:** the v1 definition-of-done sentence is true, **minus chat**.

- [ ] End-of-meeting pipeline: glossary cleanup at transcription time (Phase 1 post-pass), then a single headless-Claude distill call returning summary, action items, and decisions as one structured result
- [x] Distill token budgeting: chunk and map-reduce transcripts that exceed the distill input character budget; a 2-hour meeting must distill, not error (#59 `feat/distill-token-budget`)
- [ ] Confidence-split routing; Inbox UI; one-click re-route that **records** each correction as a routing example (`_routing_examples.yml`, in the KB folder) **and feeds them back** as an additive similarity signal so a correction changes future routing (#56 `feat/routing-examples-signal`) — wiring routing into the distill pipeline tracked as #55 `feat/wire-distill-routing`
- [ ] Incremental capture durability: flush audio/segments to a **transient** on-disk recovery spool during capture (cleared once the transcript lands; recovered on next startup after a crash) so a crash mid-meeting loses at most the last flush interval, and memory stays bounded for multi-hour meetings (#57 `feat/incremental-capture-flush`). The spool is not the *retained recording* — that is a separate `.wav` artifact the note↔source pairing keeps, governed by the retention policy like the transcript (`docs/FILENAME_SCHEME.md`)
- [ ] Quick-capture window (global hotkey → text box → same routing pipeline) + basic note create/edit within a project
- [ ] Markdown writer (frontmatter schema from Phase 0)
- [ ] Frontmatter-validator Claude Code skill (check emitted notes against `docs/FRONTMATTER_SCHEMA.md`) — build alongside the markdown writer, its first real consumer
- [ ] SQLite schema: FTS5 + sqlite-vec; local embedding pipeline; file watcher; full rebuild command
- [ ] Hybrid retrieval (RRF merge) exposed as `search_notes` MCP tool
- [ ] Retention policy setting + consent nudge
- [x] Document (and where possible disable) transcript retention inside Claude Code's own session logs, so the in-app retention policy's promise is complete — both the headless distill runner (`kodabi-llm`) and the embedded terminal (`terminal_cmds`) set `CLAUDE_CODE_SKIP_PROMPT_HISTORY` on the spawned `claude`, disabling its own transcript/prompt-history writing; documented in FOUNDING_DOC §3.7

## Phase 3 — The brain (MCP + Claude Code)
**Goal:** Wire Claude Code into the knowledge base via the MCP server; deliver chat over real history.
**Milestone:** "What's outstanding on Briarwood Golf?" answered correctly in-app from real meeting history. **← Dogfood daily from here.**

- [ ] MCP server (stdio) exposing the v1 tool surface
  - [x] `crates/kodabi-mcp` stands up the stdio server (hand-rolled JSON-RPC) with the first three read tools: `search_notes`, `get_note`, `list_projects` (`get_note`'s `meeting`/`action_items` fields backed by the note index: decisions + action items parsed from the body, duration + speaker count from the session transcript)
  - [x] Write tools `file_note_to_project` and `add_glossary_term` close the human correction loop from chat, wrapping the same `vault::file_note_to_project` path the Inbox UI uses (open windows converge via the file watcher's reconcile)
  - [x] `get_meeting_transcript`, `list_outstanding_items`, and `get_project_context` complete the six-tool read surface: transcripts page the note↔source pairing seam keyed on the segment ordinal; outstanding items query the index's action-item table across notes, filtering derived open/overdue status in SQL so keyset pagination and the cross-page totals agree with what each item renders; project context composes the disk-backed project, README, and glossary with index-backed counts, recent notes, and outstanding items, hard-capped per section with true totals in `counts` (the one documented pagination exception)
- [ ] Routing reads recorded corrections as an additive scoring signal — a correction must measurably change future routing (#56 `feat/routing-examples-signal`)
- [x] Embedded xterm.js terminal running Claude Code with the MCP server preconfigured — an in-app xterm.js view over a ConPTY PTY (`portable-pty`) hosting interactive `claude`, wired to the `kodabi` MCP server via a generated machine-local `.mcp.json` (read tools pre-approved, writes still prompt); KB root and index resolved from app config. `kodabi-mcp` resolves from a sibling of the app exe in dev/release-from-source, and the installer now carries it too, via `bundle.resources` in the `src-tauri/tauri.bundle.conf.json` overlay that `pnpm tauri:build` applies (an overlay rather than the base config because `tauri-build` validates every resource path at *each* compile, which the bare cargo gates would fail on)
- [x] Chat sessions distilled + filed + indexed as first-class documents — an ended chat's `chats/*.jsonl` transcript runs a chat-flavored distill (`kodabi-core`'s `chat_distill`, sharing the meeting pass's `RESPONSE_SHAPE_SPEC`, chunking, routing, and note write) into a `type: chat` note carrying `source: chats/<file>.jsonl`, routed by the same confidence split and indexed by the file watcher. Only what the user saw reaches the prompt: turns as prose, tool calls as their rendered summary, never a tool's raw input. Triggered on restart and on the CLI exiting, plus a startup sweep that derives "not yet distilled" from note `source:` values — which is what covers a chat ended by quitting or by a crash, and what makes a failed distill retry itself. A conversation under `MIN_CHAT_DISTILL_CHARS` is skipped rather than filed. **The raw transcript is never pruned:** `chats/` stays outside the retention sweep, unlike `sessions/`
- [x] Designed chat UI driving Claude Code headless (same stack, second skin) — a full-height chat view (sidebar + palette entries) over one long-lived `claude -p` in bidirectional stream-json mode, wired to the `kodabi` MCP server with built-in tools disabled and the read tools pre-approved; answers stream onto the reading ramp, tool use shows as quiet status lines, and MCP write tools raise an inline Allow/Deny card driven by the CLI's `can_use_tool` control protocol (every non-answer path resolves to deny). Resolves FOUNDING_DOC §7's headless-CLI-vs-Agent-SDK decision in favor of the CLI

## Phase 4 — Polish & open-source launch
**Goal:** Production polish + public open-source launch.
**Milestone:** a signed, onboarded, documented Windows release, launched publicly.

- [ ] Design pass on every screen against the locked system; the distill-and-route transition
- [ ] Onboarding: first project, glossary seeding, hotkey setup, consent nudge
- [ ] README with screenshots, architecture doc (trimmed founding doc), contribution guide
- [ ] Windows installer / signing — includes code-signing **certificate procurement** (long lead time: start this early in the phase); crash-reporting decision (opt-in only)
- [ ] Launch: GitHub, relevant communities

## Phase 5 — Parking Lot (growth candidates)
Pulled by daily use, not pushed by roadmap. Each earns its place only after the core loop proves reliable. Names only — the full detail behind each candidate lives in `FOUNDING_DOC.md` §6:

- Commitment ledger (flagship)
- Azure DevOps board integration
- Weekly digests per project
- Decision log queries
- Pre-meeting prep briefs (auto-triggered by meeting detection)
- Live meeting assistance
- Background workflows: contradiction reconciliation; status report drafting; self-building dossiers; open-question tracking; inbox self-draining; living onboarding doc
- Dictated voice memos
- Meeting context feeding Claude Code coding sessions
- Meeting auto-detection (Teams / Meet / Zoom, mic-in-use signal)
- Speaker identity: active-speaker scraping; Claude attribution in post-pass; local diarization; post-meeting naming + voice profiles
- macOS / Linux ports (community)
