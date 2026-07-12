# Kodama — Frontmatter Schema

**Status:** Locked (Phase 0, ticket P0-9). Specifies the YAML frontmatter every note carries; the
Phase 2 markdown writer emits it, the Phase 2 file watcher and full rebuild command index it into
SQLite, and the Phase 3 MCP tools (`search_notes`, `file_note_to_project`, `list_projects`,
`get_project_context`) route and query over it.

Per storage (§3.6), plain Markdown + YAML frontmatter **is** the source of truth — the SQLite
index (FTS5 + `sqlite-vec`) is a cache, rebuildable from these files at any time.

---

## Why plain Markdown + YAML

> The files are the truth; the database is a cache.

Choosing files over a database buys three things directly: **Obsidian compatibility** (the vault
opens and renders correctly in a plain folder of `.md` files, no plugin required), **git backups**
(notes diff and version like any other text), and **zero lock-in** (nothing about the knowledge
base depends on Kodama continuing to exist).

---

## Fields

Canonical key order the writer emits: **`type, project, date, tags, source, confidence`**.

| Field | Type | Required | Allowed values / format |
| --- | --- | --- | --- |
| `type` | string enum | yes | `meeting` \| `note` \| `chat` (closed set) |
| `project` | string | yes | A project name; the sentinel `Inbox` when unrouted |
| `date` | string (ISO 8601) | yes | Timestamp with offset when a time is known (`2026-07-09T14:00:00-07:00`); date-only (`2026-07-11`) otherwise |
| `tags` | list of strings | no | Lowercase kebab-case, no leading `#`; omit the key entirely when there are no tags |
| `source` | string | yes | A keyword — `transcript` \| `quick-capture` \| `chat` \| `import` \| `manual` — **or** a repo-relative path to the raw artifact when one exists |
| `confidence` | float | conditional | `0.0`–`1.0`, the routing score |

**Notes on individual fields:**

- **`project`** — `Inbox` is not a real project; it is the sentinel value confidence-split routing
  uses when a note's score is too low to auto-file. The Inbox UI's one-click re-route corrects
  `project` and re-scores `confidence` for the chosen project, in place.
- **`date`** — full timestamp+offset for anything with a real start time (a meeting, a chat
  session); date-only is acceptable for a quick-capture note jotted with no meaningful clock time.
  Store the value exactly as written. A lexical string sort orders same-offset timestamps and
  date-only values chronologically, but it compares wall-clock digits rather than the underlying
  instant — so `2026-07-09T14:00:00-07:00` (21:00Z) sorts *before* `2026-07-09T15:00:00+00:00`
  (15:00Z) even though it happened later. When notes span multiple offsets, the index must
  normalize `date` to UTC before ordering; do not rely on raw string comparison across offsets.
- **`tags`** — a plain YAML list, which Obsidian reads natively as note tags. Keep the key absent
  rather than an empty list when a note has none.
- **`source`** — identifies *how* a note came to exist. For `type: meeting` and `type: chat` notes
  that have a corresponding raw session artifact (per the Phase 1 raw session store), `source` is
  a relative path to that artifact, giving direct traceback from the distilled note to its raw
  recording without adding a seventh field. When no raw artifact exists — a quick-capture note, an
  imported file, a hand-written note — `source` falls back to the closest keyword.
- **`confidence`** — present whenever a routing score backs the current `project` value: notes
  confidence-split routing auto-filed, **including** low-score notes that land in `Inbox` (the score
  is *why* it landed there), and notes the Inbox re-route re-scored into a project. The trigger is
  that routing produced the score, not who authored the note — a human-jotted quick-capture note is
  still auto-routed, so it carries `confidence`. It is **omitted** entirely only when a human chose
  the `project` directly with no routing involved — a note filed by hand, or an import — since no
  routing score exists to report.

---

## Examples

One example per `type`, each exercising a different combination of field states.

### `meeting` — auto-routed, high confidence, traceable to a raw recording

```markdown
---
type: meeting
project: Briarwood Golf
date: 2026-07-09T14:00:00-07:00
tags: [budgeting, phase-2]
source: raw/2026-07-09-briarwood-golf-sync.jsonl
confidence: 0.94
---

# Summary

Reviewed Q3 budget allocation for the course renovation and agreed on the irrigation
contractor shortlist.

## Decisions

- Approved the revised irrigation budget of $42,000.
- Selected GreenFlow Systems as the lead contractor for bidding.

## Action items

- [ ] Shane to send the signed budget memo to finance by 2026-07-11.
- [ ] Priya to request formal bids from GreenFlow and two alternates.
```

### `note` — quick-capture, low confidence, unrouted to Inbox

```markdown
---
type: note
project: Inbox
date: 2026-07-10
tags: [idea]
source: quick-capture
confidence: 0.38
---

# Note

Maybe worth checking if the course's irrigation contractor also handles the clubhouse
drainage — ask Priya next sync.
```

### `chat` — distilled Claude Code session, manually filed

```markdown
---
type: chat
project: Briarwood Golf
date: 2026-07-10T09:15:00-07:00
tags: [research]
source: raw/2026-07-10-irrigation-contractor-comparison.jsonl
---

# Chat: irrigation contractor comparison

Asked Claude Code to compare GreenFlow Systems against the two alternate bidders using
prior meeting notes filed under this project; kept the comparison table as a reference
note.
```

Note the `chat` example has no `confidence` key — it was manually kept into the project rather
than auto-routed, so there is no routing score to record.

---

## What this hands downstream

- **→ Phase 2 markdown writer:** emits this frontmatter, in the canonical key order above, for
  every note it produces — end-of-meeting notes, quick-capture notes, and (later) distilled chat
  sessions.
- **→ Phase 2 file watcher & full rebuild command:** parses `project`, `date`, `tags`, `type`, and
  `confidence` out of frontmatter to populate the SQLite FTS5 + `sqlite-vec` index (`date` is
  indexed so results can be ordered and range-filtered by recency without re-reading files); because the
  files are the source of truth, a full rebuild can always reconstruct the index from them alone.
- **→ Phase 3 MCP tools:** `search_notes`, `file_note_to_project`, `list_projects`, and
  `get_project_context` all route and filter over these fields (ticket P0-10, the MCP tool
  surface, is explicitly informed by this schema).

---

*Locked, not final in every detail: field names and the `type` enum are fixed by this ticket; the
Phase 2 markdown writer is the first real consumer and may surface edge cases this document should
absorb before Phase 3 depends on them further.*
