# Kodabi — Frontmatter Schema

**Status:** Locked (Phase 0, ticket P0-9; amended by the Phase 0 readiness audit to absorb
P0-10's stable-`id` recommendation). Specifies the YAML frontmatter every note carries; the
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
base depends on Kodabi continuing to exist).

---

## Fields

Canonical key order the writer emits: **`id, type, category, tracking, title, project, date, tags, source, confidence, category_confidence`**.

| Field | Type | Required | Allowed values / format |
| --- | --- | --- | --- |
| `id` | string | yes | `n_` + base36 (`^n_[0-9a-z]{6,}$`), e.g. `n_a1b2c3`; generated once at creation, **never rewritten** on move or re-route |
| `type` | string enum | yes | `meeting` \| `note` \| `chat` (closed set) |
| `category` | string enum | no | The meeting's genre: `standup` \| `one-on-one` \| `client` \| `working-session` \| `review` \| `all-hands` \| `observer` (closed set). **Meetings only**; omit the key on any other type, and on a meeting nothing has classified |
| `tracking` | string enum | no | The per-meeting commitment-tracking override: `tracked` \| `context-only`. Omit the key to inherit (the category's default, then the global default of full tracking) |
| `title` | string | no | The human display title; free text, capped at 120 characters. Omit the key for a note without one — the display layer then de-slugs the filename |
| `project` | string | yes | A project name; the sentinel `Inbox` when unrouted |
| `date` | string (ISO 8601) | yes | Timestamp with offset when a time is known (`2026-07-09T14:00:00-07:00`); date-only (`2026-07-11`) otherwise |
| `tags` | list of strings | no | Lowercase kebab-case, no leading `#`; omit the key entirely when there are no tags |
| `source` | string | yes | A keyword — `transcript` \| `quick-capture` \| `chat` \| `import` \| `manual` — **or** a repo-relative path to the raw artifact when one exists |
| `confidence` | float | conditional | `0.0`–`1.0`, the routing score |
| `category_confidence` | float | conditional | `0.0`–`1.0`, how strongly the classifier backed `category`. Requires `category`; **removed** when a person sets the category by hand |

**Notes on individual fields:**

- **`id`** — the note's **stable identity and the MCP write handle** (absorbed from
  [`MCP_TOOL_SURFACE.md`](MCP_TOOL_SURFACE.md), "Recommendation to P0-9"). `file_note_to_project`,
  `get_note`, and `get_meeting_transcript` all address notes by this value, so it survives every
  move and re-route unchanged — the file path is informational and changes on move; the `id` never
  does. Matches the `NoteId` schema (`^n_[0-9a-z]{6,}$`) in the MCP tool surface. The Phase 2 writer
  generates `n_` + **8** random base36 characters (the regex permits any length ≥ 6; 8 is chosen so
  an id collision stays negligible even when import/merge pools several devices' notes into one
  vault, since — unlike a filename clash — an id collision is unrecoverable).
- **`title`** — the note's human display label, shown in every list and the note editor. It is
  stored so it survives past the filename slug: the filename `{slug}.md` caps the title at 40
  characters (filesystem sanity), but the frontmatter `title` keeps the full string (capped at 120,
  whitespace collapsed to a single line). **Optional and derived-fallback:** a note without the key
  — a hand-written note, or any note created before this field existed — has its display title
  de-slugged from the filename stem (`weekly-sync` → `weekly sync`) exactly as before, so nothing
  regresses. The writers that have a real title (distill, the create-note command, and quick
  capture, whose title is its first body line) emit the key. **The title is editable afterward:**
  the note editor's `save_note` rewrites the value in place, adding the key to a note that never
  had one. The *filename* is what is set once at creation — it keeps its original slug however the
  title changes, because renaming the file would break the note↔source pairing (a distilled note
  finds its recording and transcript by filename stem) and every link pointing at the old path.
- **`project`** — `Inbox` is not a real project; it is the sentinel value confidence-split routing
  uses when a note's score is too low to auto-file. The Inbox UI's one-click re-route corrects
  `project` and re-scores `confidence` for the chosen project, in place. Because a project maps to
  an on-disk folder (segments split on `/`, so `Growth/Q3` nests), each segment must be a legal
  folder name beyond matching the slug pattern: no `:*?"<>|`, no trailing dot or space, not a
  Windows reserved device name (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`), and no
  leading `.` or `_` — those prefixes mark infra folders (`.obsidian`, `_assets`, the
  `_glossary.yml` home) that routing discovery skips, so such a project would be writable yet
  invisible to routing. `Inbox` (any casing) is a reserved folder name — a real project may not be
  named `Inbox` — and `sessions`, `raw`, and `chats` (any casing) are reserved as first segments:
  `<vault>/sessions/` holds raw session artifacts and `<vault>/chats/` holds raw chat transcripts,
  never notes (`raw` stays reserved alongside them; a nested segment like `Data/raw` is fine).
- **`date`** — full timestamp+offset for anything with a real start time (a meeting, a chat
  session); date-only is acceptable for a quick-capture note jotted with no meaningful clock time.
  Store the value exactly as written. The two accepted shapes are strictly a `YYYY-MM-DD` calendar
  date **or** an RFC 3339 timestamp that **carries an offset** (`Z` or numeric); a naive
  `2026-07-09T14:00:00` with no offset is rejected, because without an offset the instant is
  ambiguous. **The canonical `date` shape is the device's local offset at capture time** (e.g.
  `2026-07-09T20:00:00-04:00`, not the `…Z` UTC form): the offset preserves the exact instant, but
  the wall-clock digits reflect the user's *local* day, so an evening meeting near the UTC day
  boundary files under the correct local date rather than "tomorrow." A date-only value is likewise
  the local calendar date. A lexical string sort orders same-offset timestamps and date-only values
  chronologically, but it compares wall-clock digits rather than the underlying instant — so
  `2026-07-09T14:00:00-07:00` (21:00Z) sorts *before* `2026-07-09T15:00:00+00:00` (15:00Z) even
  though it happened later. When notes span multiple offsets, the index must normalize `date` to UTC
  before ordering; do not rely on raw string comparison across offsets.
- **`tags`** — a plain YAML list, which Obsidian reads natively as note tags. Each tag matches
  `^[a-z0-9]+(?:-[a-z0-9]+)*$` (lowercase kebab-case, no leading `#`). Emitted as an inline flow
  list (`tags: [budgeting, phase-2]`). Keep the key absent rather than an empty list when a note has
  none; a hand-edited `tags: []` is normalized to an omitted key on the next rewrite.
- **`source`** — identifies *how* a note came to exist. For `type: meeting` notes with a raw
  session artifact (the Phase 1 raw session store, `sessions/`) and for `type: chat` notes
  distilled from a saved conversation (the chat transcript store, `chats/`), `source` is
  a relative path to that artifact, giving direct traceback from the distilled note to its raw
  recording without adding a seventh field. Raw artifact filenames follow the timestamp+device-ID
  scheme in [`FILENAME_SCHEME.md`](FILENAME_SCHEME.md), so simultaneous capture on two devices
  never collides. When no raw artifact exists — a quick-capture note, an imported file, a
  hand-written note — `source` falls back to the closest keyword. Disambiguation rule: a value
  **exactly equal** to one of the five keywords (`transcript` | `quick-capture` | `chat` | `import`
  | `manual`) is that keyword; anything else is a repo-relative raw-artifact path (which may not be
  absolute). A path-valued `source` is best-effort traceback only: the retention policy may have
  since pruned the referenced raw artifact, so a reader must tolerate a `source` path that no longer
  resolves (the MCP `get_meeting_transcript` tool reports this as `transcript_available: false` — see
  [`MCP_TOOL_SURFACE.md`](MCP_TOOL_SURFACE.md)).
- **`confidence`** — present whenever a routing score backs the current `project` value: notes
  confidence-split routing auto-filed, **including** low-score notes that land in `Inbox` (the score
  is *why* it landed there), and notes the Inbox re-route re-scored into a project. The trigger is
  that routing produced the score, not who authored the note — a human-jotted quick-capture note is
  still auto-routed, so it carries `confidence`. It is **omitted** entirely only when a human chose
  the `project` directly with no routing involved — a note filed *at creation* by hand, or an
  import — since no routing score exists to report. This does **not** conflict with
  `file_note_to_project` recording a manual re-route as `1.0`
  ([`MCP_TOOL_SURFACE.md`](MCP_TOOL_SURFACE.md)): a re-route is a routing action, so the corrected
  note *gains* a `confidence` (of `1.0`). The two ends describe one rule — the key is present iff a
  routing score (auto or a `1.0` correction) backs `project`. Emitted as a YAML float that always
  carries a decimal point (`1.0`, never `1`); range `0.0`–`1.0`.

- **`category`** — the meeting's **genre**: a second facet beside `type`, answering "what kind of
  meeting was this" rather than "what kind of document is this". Closed set on purpose, so the
  classifier's answer is checkable and a corrected value means the same thing next week:
  `standup` (a short recurring status round), `one-on-one` (a private conversation between two
  people), `client` (a conversation with an external customer or partner), `working-session`
  (hands-on work done together), `review` (an assessment of finished work), `all-hands` (a large
  company-wide or department-wide address), `observer` (a meeting attended only to listen in).

  **Meetings only.** The key is invalid on a `note` or a `chat`, and editing a meeting into another
  type drops it. It is also omitted on a meeting nothing has classified — an old note, or one whose
  distill declined to pick.

  Written by the distill pass and correctable in one click from the note view or the Inbox; each
  correction is recorded as an example in the project's `_category.yml`, which feeds the classifier
  the next time a note routes there. Each kind also carries a **commitment-enrollment default**,
  editable in Settings, which fills the middle slot of the `tracking` chain below
  (`ledger::effective_mode`); `all-hands` and `observer` default to tracking direct asks only. So
  correcting a meeting's kind also re-evaluates what it contributes to the ledger, in both
  directions, sparing anything a person has already acted on.

- **`tracking`** — the per-meeting **commitment-tracking override**: `tracked` (every extracted item
  is enrolled in the ledger) or `context-only` (only items the local user owns are). Omit the key to
  **inherit** — the meeting category's default, then the global default of `tracked`.

  Absence means *inherit*, which since categories carry defaults is no longer the same as `tracked`.
  The UI's switch therefore **writes a value in both directions** rather than clearing the key:
  clearing it on an all-hands would hand the meeting straight back to the genre that was already
  gating it, so the switch would move and nothing would change. The explicit `tracked` value is what
  says "track this one, whatever my kind defaults to." There is deliberately no affordance for
  returning a meeting to inheriting; hand-deleting the key still does it.

  Called *per-meeting* because that is what the surface offers and what the word means to a reader,
  but unlike `category` it is **not restricted to `type: meeting`**: a chat carries action items and
  feeds the ledger the same way (`meeting::derives_facts`), so the same override has to be able to
  apply to one. Nothing writes it on a chat today.

  It lives here rather than in the ledger database because it is a judgement about the *note*, so it
  travels with the file: a re-route, a vault rebuild, or a sync to another machine all carry it for
  free. Extraction is unconditional either way — an untracked meeting still gets a full note with
  every action item in its body; only ledger enrolment is gated (see
  [`ARCHITECTURE.md`](ARCHITECTURE.md), "Extraction is not tracking").

- **`category_confidence`** — how strongly the distill pass backed its own `category` guess, on the
  same `0.0`–`1.0` scale and emitted the same way as `confidence` (always with a decimal point).
  Requires `category`: a score for a category that is not there describes nothing.

  **Removed when a person sets the category by hand**, which is the same rule `confidence` follows in
  reverse — a machine guess carries its uncertainty, a human correction is a fact and carries none.
  So the key's presence is exactly the answer to "did anyone confirm this genre".

---

## Examples

One example per `type`, plus a classified meeting, each exercising a different combination of
field states.

### `meeting` — auto-routed, high confidence, traceable to a raw recording

```markdown
---
id: n_a1b2c3
type: meeting
title: Briarwood Golf Q3 budget and irrigation contractor shortlist
project: Briarwood Golf
date: 2026-07-09T14:00:00-07:00
tags: [budgeting, phase-2]
source: sessions/20260709T210000000Z-k4m2xp7q-briarwood-golf-sync.jsonl
confidence: 0.94
---

# Summary

Reviewed Q3 budget allocation for the course renovation and agreed on the irrigation
contractor shortlist.

## Decisions

- Approved the revised irrigation budget of $42,000.
- Selected GreenFlow Systems as the lead contractor for bidding.

## Action items

- [ ] Jane to send the signed budget memo to finance by 2026-07-11.
- [ ] Priya to request formal bids from GreenFlow and two alternates.
```

### `meeting` — classified, and attended for context only

```markdown
---
id: n_j1k2l3
type: meeting
category: all-hands
tracking: context-only
title: Q3 company all hands
project: Briarwood Golf
date: 2026-08-19T10:00:00-07:00
tags: [company]
source: sessions/20260819T170000000Z-k4m2xp7q-q3-all-hands.jsonl
confidence: 0.71
category_confidence: 0.88
---

# Summary

Leadership walked through Q3 results and the Q4 hiring plan.

## Action items

- [ ] Priya to circulate the revised hiring plan.
- [ ] You to send the course renovation numbers to Priya by 2026-08-22.
```

Both new facets are visible here, and they answer different questions. `category: all-hands` is the
distill pass's classification, with `category_confidence: 0.88` recording how sure it was — the
score would be gone had a person set the genre by hand. `tracking: context-only` is a judgement a
person made: this was a meeting attended to listen, so only the direct ask (`You to send the course
renovation numbers`) is enrolled in the commitment ledger. Priya's line stays in the note exactly as
extracted; extraction is unconditional, and only tracking is gated.

### `note` — quick-capture, low confidence, unrouted to Inbox

```markdown
---
id: n_d4e5f6
type: note
title: Check if the irrigation contractor also handles clubhouse drainage
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
id: n_g7h8i9
type: chat
title: Chat: irrigation contractor comparison
project: Briarwood Golf
date: 2026-07-10T09:15:00-07:00
tags: [research]
source: chats/20260710T161500000Z-k4m2xp7q.jsonl
---

# Chat: irrigation contractor comparison

Asked Claude Code to compare GreenFlow Systems against the two alternate bidders using
prior meeting notes filed under this project; kept the comparison table as a reference
note.
```

Note the `chat` example has no `confidence` key — it was manually kept into the project rather
than auto-routed, so there is no routing score to record. A chat note that came straight out of
the distill pass (`kodabi-core`'s `chat_distill`) does carry one, exactly like a meeting: it is
routed by the same confidence split, and lands in `Inbox` with its score when uncertain.

Its **body** matches a meeting's too. The chat pass shares the meeting pass's renderer, so a
distilled chat carries the same `# Summary` / `## Decisions` / `## Action items` / `## Open
questions` scaffolding (each section omitted when empty), and its decisions and action items are
parsed back out into the index like a meeting's — so a commitment made in a chat reaches
`list_outstanding_items` and `get_note`'s `action_items`. A hand-filed note (`type: note`) is not
parsed this way: its body is stored verbatim, so a checkbox in it is prose, not a tracked item.

The `source` path is the chat transcript under `chats/`, written by the chat view one JSONL record
per turn. Unlike the `sessions/` scheme it carries no title slug — a chat is named only by when it
started and which device it started on.

---

## Closure annotations under an action item

A line may sit directly beneath an action item, recording how the commitment was resolved:

```markdown
- [ ] Jane to send the signed budget memo to finance by 2026-07-11.
  - Closed 2026-07-09: memo acknowledged in the finance thread (evidence in n_a1b2c3).
```

The shape is fixed: two spaces, then `- Closed <YYYY-MM-DD>: `, then a sentence. The writer is
`vault::annotate_action_item`, the seam the commitment ledger's evidence providers call when they
close an entry. Two providers reach it today: a human confirming a parked claim
(`confirm_commitment_evidence`), and the distill pass, when a later conversation reports a
commitment already done confidently enough to close it without asking
(`distill_follow_up::apply_after_distill`, whose confidence floor is the user's
`ledger.conversation_autoclose` setting). The comparison is strict, so a claim *at or below* that
floor parks the entry in `needs_review` with the evidence attached and writes nothing to the note.

The prefix is chosen to be **inert to the action-item grammar** by construction: the parser trims
each line and then skips anything that is not `- [ ] ` or `- [x] `, so a body carrying any number of
these re-derives byte-identical action items, ids included. That inertness is what makes the whole
approach safe, and it is why an annotation is written alongside every automatic tick rather than
instead of the story: the human-readable account of a commitment stays in the Markdown rather than
living only in a database, so a box the app ticked says on the next line who reported it done and
where. The two writes are separate calls and the annotation is allowed to fail on its own, so a
ticked box without one is possible; what is never possible is an annotation that silently rewrote
the item. Ticking is reserved for evidence that cleared the confidence bar or that a human
confirmed; everything less certain waits in `needs_review` and leaves the box alone. The one thing that would break it is a line that *does* start with a checkbox,
which would mint a phantom item and shift the occurrence counter behind every duplicate line after
it.

---

## On-disk placement, filenames & serialization

The Phase 2 markdown writer (`kodabi-core::note`) is the first implementation of this schema; these
are the placement and byte-level rules it establishes.

- **Folder.** A note lives at `<vault>/<project>/<slug>.md`. A hierarchical `project` nests folders
  (`Growth/Q3` → `<vault>/Growth/Q3/`), creating any missing parents; an `Inbox` note lives in
  `<vault>/Inbox/`. The vault root is the KB root (`sessions/…` and `chats/…` in `source` are
  relative to it).
- **Filename.** `{slug}.md`, where `slug` comes from the note's title under the same slug rules the
  session scheme uses (lowercase, non-alphanumeric runs → `-`, 40-char cap). This is the
  distilled-note filename and is **distinct** from the timestamp+device *raw/session* scheme in
  [`FILENAME_SCHEME.md`](FILENAME_SCHEME.md). The 40-char cap is a filesystem concern only — the
  full title is kept in the `title` frontmatter field, so a long title is not lost when its slug is
  truncated. When the title slugifies to empty (blank, emoji-only, punctuation-only), the filename
  falls back to `{id}.md`. A name clash gets an increasing numeric suffix (`weekly-sync-2.md`) and
  never overwrites. The path is informational and changes on move; the `id` never does.
- **Serialization contract.** Opening `---` fence line, then the present keys in the canonical order
  above (the optional and conditional keys — `category`, `tracking`, `title`, `tags`, `confidence`,
  `category_confidence` — are emitted only when present), closing `---` fence line, then one blank separator line, the body, and a single trailing newline.
  An empty body ends the file at the closing `---`. The body is stored trimmed of surrounding blank
  lines. Only the **first** `---`-on-its-own-line after the opening fence closes the frontmatter, so
  a `---` horizontal rule inside the body is preserved verbatim. Scalar values that would otherwise
  re-resolve as a non-string (a project literally named `true`, `null`, or `123`) are quoted so they
  round-trip as strings. Unknown frontmatter keys are tolerated on read but **not** preserved on
  rewrite — round-trip fidelity is guaranteed for the canonical keys above.

---

## What this hands downstream

- **→ Phase 2 markdown writer:** emits this frontmatter, in the canonical key order above, for
  every note it produces — end-of-meeting notes, quick-capture notes, and (later) distilled chat
  sessions. **Implemented** in `kodabi-core::note` (struct → md → struct round-trip), wrapped by the
  thin `write_note` Tauri command.
- **→ Phase 2 file watcher & full rebuild command:** parses `id`, `type`, `category`, `tracking`,
  `title`, `project`, `date`, `tags`, `confidence`, and `category_confidence` out of frontmatter to
  populate the SQLite FTS5 + `sqlite-vec` index
  (`date` is indexed so results can be ordered and range-filtered by recency without re-reading files;
  `title` is indexed for full-text search and falls back to the de-slugged filename when absent);
  because the files are the source of truth, a full rebuild can always reconstruct the index from them
  alone.
- **→ Phase 3 MCP tools:** `search_notes`, `file_note_to_project`, `list_projects`, and
  `get_project_context` all route and filter over these fields (ticket P0-10, the MCP tool
  surface, is explicitly informed by this schema).
- **→ Phase 5 commitment ledger:** reads `tracking` (through the index row) to gate which extracted
  items earn a ledger entry — the note file, not a database table, is where that judgement lives.
  `category` is the level the enrollment default attaches at, one step beneath that override, and
  now has a behavior consumer: the meeting's genre resolves to an enrollment default — a per-genre
  Settings value, falling back to the built-in when the user has set none, which is the shipping
  state (`kodabi_core::ledger::category_default_for`, read from the same index row) — and a
  `tracking` value overrides it whenever the note carries one.

---

*Locked, not final in every detail: field names and the `type` enum are fixed by this ticket. The
Phase 2 markdown writer (`kodabi-core::note`) is the first real consumer; building it surfaced the
edge cases now absorbed above — the strict date shapes (offset required), the tag grammar, the
project-segment folder-name constraints, the `source` keyword-vs-path rule, the `confidence`/re-route
reconciliation, and the placement/filename/serialization section. The mirrored MCP shapes in
[`MCP_TOOL_SURFACE.md`](MCP_TOOL_SURFACE.md) were updated in the same change (spec agreement).*
