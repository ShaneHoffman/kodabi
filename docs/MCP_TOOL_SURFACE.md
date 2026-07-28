# Kodabi — MCP Tool Surface (v1)

**Status:** Spec (Phase 0, ticket P0-10). Defines the stdio MCP tool surface the Phase 3 server
implements and the Phase 2 `search_notes` retrieval builds against. Schemas are the contract;
shapes descend from the [`FRONTMATTER_SCHEMA.md`](FRONTMATTER_SCHEMA.md) fields (id, type,
project, date, tags, source, confidence). The candidate tool list originates in
[`FOUNDING_DOC.md`](FOUNDING_DOC.md) §3.2.

> **North star:** one small, sharp set of tools that lets Claude Code answer "what's outstanding
> on \<project\>?" over real meeting history.

This document is spec/design only — no server code lands here. It is verified against the current
Claude Code MCP reference (`code.claude.com/docs/en/mcp`) and the MCP tool specification
(`modelcontextprotocol.io/specification/2025-06-18/server/tools`).

---

## Server & wiring

- **Server name:** `kodabi`. **Transport:** local **stdio**. The process is spawned by the Tauri
  shell and preconfigured for the embedded Claude Code terminal (Phase 3).
- Tools are callable as **`mcp__kodabi__<tool>`** — Claude Code's namespacing for a server named
  `kodabi` with no plugin wrapper.
- The `.mcp.json` shape (the embedded terminal generates this machine-local at runtime — see
  `src-tauri/src/terminal_cmds.rs`; the repo's `.mcp.json.example` is the template):

  ```json
  {
    "mcpServers": {
      "kodabi": {
        "command": "<path-to-kodabi-mcp-binary>",
        "args": [],
        "env": {
          "KODABI_INDEX_DB": "<app-data>/index.db",
          "KODABI_KB_ROOT": "<knowledge-base-root>"
        }
      }
    }
  }
  ```

  An entry with no `type`/`url` field is read by Claude Code as a stdio server. The server resolves
  the knowledge-base root and the index from its own config — the two env vars above, injected by the
  Tauri shell from app config (not from the working directory), read by `crates/kodabi-mcp/src/config.rs`.
  It may implement the MCP `roots/list` request if it wants to bound its own filesystem access to
  Claude Code's granted directories.
- **Permissions.** The *implemented* read tools are pre-approved as a group (the single source is
  `READ_TOOL_PERMISSIONS` in `crates/kodabi-core/src/terminal.rs` — a read tool added to the server
  must be added there too, or it will prompt), and the write tools (e.g.
  `"mcp__kodabi__file_note_to_project"`, `"mcp__kodabi__add_glossary_term"`) are deliberately left
  off the allow-list, so approval stays meaningful per-tool. The server performs no confirmation of
  its own; approval is entirely Claude Code's permission model — and how the prompt reaches the user
  depends on the consumer (below).
- **Two in-app consumers share this wiring**, both spawned against the same generated `.mcp.json`
  with `--strict-mcp-config`, both with `CLAUDE_CODE_SKIP_PROMPT_HISTORY=1`:
  - the **embedded terminal** (`src-tauri/src/terminal_cmds.rs`): interactive `claude` in a PTY,
    read tools pre-approved via a generated `--settings` file; a write tool's permission prompt has
    a real TTY to answer.
  - the **designed chat view** (`src-tauri/src/chat_cmds.rs`): headless `claude -p` in
    bidirectional stream-json mode, built-in tools disabled (`--tools ""`), read tools pre-approved
    via `--allowedTools`. There is no TTY, so `--permission-prompt-tool stdio` routes a write tool's
    request onto stdout as a `can_use_tool` control request; the chat renders it as an inline
    Allow/Deny card and writes the decision back over stdin. A prompt lost to a stop, restart, or
    app exit always resolves to deny.
  Both may run at once: two `claude → kodabi-mcp` process pairs over the same index and vault is
  fine (SQLite reads are concurrent, and writes go through the vault paths the file watcher
  reconciles).
- Keep every tool `description` and the server's own instructions string **under 2 KB** — Claude
  Code truncates both at that size, and truncation would silently drop the "when to use this"
  guidance a tool-search-driven client relies on.

---

## Conventions

- All schemas are **JSON Schema draft 2020-12** (`"$schema": "https://json-schema.org/draft/2020-12/schema"`).
- Every tool definition carries `name`, `title`, `description`, `inputSchema`, `outputSchema`, and
  `annotations` — the fields the MCP spec defines for a `Tool` object.
- Every successful call returns **`structuredContent`** conforming to the tool's `outputSchema`,
  and — per the MCP spec's backward-compatibility requirement — the same result **serialized as JSON
  in a `content` text block**, so clients that don't parse `structuredContent` still receive the full
  data. A short human-readable summary may precede it in the block, but must not replace the
  serialized payload.
- Plain objects use `"additionalProperties": false`. Schemas that **extend** another via `allOf`
  composition (`SearchHit`, `MeetingMeta`) use **`"unevaluatedProperties": false` instead** —
  their own `additionalProperties` could not see the base's `$ref`'d fields, while
  `unevaluatedProperties` sees across `allOf`/`$ref`. The **base being extended** (`NoteSummary`)
  carries **neither keyword and stays open**: under draft 2020-12, annotations never flow from a
  parent schema into a `$ref`'d subschema, so a base closed with either keyword would treat the
  extension's added fields (`score`, `duration_seconds`, …) as unknown and reject every composed
  instance. Strictness on standalone `NoteSummary` uses is deliberately traded away to keep the
  composition valid; the extending schemas re-close the full shape.
- Nullability uses draft 2020-12 union types (`"type": ["string", "null"]`) or
  `"oneOf": [{"$ref": "..."}, {"type": "null"}]` for `$ref`'d types.
- Errors follow the [Cross-cutting contract](#cross-cutting-contract) below, not ad hoc shapes.

---

## Tool index

| Tool | Title | Access | Purpose |
| --- | --- | --- | --- |
| `search_notes` | Search notes | read | Hybrid FTS + vector search → ranked snippets |
| `get_note` | Get note | read | Full distilled note body + metadata by id |
| `get_meeting_transcript` | Get meeting transcript | read | Per-channel transcript segments for a meeting |
| `list_outstanding_items` | List outstanding items | read | Not-done action items, linked to source note |
| `list_projects` | List projects | read | Enumerate projects (hierarchy, counts) |
| `get_project_context` | Get project context | read | Aggregate briefing for one project |
| `file_note_to_project` | File note to project | **write** | Route/re-route a note (the correction loop) |
| `add_glossary_term` | Add glossary term | **write** | Upsert a project glossary term |

`get_note` is not in the ticket's §3.2 candidate list; it closes a gap the seven candidates leave
open (see [What this hands downstream](#what-this-hands-downstream) and the milestone walkthrough
below) — `search_notes` returns only snippets, and Phase 3 chat needs to quote full note content.

All `annotations` objects use the four MCP tool-annotation hints: `readOnlyHint`, `destructiveHint`,
`idempotentHint`, `openWorldHint`. The six reads set `readOnlyHint: true`; both writes set
`readOnlyHint: false` and `destructiveHint: false` (mutating, but reversible with no data loss).

---

## The tools

### 1. `search_notes`

Hybrid full-text + semantic retrieval over the whole knowledge base, returning ranked note hits
with snippets.

- **title:** `Search notes`
- **description:** `Hybrid full-text + semantic search across all notes. Returns ranked hits with snippets. Filter by project (and subtree), note type, tags, and date range; page with limit + cursor.`

**inputSchema**
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["query"],
  "properties": {
    "query": { "type": "string", "minLength": 1, "description": "Natural-language or keyword query. Fed to both the FTS5 and vector indexes; results merged via RRF." },
    "project": { "$ref": "#/$defs/ProjectSlug", "description": "Restrict to this project. Also matches descendants unless include_descendants is false. The reserved slug \"Inbox\" (any casing) is accepted here and matches unfiled notes (those whose project is null) rather than a real project." },
    "include_descendants": { "type": "boolean", "default": true, "description": "When project is set, also search nested sub-projects (e.g. 'Growth' includes 'Growth/Q3'). Ignored for the \"Inbox\" sentinel, which has no descendants." },
    "type": { "type": "array", "items": { "$ref": "#/$defs/NoteType" }, "uniqueItems": true, "maxItems": 64, "description": "Restrict to these note types. Omit for all. Repeats are ignored." },
    "tags": { "type": "array", "items": { "type": "string", "minLength": 1 }, "uniqueItems": true, "maxItems": 64, "description": "Restrict to notes carrying these tags. Repeats are ignored; more than 64 distinct tags is an error, not a silent truncation." },
    "tag_match": { "type": "string", "enum": ["any", "all"], "default": "any", "description": "Whether a hit must carry any or all of the listed tags." },
    "date_from": { "$ref": "#/$defs/IsoDate", "description": "Inclusive lower bound on note date, compared against the note's local calendar day as frontmatter stores it (not its UTC instant), so an evening note stays on the day the user saw it." },
    "date_to": { "$ref": "#/$defs/IsoDate", "description": "Inclusive upper bound on note date, compared against the note's local calendar day as frontmatter stores it (not its UTC instant)." },
    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "default": 10, "description": "Max hits in this page." },
    "cursor": { "type": "string", "description": "Opaque pagination token from a prior response's page.next_cursor." }
  }
}
```

**outputSchema**
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["hits", "page"],
  "properties": {
    "hits": { "type": "array", "items": { "$ref": "#/$defs/SearchHit" }, "description": "Ranked note hits, best first." },
    "page": { "$ref": "#/$defs/PageInfo" }
  }
}
```

**annotations**
```json
{ "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }
```
Read-only query over the local index; no mutation; identical input yields identical ranking; the
world is the local knowledge base only.

---

### 2. `get_note`

Fetch a note's full distilled content by stable id — frontmatter metadata plus the rendered
markdown body, and for meetings, extracted decisions and action items. Use after `search_notes` to
read a hit in full.

- **title:** `Get note`
- **description:** `Fetch a note's full distilled content by stable id: frontmatter metadata plus the rendered markdown body. For meetings, also returns extracted decisions and action items. Use after search_notes to read a hit in full.`

**inputSchema**
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["id"],
  "properties": {
    "id": { "$ref": "#/$defs/NoteId", "description": "Stable id of the note to read." },
    "include_body": { "type": "boolean", "default": true, "description": "Include the full distilled markdown body. Set false for metadata + decisions + action items only." },
    "include_action_items": { "type": "boolean", "default": true, "description": "Include extracted action items (meetings only)." }
  }
}
```

**outputSchema**
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["note", "meeting", "body_markdown", "action_items"],
  "properties": {
    "note": { "$ref": "#/$defs/NoteSummary", "description": "Note metadata (frontmatter-derived)." },
    "meeting": { "oneOf": [ { "$ref": "#/$defs/MeetingMeta" }, { "type": "null" } ], "description": "Meeting metadata (including extracted decisions) when type is meeting; null otherwise. Always present." },
    "body_markdown": { "type": ["string", "null"], "description": "The note's distilled markdown body, or null when include_body is false." },
    "action_items": { "type": "array", "items": { "$ref": "#/$defs/ActionItem" }, "description": "Extracted action items (empty for non-meetings or when include_action_items is false)." }
  }
}
```

**annotations**
```json
{ "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }
```
Read-only fetch of stored content; closed world. A note id that does not exist returns an
`isError` result (see [Cross-cutting contract](#cross-cutting-contract)), not this success shape.

---

### 3. `get_meeting_transcript`

Return the raw per-channel transcript segments and metadata for a meeting note, addressed by its
stable id.

- **title:** `Get meeting transcript`
- **description:** `Fetch the per-channel transcript (you/them attribution, millisecond offsets) and metadata for a meeting note by stable id. Returns transcript_available=false with empty segments when no transcript is stored; errors if the id is not a meeting note.`

**inputSchema**
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["id"],
  "properties": {
    "id": { "$ref": "#/$defs/NoteId", "description": "Stable id of the meeting note whose transcript to fetch." },
    "include_metadata": { "type": "boolean", "default": true, "description": "Include meeting metadata (duration, speaker count, decisions) in the response." },
    "limit": { "type": "integer", "minimum": 1, "maximum": 1000, "default": 200, "description": "Max transcript segments per page (segments are ordered by start time)." },
    "cursor": { "type": "string", "description": "Opaque pagination token from a prior response's page.next_cursor." }
  }
}
```

**outputSchema**
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["note", "transcript_available", "segments", "page"],
  "properties": {
    "note": { "$ref": "#/$defs/NoteRef" },
    "meeting": { "oneOf": [ { "$ref": "#/$defs/MeetingMeta" }, { "type": "null" } ], "description": "Meeting metadata, or null when include_metadata is false." },
    "transcript_available": { "type": "boolean", "description": "False when the note is a meeting but has no stored transcript; segments will be empty." },
    "segments": { "type": "array", "items": { "$ref": "#/$defs/TranscriptSegment" }, "description": "Transcript segments for this page, ordered by start_ms ascending." },
    "page": { "$ref": "#/$defs/PageInfo" }
  }
}
```

**annotations**
```json
{ "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }
```
Read-only fetch of stored data; closed world. Not-found id and "not a meeting" are returned as
`isError` results, not as this success shape.

---

### 4. `list_outstanding_items`

List action items that are not yet done (open or overdue), each linked back to its source meeting
note.

- **title:** `List outstanding items`
- **description:** `List action items that are not done (open/overdue), extracted from meetings and linked to their source note. Filter by project subtree, owner, status, due-before date, or source meeting.`

**inputSchema**
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "project": { "$ref": "#/$defs/ProjectSlug", "description": "Restrict to items whose source note is in this project." },
    "include_descendants": { "type": "boolean", "default": true, "description": "When a project filter is set, also include items from nested sub-projects." },
    "owner": { "type": "string", "minLength": 1, "description": "Restrict to items with this owner (e.g. \"you\" or a person's name)." },
    "status": { "type": "array", "items": { "$ref": "#/$defs/ActionItemStatus" }, "uniqueItems": true, "default": ["open", "overdue"], "description": "Statuses to include. Defaults to the not-done set (open + overdue)." },
    "due_before": { "$ref": "#/$defs/IsoDate", "description": "Only items with a due date strictly before this date (items with no due date are excluded when this is set)." },
    "source_note_id": { "$ref": "#/$defs/NoteId", "description": "Restrict to items extracted from this specific meeting note." },
    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 50, "description": "Max items per page." },
    "cursor": { "type": "string", "description": "Opaque pagination token from a prior response's page.next_cursor." }
  }
}
```

**outputSchema**
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["items", "summary", "page"],
  "properties": {
    "items": { "type": "array", "items": { "$ref": "#/$defs/ActionItem" }, "description": "Matching action items, ordered by due date ascending (undated last)." },
    "summary": {
      "type": "object",
      "additionalProperties": false,
      "required": ["open", "overdue", "done"],
      "properties": {
        "open": { "type": "integer", "minimum": 0, "description": "Total matching items with status open (across all pages)." },
        "overdue": { "type": "integer", "minimum": 0, "description": "Total matching items with status overdue (across all pages)." },
        "done": { "type": "integer", "minimum": 0, "description": "Total matching items with status done (across all pages; 0 unless done was requested)." }
      }
    },
    "page": { "$ref": "#/$defs/PageInfo" }
  }
}
```

**annotations**
```json
{ "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }
```
Read-only aggregation over the local index; closed world.

---

### 5. `list_projects`

Enumerate routing-target projects, including hierarchy, counts, and last activity.

- **title:** `List projects`
- **description:** `Enumerate routing-target projects with hierarchy (parent + slug), display name, note/meeting counts, and last activity. Use to resolve a project name to its slug before filtering other tools.`

**inputSchema**
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "parent": { "$ref": "#/$defs/ProjectSlug", "description": "List projects under this parent. Omit to start from the top level." },
    "include_descendants": { "type": "boolean", "default": true, "description": "Include the full subtree (true) or only direct children / top level (false)." },
    "include_empty": { "type": "boolean", "default": true, "description": "Include projects with zero notes." },
    "sort": { "type": "string", "enum": ["name", "last_activity", "note_count"], "default": "name", "description": "Sort order for the returned projects." },
    "limit": { "type": "integer", "minimum": 1, "maximum": 200, "default": 100, "description": "Max projects per page." },
    "cursor": { "type": "string", "description": "Opaque pagination token from a prior response's page.next_cursor." }
  }
}
```

**outputSchema**
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["projects", "page"],
  "properties": {
    "projects": { "type": "array", "items": { "$ref": "#/$defs/Project" }, "description": "Matching projects in the requested sort order." },
    "page": { "$ref": "#/$defs/PageInfo" }
  }
}
```

**annotations**
```json
{ "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }
```
Read-only enumeration; closed world.

---

### 6. `get_project_context`

Return an aggregate briefing for one project — description, glossary, recent notes, outstanding
items, and counts — in a single call.

- **title:** `Get project context`
- **description:** `Aggregate context for one project in a single call: description, glossary, recent notes, outstanding items, and counts. Toggle and limit each section. Ideal for grounding a chat about a project.`

**inputSchema**
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["project"],
  "properties": {
    "project": { "$ref": "#/$defs/ProjectSlug", "description": "The project to summarize." },
    "include_descendants": { "type": "boolean", "default": false, "description": "Aggregate over the project's subtree (true) or just this project (false)." },
    "include_glossary": { "type": "boolean", "default": true, "description": "Include the project glossary." },
    "include_recent_notes": { "type": "boolean", "default": true, "description": "Include recent notes." },
    "include_outstanding": { "type": "boolean", "default": true, "description": "Include not-done action items." },
    "recent_notes_limit": { "type": "integer", "minimum": 0, "maximum": 50, "default": 10, "description": "Max recent notes to return (0 to omit)." },
    "outstanding_limit": { "type": "integer", "minimum": 0, "maximum": 100, "default": 20, "description": "Max outstanding items to return (0 to omit)." },
    "glossary_limit": { "type": "integer", "minimum": 0, "maximum": 500, "default": 200, "description": "Max glossary terms to return (0 to omit)." }
  }
}
```

**outputSchema**
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["project", "counts"],
  "properties": {
    "project": { "$ref": "#/$defs/Project" },
    "description": { "type": ["string", "null"], "description": "Project description / README text, or null if none." },
    "glossary": { "type": "array", "items": { "$ref": "#/$defs/GlossaryTerm" }, "description": "Glossary terms (present when include_glossary; truncated to glossary_limit)." },
    "recent_notes": { "type": "array", "items": { "$ref": "#/$defs/NoteSummary" }, "description": "Most recent notes, newest first (present when include_recent_notes)." },
    "outstanding": { "type": "array", "items": { "$ref": "#/$defs/ActionItem" }, "description": "Not-done action items (present when include_outstanding)." },
    "counts": {
      "type": "object",
      "additionalProperties": false,
      "required": ["notes", "meetings", "notes_by_type", "outstanding_open", "outstanding_overdue", "glossary_terms"],
      "properties": {
        "notes": { "type": "integer", "minimum": 0, "description": "Total notes in scope." },
        "meetings": { "type": "integer", "minimum": 0, "description": "Total meeting-type notes in scope." },
        "notes_by_type": {
          "type": "object",
          "additionalProperties": false,
          "required": ["meeting", "note", "chat"],
          "properties": {
            "meeting": { "type": "integer", "minimum": 0 },
            "note": { "type": "integer", "minimum": 0 },
            "chat": { "type": "integer", "minimum": 0 }
          }
        },
        "outstanding_open": { "type": "integer", "minimum": 0, "description": "Count of open action items in scope." },
        "outstanding_overdue": { "type": "integer", "minimum": 0, "description": "Count of overdue action items in scope." },
        "glossary_terms": { "type": "integer", "minimum": 0, "description": "Total glossary terms for the project." }
      }
    }
  }
}
```

**annotations**
```json
{ "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }
```
Read-only aggregation; closed world.

---

### 7. `file_note_to_project` — *write*

Route or re-route a note to a project — the human correction loop — moving the file and updating
its frontmatter while preserving its stable id.

- **title:** `File note to project`
- **description:** `Route or re-route a note to a project (the human correction loop). Moves the file, updates its frontmatter project + confidence, preserves the stable id, and returns the new path. Mutating but reversible.`

**inputSchema**
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["id", "project"],
  "properties": {
    "id": { "$ref": "#/$defs/NoteId", "description": "Stable id of the note to (re-)route. The id is preserved across the move." },
    "project": { "$ref": "#/$defs/ProjectSlug", "description": "Target project slug/path. The note's file moves into this project's folder." },
    "create_project": { "type": "boolean", "default": false, "description": "If the target project does not exist, create it (and any missing parents). When false, a missing target is an error." },
    "confidence": { "type": "number", "minimum": 0, "maximum": 1, "description": "Override the routing confidence written to frontmatter. Omit to record a human correction as 1.0." },
    "reason": { "type": "string", "maxLength": 500, "description": "Optional human-readable reason for the correction, stored as the note's last-correction note (overwrites any prior)." }
  }
}
```

**outputSchema**
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["note", "previous", "moved"],
  "properties": {
    "note": { "$ref": "#/$defs/NoteSummary", "description": "The note after routing, with its new path, project, and confidence." },
    "previous": {
      "type": "object",
      "additionalProperties": false,
      "required": ["path", "project"],
      "properties": {
        "path": { "type": "string", "description": "The note's relative path before the move." },
        "project": { "oneOf": [ { "$ref": "#/$defs/ProjectSlug" }, { "type": "null" } ], "description": "The note's project before the move (null if it was unfiled, e.g. in the Inbox)." }
      }
    },
    "moved": { "type": "boolean", "description": "False when the note was already in the target project (no file move occurred)." }
  }
}
```

**annotations**
```json
{ "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }
```
Mutating (`readOnlyHint: false`) but reversible with no data loss (`destructiveHint: false`);
re-filing the same note to the same project repeatedly converges to one end state
(`idempotentHint: true`) because `reason`/`confidence` overwrite rather than append; local-only.
Approval is handled entirely by Claude Code's permission prompt (see
[Cross-cutting contract](#cross-cutting-contract)).

---

### 8. `add_glossary_term` — *write*

Add or update a glossary term for a project, used for transcription biasing and cleanup.

- **title:** `Add glossary term`
- **description:** `Add or update a glossary term (term, definition, aliases) for a project. Upsert by normalized term. Used for transcription biasing and post-pass cleanup.`

**inputSchema**
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["project", "term", "definition"],
  "properties": {
    "project": { "$ref": "#/$defs/ProjectSlug", "description": "Project whose glossary receives the term." },
    "term": { "type": "string", "minLength": 1, "maxLength": 200, "description": "The term or phrase (e.g. an acronym, product, or person). Matched case-insensitively for upsert." },
    "definition": { "type": "string", "minLength": 1, "maxLength": 2000, "description": "Definition or expansion of the term." },
    "aliases": { "type": "array", "items": { "type": "string", "minLength": 1 }, "uniqueItems": true, "default": [], "description": "Alternative spellings/forms that should map to this term." },
    "on_conflict": { "type": "string", "enum": ["update", "error"], "default": "update", "description": "When the term already exists: update it in place, or return an error." }
  }
}
```

**outputSchema**
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["term", "created"],
  "properties": {
    "term": { "$ref": "#/$defs/GlossaryTerm", "description": "The stored term after the upsert." },
    "created": { "type": "boolean", "description": "True if a new term was created; false if an existing term was updated." }
  }
}
```

**annotations**
```json
{ "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }
```
Mutating but non-destructive; upsert keyed on the normalized term means identical inputs converge
to one state; local-only.

---

## Shared schema library (`$defs`)

Presented once here as the canonical library. The server inlines, per emitted `tools/list` entry,
the transitive subset of `$defs` each tool references, so each schema is self-contained, with
`$ref` pointing at `#/$defs/<Name>`.

```json
{
  "$defs": {
    "NoteId": {
      "type": "string",
      "pattern": "^n_[0-9a-z]{6,}$",
      "description": "Stable note identifier, e.g. \"n_a1b2c3\". Generated at creation, never rewritten on move/re-route. This is the write handle."
    },
    "ProjectSlug": {
      "type": "string",
      "minLength": 1,
      "maxLength": 300,
      "pattern": "^[^/\\\\]+(?:/[^/\\\\]+)*$",
      "description": "Hierarchical project path/slug relative to the KB root, e.g. \"Growth/Q3\". Segments are folder names; no leading/trailing or empty segments. Each segment must also be a legal Windows folder name (no reserved device names such as CON/PRN/NUL, no trailing dot or space, none of the characters Windows forbids in a path segment), and may not start with \".\" or \"_\" — those prefixes mark infra folders (\".obsidian\", \"_assets\") that routing discovery skips, so such a project would be writable yet invisible to routing. The Phase 2 writer rejects violations. \"Inbox\" (any casing) is a reserved folder name — a real project may not use it — and \"sessions\", \"raw\", and \"chats\" (any casing) are reserved as first segments: <KB root>/sessions/ holds raw session artifacts and <KB root>/chats/ holds raw chat transcripts, never notes (\"raw\" stays reserved alongside them; a nested segment like \"Data/raw\" is fine). This is the canonical project handle accepted by tools. Because no real project can be named \"Inbox\", read tools that filter by project reuse it as a sentinel meaning \"unfiled\" — see search_notes's project field."
    },
    "IsoDate": {
      "type": "string",
      "format": "date",
      "description": "Calendar date, ISO 8601 / RFC 3339 full-date, e.g. \"2026-07-11\"."
    },
    "IsoDateTime": {
      "type": "string",
      "format": "date-time",
      "description": "Timestamp, RFC 3339 date-time with offset (\"Z\" or numeric), e.g. \"2026-07-11T14:03:00Z\" or \"2026-07-09T14:00:00-07:00\"."
    },
    "NoteType": {
      "type": "string",
      "enum": ["meeting", "note", "chat"],
      "description": "The note's frontmatter type."
    },
    "Channel": {
      "type": "string",
      "enum": ["you", "them", "unknown"],
      "description": "Per-channel attribution for a transcript segment (local-mic = you, remote = them)."
    },
    "ActionItemStatus": {
      "type": "string",
      "enum": ["open", "overdue", "done"],
      "description": "Action-item status. 'overdue' is derived server-side from 'open' + a past due date."
    },
    "PageInfo": {
      "type": "object",
      "additionalProperties": false,
      "required": ["has_more", "next_cursor"],
      "properties": {
        "has_more": { "type": "boolean", "description": "True if more results exist beyond this page." },
        "next_cursor": { "type": ["string", "null"], "description": "Opaque token to pass as the next request's cursor; null when has_more is false." },
        "total_estimate": { "type": ["integer", "null"], "minimum": 0, "description": "Total matches when the underlying candidate pool was exhausted (an exact count), or null when it was not — a truncated pool knows only its own size, which would understate the total. Never a value the caller can treat as a floor for how much is left; use has_more for that." }
      },
      "description": "Cursor-based pagination envelope. Cursors are opaque and mutation-safe (they encode sort key + id, not an offset)."
    },
    "NoteRef": {
      "type": "object",
      "additionalProperties": false,
      "required": ["id", "path"],
      "properties": {
        "id": { "$ref": "#/$defs/NoteId" },
        "path": { "type": "string", "description": "Current note path relative to the KB root, e.g. \"Growth/Q3/weekly-sync.md\". Informational; changes on move." }
      },
      "description": "Minimal stable reference to a note: stable id plus current path."
    },
    "NoteSummary": {
      "type": "object",
      "required": ["id", "path", "title", "type", "project", "date", "tags", "source"],
      "properties": {
        "id": { "$ref": "#/$defs/NoteId" },
        "path": { "type": "string", "description": "Current note path relative to the KB root, e.g. \"Growth/Q3/weekly-sync.md\"; an unfiled (Inbox) note's path begins with \"Inbox/\". The filename is a slug of the title (or the id when the title slugifies to empty)." },
        "title": { "type": "string", "description": "Note display title. The frontmatter `title` when the note carries one (kept in full, past the 40-char filename slug); otherwise the de-slugged filename stem for a legacy or hand-made note without the key." },
        "type": { "$ref": "#/$defs/NoteType" },
        "project": { "oneOf": [ { "$ref": "#/$defs/ProjectSlug" }, { "type": "null" } ], "description": "Owning project slug, or null if unfiled (Inbox). Frontmatter stores the sentinel string \"Inbox\" for the null case." },
        "date": { "oneOf": [ { "$ref": "#/$defs/IsoDateTime" }, { "$ref": "#/$defs/IsoDate" } ], "description": "Frontmatter date, verbatim as stored: full timestamp with the device's local offset (not UTC) when a time is known, local calendar date otherwise. The writer accepts only these two shapes and rejects a naive timestamp with no offset." },
        "tags": { "type": "array", "items": { "type": "string" }, "description": "Frontmatter tags; empty array when the frontmatter key is absent (the writer omits the key for untagged notes, and normalizes a hand-edited empty list to an omitted key). Each tag is lowercase kebab-case." },
        "source": { "type": "string", "description": "Frontmatter source: a capture keyword (transcript | quick-capture | chat | import | manual) or a repo-relative path to the raw artifact. Disambiguation: a value exactly equal to a keyword is that keyword; anything else is the path." },
        "confidence": { "type": ["number", "null"], "minimum": 0, "maximum": 1, "description": "Routing confidence 0..1, or null when no routing score exists (hand-filed or imported notes)." }
      },
      "description": "Core note metadata shared by search hits, recent-notes lists, and routing results. Deliberately open (no unevaluatedProperties) because SearchHit and MeetingMeta extend it — see Conventions."
    },
    "SearchHit": {
      "allOf": [ { "$ref": "#/$defs/NoteSummary" } ],
      "unevaluatedProperties": false,
      "required": ["score", "rank", "snippet"],
      "properties": {
        "score": { "type": "number", "description": "Fused RRF relevance score; higher is better." },
        "rank": { "type": "integer", "minimum": 1, "description": "1-based rank within the full result set." },
        "snippet": { "type": "string", "description": "Highlighted excerpt of the matching passage." }
      },
      "description": "A NoteSummary augmented with retrieval score, rank, and snippet. Uses unevaluatedProperties (not additionalProperties) so the composed NoteSummary fields are allowed."
    },
    "MeetingMeta": {
      "allOf": [ { "$ref": "#/$defs/NoteSummary" } ],
      "unevaluatedProperties": false,
      "required": ["duration_seconds", "speaker_count", "decisions", "action_item_count"],
      "properties": {
        "duration_seconds": { "type": ["integer", "null"], "minimum": 0, "description": "Meeting duration in seconds, or null if unknown." },
        "speaker_count": { "type": ["integer", "null"], "minimum": 0, "description": "Distinct speaker count (channel-based; names not resolved in v1), or null." },
        "decisions": { "type": "array", "items": { "type": "string" }, "description": "Extracted decisions from the meeting." },
        "action_item_count": { "type": "integer", "minimum": 0, "description": "Number of action items extracted from this meeting." }
      },
      "description": "Meeting-specific metadata: a NoteSummary plus meeting fields."
    },
    "TranscriptSegment": {
      "type": "object",
      "additionalProperties": false,
      "required": ["index", "channel", "start_ms", "end_ms", "text"],
      "properties": {
        "index": { "type": "integer", "minimum": 0, "description": "0-based ordinal within the transcript." },
        "channel": { "$ref": "#/$defs/Channel" },
        "speaker": { "type": ["string", "null"], "description": "Speaker label if known, else null (diarization is post-v1)." },
        "start_ms": { "type": "integer", "minimum": 0, "description": "Segment start offset in milliseconds from meeting start." },
        "end_ms": { "type": "integer", "minimum": 0, "description": "Segment end offset in milliseconds from meeting start." },
        "text": { "type": "string", "description": "Transcribed (and glossary-cleaned) text of the segment." }
      },
      "description": "One attributed, timestamped transcript segment."
    },
    "ActionItem": {
      "type": "object",
      "additionalProperties": false,
      "required": ["id", "description", "owner", "due_date", "status", "source"],
      "properties": {
        "id": { "type": "string", "pattern": "^a_[0-9a-z]{6,}$", "description": "Stable action-item id, e.g. \"a_9f8e7d\"." },
        "description": { "type": "string", "description": "What is to be done." },
        "owner": { "type": "string", "description": "Who owns it (e.g. \"you\" or a person's name)." },
        "due_date": { "oneOf": [ { "$ref": "#/$defs/IsoDate" }, { "type": "null" } ], "description": "Due date, or null if none." },
        "status": { "$ref": "#/$defs/ActionItemStatus" },
        "source": { "$ref": "#/$defs/NoteRef", "description": "The meeting note this item was extracted from." },
        "extracted_date": { "oneOf": [ { "$ref": "#/$defs/IsoDate" }, { "type": "null" } ], "description": "Date the item was extracted (usually the meeting date)." }
      },
      "description": "An extracted action / outstanding item linked to its source note."
    },
    "GlossaryTerm": {
      "type": "object",
      "additionalProperties": false,
      "required": ["term", "definition", "aliases", "project"],
      "properties": {
        "term": { "type": "string", "description": "The term or phrase." },
        "definition": { "type": "string", "description": "Definition or expansion." },
        "aliases": { "type": "array", "items": { "type": "string" }, "description": "Alternative forms that map to this term." },
        "project": { "$ref": "#/$defs/ProjectSlug", "description": "Project this term is scoped to." }
      },
      "description": "A project-scoped glossary term."
    },
    "Project": {
      "type": "object",
      "additionalProperties": false,
      "required": ["id", "slug", "display_name", "parent", "note_count", "meeting_count"],
      "properties": {
        "id": { "type": "string", "pattern": "^p_[0-9a-z]{6,}$", "description": "Stable project id (informational). The slug is the handle tools accept." },
        "slug": { "$ref": "#/$defs/ProjectSlug" },
        "display_name": { "type": "string", "description": "Human-facing project name (the leaf, e.g. \"Q3\" or \"Briarwood Golf\")." },
        "parent": { "oneOf": [ { "$ref": "#/$defs/ProjectSlug" }, { "type": "null" } ], "description": "Parent project slug, or null for a top-level project." },
        "note_count": { "type": "integer", "minimum": 0, "description": "Notes directly in this project (not descendants)." },
        "meeting_count": { "type": "integer", "minimum": 0, "description": "Meeting-type notes directly in this project." },
        "last_activity": { "oneOf": [ { "$ref": "#/$defs/IsoDateTime" }, { "type": "null" } ], "description": "UTC timestamp of the most recent note activity, or null if empty." }
      },
      "description": "A routing-target project (maps to a folder), with hierarchy and counts."
    }
  }
}
```

---

## Cross-cutting contract

1. **Errors.** Malformed calls (schema-invalid params, unknown tool) → JSON-RPC protocol errors.
   Business errors the model should see and reason about — note id not found; id is not a meeting;
   target project missing with `create_project: false`; `on_conflict: "error"` hit — return a normal
   `CallToolResult` with **`isError: true`** and a human-readable `content` text block;
   `structuredContent` is omitted on error, so each `outputSchema` above stays the clean success
   shape.
2. **Not-found vs. empty.** A lookup by a specific id/slug that misses (`get_note`,
   `get_meeting_transcript`, `get_project_context`) → `isError: true` (the caller asserted a thing
   exists). A search/list that matches nothing (`search_notes`, `list_outstanding_items`,
   `list_projects`) → success with an empty array and `has_more: false` (absence is a valid answer).
3. **Writes & confirmation.** The server performs no confirmation of its own; approval is delegated
   entirely to **Claude Code's permission model**, which prompts per tool call. The
   `readOnlyHint`/`destructiveHint` annotations let the client render `file_note_to_project` and
   `add_glossary_term` appropriately. Neither is marked destructive because both are reversible with
   no data loss.
4. **Pagination.** Uniform **cursor-based** pagination (`limit` + opaque `cursor` in, `PageInfo` out)
   on every list-returning tool. Cursors are chosen over offsets because the knowledge base mutates
   under a file watcher and re-routing; an offset would skip or duplicate rows across pages. A cursor
   names the boundary **row**, not its sort key, because a ranked surface's scores are not stable
   either: `search_notes` fuses by rank, so one note arriving above the cursor shifts every score
   below it. Resolving the boundary by id keeps the walk exact across that. The standard keyset
   trade still applies: a row inserted *above* a position the caller has already passed is not seen
   until the next walk. The sole
   exception is `get_project_context`, a deliberately bounded single-call briefing: its sections are
   hard-capped by their `*_limit` params and carry no cursor. Its `counts` report the true totals, so
   a caller that hits a cap falls back to the paginated tool for that slice (`list_outstanding_items`
   for outstanding items, `search_notes` for notes); standalone glossary pagination is deferred (see
   [Deferred / not in v1](#deferred--not-in-v1)).
5. **Dates & time.** ISO 8601 throughout: a note's `date` is passed through **verbatim as
   frontmatter stores it** — a full RFC 3339 timestamp with offset when a time is known,
   `YYYY-MM-DD` otherwise (per the frontmatter schema). That offset is the device's **local**
   offset at capture time (not UTC), so a note's wall-clock date reflects the user's local day;
   a consumer that orders across mixed offsets must normalize to UTC first (per the frontmatter
   schema's sort caveat). Due dates are `YYYY-MM-DD`; `last_activity` is an index-derived RFC 3339
   `date-time` in UTC. Transcript positions are **integer
   millisecond offsets** (`start_ms`/`end_ms`) from meeting start, not wall-clock, because
   per-channel segments are recorded relative to session start.
6. **Identity handles.** The **note write handle is the stable `id`**; `path` is always returned but
   never accepted as a handle, since it changes on move. The **project handle is the `slug`**
   (`"Growth/Q3"`), which mirrors the on-disk folder path and is human-usable; `Project.id` is
   exposed for stability, but no tool requires it.

---

## Deferred / not in v1

Out of scope for this spec, listed so Phase 3+ doesn't rediscover the gap from scratch:

- **`update_action_item`** (mark an item done) — belongs with the later commitment-ledger work, not
  the read/route/glossary surface defined here.
- **`resolve_project`** (fuzzy project-name → slug) — `list_projects` covers name resolution
  acceptably for v1; a dedicated fuzzy resolver is a nice-to-have if free-text matching proves
  unreliable in practice.
- **Glossary `list` / `remove` / `update`-beyond-add** — reads are already covered by
  `get_project_context`'s `glossary` section; standalone edit/delete tools are minor and deferred.
- **`delete_note`** (remove a note and everything derived from it) — now exists at the vault/Tauri
  layer (`vault::delete_note`, the `delete_note` command): it deletes the `.md` file, its index
  rows, and any paired session artifacts. An `mcp__kodabi__delete_note` tool would be the surface's
  first **destructive** write (`destructiveHint: true`), reusing the `NoteId` `$def` as its input
  handle; deferred to the Phase 3 server rather than added to this v1 spec.
- **MCP resources** (`@kodabi:note://...`) **and prompts** (`/mcp__kodabi__...`) — separate MCP
  surfaces from tools; not addressed by this ticket.

---

## Recommendation to P0-9

**Adopted** — [`FRONTMATTER_SCHEMA.md`](FRONTMATTER_SCHEMA.md) now carries the stable **`id`**
field this spec recommended: generated once at note creation (prefix `n_` + base36, e.g.
`n_a1b2c3`), **never rewritten on move or re-route**. This is the invariant the entire tool
surface above depends on as its write handle — every write tool (`file_note_to_project`) and
every id-addressed read (`get_note`, `get_meeting_transcript`) assumes it. Correspondingly,
projects need a stable `p_…` id and action items an `a_…` id at the index layer (used in
`Project.id` and `ActionItem.id` above), but those need not live in note frontmatter.

---

## Milestone walkthrough

The Phase 3 milestone — *"What's outstanding on Briarwood Golf?" answered correctly in-app from real
meeting history* — traces through this surface as:

1. `list_projects()` → resolve the free-text name "Briarwood Golf" to its slug (e.g. `"Briarwood Golf"`
   or a nested slug if it's a sub-project).
2. `list_outstanding_items(project: "Briarwood Golf")` or `get_project_context(project: "Briarwood Golf", include_outstanding: true)` → the not-done action items for that project, each carrying a `source` (`NoteRef`) back to its meeting.
3. `get_note(id: <source.id>)` → full body of the source meeting, if the answer needs to quote or
   explain an item in more detail than the `ActionItem.description` provides.

All three calls are read-only and already in the v1 surface — no gap remains for this flow.

---

## What this hands downstream

- **→ Phase 3 (stdio MCP server, `crates/kodabi-core`):** these 8 `tools/list` entries verbatim,
  the error/pagination contract, and the `.mcp.json` wiring. The server implements the schemas
  above as its `inputSchema`/`outputSchema` and returns `structuredContent` conforming to them.
- **→ Phase 2 (`search_notes` via hybrid retrieval):** the `search_notes` input filters and the
  `SearchHit`/`PageInfo` output shape that the FTS5 + `sqlite-vec` + RRF pipeline must produce.
- **→ P0-9 (frontmatter schema):** the `id`-field recommendation above (adopted); the
  `NoteSummary` fields (`id`, `type`, `title`, `project`, `date`, `tags`, `source`, `confidence`)
  mirror the frontmatter fields, so the two specs must stay in agreement as either evolves. (`path`
  is the one `NoteSummary` field with no frontmatter counterpart — it is the note's current
  location, not stored content.) The Phase 2 markdown
  writer (`kodabi-core::note`) now implements the frontmatter emitter/parser; building it surfaced
  edge cases (offset-required dates, tag grammar, project-segment folder-name constraints, the
  `source` keyword-vs-path rule, the `confidence`/re-route reconciliation, Inbox-folder placement)
  that were absorbed into [`FRONTMATTER_SCHEMA.md`](FRONTMATTER_SCHEMA.md) and mirrored into the
  `$defs` above in the same change.

---

*Spec, not implementation: the stdio MCP server is built in Phase 3; the `search_notes` retrieval
pipeline is built in Phase 2. This document fixes the tool names and schemas both build against.*
