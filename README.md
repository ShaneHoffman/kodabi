# Kodama

A self-organizing personal knowledge base. Kodama turns meeting transcripts and quick notes into a
searchable, chat-able knowledge base: **transcribe → distill → auto-route → search & chat**.

## Status

Pre-alpha — early development, no releases yet. See [`ROADMAP.md`](ROADMAP.md) for the phased plan.

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

## License

Kodama is free software licensed under the **GNU Affero General Public License, version 3**
(`AGPL-3.0-only`). You may redistribute and/or modify it under the terms of version 3 of the License
as published by the Free Software Foundation. See [`LICENSE`](LICENSE) for the full text.

Copyright (C) 2026 Shane Hoffman
