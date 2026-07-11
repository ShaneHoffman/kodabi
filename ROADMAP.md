# Kodama — Roadmap (Phases 2–5)

Phases 0 and 1 are tracked as individual, agent-actionable tickets in the Kangentic backlog.
This file holds the **later phases as unrefined planning material** — goals, milestones, and
checklists that get broken into tickets when their phase comes up. Vision + architecture source
of truth is `FOUNDING_DOC.md`; this is the working roadmap derived from it.

**Decisions already locked that affect later phases:** License = AGPL-3.0 · Frontend = React +
Tailwind · Transcription = per-channel (you/them attribution) · Glossary-cleanup post-pass pulled
forward into Phase 1.

---

## Phase 2 — Distill, route, store, index
**Goal:** Turn a raw transcript into a routed, stored, searchable note — plus quick-capture notes through the same pipeline.
**Milestone:** the v1 definition-of-done sentence is true, **minus chat**.

- [ ] End-of-meeting pipeline: summary → action items / decisions extraction (the glossary-cleanup step is already pulled into Phase 1 as the post-pass)
- [ ] Confidence-split routing; Inbox UI; one-click re-route feeding the correction loop
- [ ] Quick-capture window (global hotkey → text box → same routing pipeline) + basic note create/edit within a project
- [ ] Markdown writer (frontmatter schema from Phase 0)
- [ ] SQLite schema: FTS5 + sqlite-vec; local embedding pipeline; file watcher; full rebuild command
- [ ] Hybrid retrieval (RRF merge) exposed as `search_notes` MCP tool
- [ ] Retention policy setting + consent nudge

## Phase 3 — The brain (MCP + Claude Code)
**Goal:** Wire Claude Code into the knowledge base via the MCP server; deliver chat over real history.
**Milestone:** "What's outstanding on Briarwood Golf?" answered correctly in-app from real meeting history. **← Dogfood daily from here.**

- [ ] MCP server (stdio) exposing the v1 tool surface
- [ ] Embedded xterm.js terminal running Claude Code with the MCP server preconfigured
- [ ] Chat sessions distilled + filed + indexed as first-class documents
- [ ] Designed chat UI driving Claude Code headless (same stack, second skin)

## Phase 4 — Polish & open-source launch
**Goal:** Production polish + public open-source launch.
**Milestone:** *Undefined in the founding doc — needs a "launch-ready" definition.* Implied: a signed, onboarded, documented Windows release, launched publicly.

- [ ] Design pass on every screen against the locked system; the distill-and-route transition
- [ ] Onboarding: first project, glossary seeding, hotkey setup, consent nudge
- [ ] README with screenshots, architecture doc (trimmed founding doc), contribution guide
- [ ] Windows installer / signing — includes code-signing **certificate procurement** (long lead time: start this early in the phase); crash-reporting decision (opt-in only)
- [ ] Launch: GitHub, relevant communities

## Phase 5 — Parking Lot (growth candidates)
Pulled by daily use, not pushed by roadmap. Each earns its place only after the core loop proves reliable. Names only:

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
