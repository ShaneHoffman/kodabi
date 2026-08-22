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

`kodabi-mcp` exposes the knowledge base as an MCP server over stdio — ten tools, seven read and
three write:

- **Read:** `search_notes`, `get_note`, `get_meeting_transcript`, `list_outstanding_items`,
  `list_commitments`, `list_projects`, `get_project_context`
- **Write:** `file_note_to_project`, `add_glossary_term`, `update_action_item`

Their schemas are specified in [`MCP_TOOL_SURFACE.md`](MCP_TOOL_SURFACE.md) and mirrored verbatim as
committed JSON under `crates/kodabi-mcp/schemas/`. The read tools are pre-approved for the CLI; the
write tools always prompt.

The last two of each are the commitment ledger's surface, and the reason they are separate tools
rather than fields on the existing ones: `list_outstanding_items` answers *what did they commit to*
from raw extraction, while `list_commitments` answers *what am I tracking* from the ledger, through
the same core join the Commitments view renders — so chat and the app cannot disagree about what is
outstanding. `update_action_item` writes both halves of a mark-done: the checkbox in the note (the
source of truth for done) and the closure in the ledger. Those three tools need `KODABI_LEDGER_DB`,
the one optional path in the sidecar's wiring; the ledger lives in the config dir, so neither of the
other two env vars locates it.

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
items, decisions, open questions and (when the pass was shown any) its verdicts on existing
commitments as one structured result. A transcript over the input character
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
`_glossary.yml`, `_routing_examples.yml`, `_category.yml` and `_ledger.yml`, which is what keeps
them per-project isolated and makes them sync with the knowledge. None of the four names its own
project — the folder it sits in is the project — which is what lets a project rename move the folder
and carry them along untouched.

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
gathered outside the vault, and the states a checkbox has no spelling for (snoozed, waived,
untracked, closed *with provenance*, superseded by a later mention). The note's checkbox remains the sole truth for
done/not-done — the ledger stores no `done` column and a `- [x]` flip is invisible to it.

That durability is why it is a separate file. The index may be nuked and rebuilt at will; these are
judgements a person made that exist nowhere else, so the ledger has its own append-only migration
set whose doctrine forbids drop-and-recreate. Its backup is the vault: after each change, the
affected project's entries are mirrored to `_ledger.yml`, and a missing or empty database is
rebuilt from those snapshots at startup — a non-empty one always wins, since merging two divergent
histories is not something to guess at. Extracted items are referenced by their content-hashed `a_`
ids, which are re-minted whenever a line's text is edited, so entries carry their own durable ids
and re-link across those edits (`kodabi_core::ledger::sync`).

**Extraction is not tracking.** The distill pass always records what was said, and the note, the
index and the MCP read surface look the same either way; what the ledger holds is the separate
question of what you are *tracking*. The gate sits at enrollment, in reconcile's create leg
(`kodabi_core::ledger::sync`), and has a per-item step ahead of a per-meeting chain: an item's
**firmness**, then a per-meeting override, then the meeting category's default, then the global
default of `tracked` (`ledger::effective_mode`). The one non-default mode is **context only**, which
enrolls just the items you own, because a direct ask is a commitment regardless of why you attended.

**Firmness is the per-item half, and it comes first.** The distill response classifies each action
item `firm` or `soft` ("I'll send the budget Friday" against "we should probably look into that
sometime"), and a soft item is declined before the meeting's mode is consulted at all: no
meeting-level setting can turn a musing into a commitment. A soft item still extracts, still renders
into the note, and still reaches the index and the MCP read surface like any other; it simply earns
no ledger entry. It is declined rather than enrolled-then-untracked on purpose, because the Settled
shelf is a record of judgements somebody made and would stop being readable as one if every idle
remark landed there.

Firmness travels in the note's own line, as a ` (tentative)` tail after the terminal period, because
enrollment re-derives action items from the Markdown on every index pass — a signal held only in
memory would be re-enrolled by the watcher seconds later. The marker is stripped before the rest of
the grammar is parsed and before the item id is hashed, so a firm line renders exactly as it always
did and no existing id re-mints. That also makes the marker an ordinary edit: deleting it by hand
promotes the item on the next reconcile, and the note rail's Track button
(`Ledger::track_item`) promotes it without touching the file. Every unreadable case defaults to
firm, so a model that ignores the field keeps the previous behaviour: enrolling something tentative
is a recoverable annoyance, and silently dropping a real commitment is not.

The per-meeting override is stored in the note's **frontmatter** (`tracking:`), not in a ledger
table: it is a judgement about the meeting, so it belongs with the meeting, and it then survives a
re-route, a vault rebuild and a sync to another machine without any bookkeeping to keep a row's
project in step. The category default is a user setting, one per genre
(`settings::CategorySettings`), and an unset one inherits a built-in
(`ledger::builtin_category_default`): `all-hands` and `observer` track direct asks only, the other
five track in full. An unset preference therefore means *inherit that built-in*, never "tracked" —
which is what lets the defaults reach the settings files already in the field, none of which carry
the key. Changing a genre's default is deliberately **prospective**: it decides what the next sync
does, and does not sweep the entries existing meetings already produced.

An un-enrolled item gets no entry and no ref at all, so the aging and evidence passes never see it —
that absence is the point, not an optimization. Both halves of the chain reach the gate on the note's
facts (`NoteSync.note_override` from the index row that mirrors the file, `NoteSync.category_default`
resolved by the shell, which is the only layer that can read the settings), and the mode is resolved
once inside the sync transaction, so both ingest paths (the index worker and the distill follow-up)
are gated identically; neither field is defaultable, so a producer that forgets one does not compile.
Because sync is idempotent, returning a meeting to tracked enrols what was skipped simply by
re-syncing it.

**Commitments do not only arrive from meetings.** Every note type derives action items
(`meeting::derives_facts`), by one of two grammars chosen from the type. A machine-rendered body (a
meeting, a chat) is read with the distill grammar: sections, and `Owner to do X by DATE.` split into
its fields. A hand-written body (`type: note` — quick capture, or anything edited by hand) is read
with the plain-checkbox grammar: any `- [ ]` line anywhere in the body, whole, owned by the user.
The split exists because the two bodies make different promises — quick capture writes the user's
text verbatim and has no headers at all, and applying the distill grammar's `" to "` split to free
text turns `- [ ] Send the deck to Priya.` into an item owned by "Send the deck". The plain grammar
infers nothing: no owner, no due date, just the line. Its owner is the fixed token `You`, which
`Direction::resolve` maps to *mine* ahead of any alias, so it survives the user renaming themselves
— it is hashed into the item's id, and an id that moved would orphan its entry.

Both grammars feed the same enrollment gate, so a plain note honours `tracking:` in its frontmatter
exactly as a meeting does. One rule is enforced at the create leg for all of them: **an item already
ticked the first time the ledger sees it mints nothing.** A checkbox flip on an item already tracked
still changes nothing (it lands on the present tier), and a later note restating a commitment as
done is still a mention of a live one — what is declined is only finished business the ledger never
knew about, which is most of what a historical note holds.

**The first run over an existing vault is seeded deliberately.** Every other route into the ledger
fires when a note *changes*, so a vault full of open commitments and an empty `ledger.db` would stay
that way: the startup reconcile skips unchanged notes, and the meeting-facts backfill finds nothing
missing. When the ledger is still empty after the snapshot restore, the index worker therefore seeds
it from what the index already holds — open items only, and only from notes dated inside the
enrolment window (`ledger::mention_window_cutoff`, the stale threshold). The window is the whole
design: without it day one is a wall of months-old items that arrive already stale and sort above
everything current. `last_mention` comes from each note's own date, so the aging tiers place a
backfilled commitment where its note sits in time rather than treating it as minted today.

The gate applies to entry *creation* only. A re-mention still links and still bumps `last_mention`,
so a live commitment restated in a context-only meeting does not age out for having been discussed
in the wrong room, and an edit still relinks its existing entry. Changing a meeting's effective mode
re-evaluates the entries it already produced in both directions
(`kodabi_core::ledger::enrollment`) — flipping its own switch does, and so does **recategorizing
it**, which is the only thing a category change sweeps. Both run at the command layer, awaited before
the re-index that follows: the retro un-enrols the strays, and that re-index's ordinary create leg,
now carrying the new default, enrols what the old mode kept out. A mode is a *default*, though, and a
default never overrules a person: any entry someone has acted on (`touched`, set only on the shell's
human-judgement path) or promoted by hand is left exactly as it is. **Untracked** is its own state
and its own verb, distinct from waived: waiving says this was mine and stopped mattering, untracking
says it was never my business. Its item refs stay active, so a re-sync sees the line as present and
can neither mint a duplicate nor park the entry as vanished. Every entry
carries `enrolled_via` (`default` / `override` / `manual` / `category`) and, when untracked,
`untracked_via` (`manual` / `override` / `category`), so any row can answer why it is in the ledger
and how it left. The sweeps match both *machine* provenances in each direction rather than only the
source that changed, which is what lets a recategorization reach entries minted long before under a
different genre; `manual` is out of reach either way.

One drift is accepted and named: recategorizing a note by hand in Obsidian re-syncs through the
watcher, so it enrols the missing, but no retro runs, so it leaves the strays tracked until that
meeting is touched in the app.

**Which commitments are yours is a resolved question, not a guessed one.** The action-item grammar
emits free-text owner names, so something has to decide that "Avery to send the deck" means *you*.
That is `settings::IdentitySettings` — a display name plus the other spellings meetings use for the
same person — resolved by `ledger::Direction::resolve` against an `OwnerIdentity`, the flattened,
normalized set of both. The grammar's own tokens win first and no alias can redefine them: `You` is
always yours, `Unassigned` is always neither. Matching is **normalization, not fuzzy matching** —
NFC, case, and whitespace, the same `owner_norm` folding the reconcile tiers already use, so a name
matched here and a name matched by `sync` can never disagree. No prefix matching and no edit
distance: a miss is cheap and self-correcting, while a false positive puts a promise in your mouth
that you never made.

An owner the identity does not resolve defaults to **theirs**, the same asymmetry the context-only
rule rests on: a stray sitting in Waiting-on-them is one click to fix, while a missed own commitment
is a real failure. That click is the correction loop the routing examples established — claiming a
row both moves it to Mine and *learns* the name it was filed under, so the next meeting gets it
right unprompted. Three spellings are refused rather than learned
(`ledger::learnable_alias`): `You` and `Unassigned` are the grammar's own tokens, already resolved
before any alias is consulted, and `Them` is the sharper case, because it is what the distill
guidance writes for an unnamed other, so adopting it would quietly claim every future them-side
commitment. A refusal is not a failure and the view does not report it as one. Claiming goes through the ordinary mutation path, which marks the entry `touched`,
which is exactly right: a person has now judged that row. That flag is also what protects the claim
from the reconcile tiers: an entry's `direction` is normally re-derived from the owner string every
time a reworded line relinks, and a claimed one is the case that string cannot express, so the
rewrite skips a `touched` entry already sitting in Mine.

Learning a name is retrospective, unlike a genre default. Saving one sweeps the entries already
recorded (`ledger::Ledger::retro_resolve_owners`), bound by the same rule as every other retro pass —
live states, `touched = 0` — and it only ever moves entries *toward* Mine. Removing a name re-files
nothing, because silently dropping a commitment out of Mine is the failure you would never see
coming. One gap is accepted and named: an item a context-only meeting gated out has no row for any
sweep to find, and enrols when its note next re-syncs.

The distill pass is told the same name, as an optional prompt block beside the commitments and
category ones. It does **not** change the owner spelling: the shared `RESPONSE_SHAPE_SPEC` still asks
for `"You"`, and the block exists so a first-person commitment on the mic channel is attributed to
the configured person rather than to whichever name the room happened to say. Emitting the name
instead would re-mint every item id against a spelling the existing ledger has never seen. Because a
misattributed owner in a context-only meeting produces no row at all, the identity block outranks
both others when the budget ladder has to drop one.

**Meetings have a genre, and it is correctable.** Beside the document `type` (meeting / note /
chat), a meeting note carries a `category`: `standup`, `one-on-one`, `client`, `working-session`,
`review`, `all-hands`, or `observer` (`kodabi_core::note::MeetingCategory`). A closed set, so the
classification is checkable and a corrected value means the same thing next week. The distill pass
picks one as part of its single JSON response (the shared `RESPONSE_SHAPE_SPEC`, so the merge call
on a long meeting returns one answer for the whole conversation rather than losing it per chunk),
and records how sure it was in `category_confidence`.

Corrections are first-class, and they teach. Recategorizing a note (`vault::set_note_category`,
behind the `set_note_category` command) rewrites the frontmatter, clears `category_confidence` —
a human correction is a fact, not an estimate — and appends the correction to `_category.yml` in
the note's own project folder, beside `_glossary.yml` and `_routing_examples.yml`. That file also
holds an optional `default:` prior. Both are rendered into the next distill prompt for a note
routing to that project, as *guidance* rather than as a rule: recurring meetings are the point, so
the same project plus a similar title should land on the genre the user picked last time, while the
transcript can always overrule the prior. This is deliberately weaker than the routing scorer's
arithmetic use of `_routing_examples.yml` — a genre is chosen by a model reading a conversation, so
its memory is prose the model reads too.

The genre's first consumer is the enrollment gate above: each kind carries a `tracked | context-only`
default, edited in Settings, filling `ledger::effective_mode`'s `category_default` slot. So a
correction is not only a better label, it changes what that meeting contributes to your commitments,
retroactively and in both directions. The category also rides the Commitments view's source line, as
a quiet faint segment, so a row can say what kind of room it came out of.

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

**A day's enrollments get a review moment, and it is deliberately not a gate.** The whole-vault view
opens with a strip naming what enrolled since the last time anyone looked, grouped by the meeting
that produced it, with Keep and Untrack on each row and a selection bar carrying the same verbs plus
Snooze for a whole group. Nothing waits for approval: every row in the strip is already tracked and
already counted in the queue below, and the strip vanishes the moment there is nothing new. That is
the point — a mandatory inbox that nobody clears goes stale and takes the credibility of the ledger
with it, so Keep records only that a person looked.

The marker behind it is device-local viewing state, so it lives in a `ledger_meta` row in `ledger.db`
and never in `_ledger.yml`: the snapshot is vault truth that travels between machines, and one
machine's reading position is not the other's. It is seeded once at startup behind the snapshot
restore, so a ledger that predates the feature does not greet its owner with its entire history, and
it advances only through the **contiguous reviewed prefix** of the batch rather than to the newest
row clicked. A single instant cannot describe a set with holes in it, so advancing past a row still
outstanding would hide it permanently; keeping a row out of order simply leaves it for next time.
The batch itself is frozen when the view mounts, because the refetch each of these writes triggers
would otherwise recompute the list against a marker the review had just moved. The strip is
whole-vault only for the same reason the marker is a single instant: reviewing inside one project
would mark every other project's new commitments seen.

Group and selection verbs route through batched commands (`untrack_commitments`,
`snooze_commitments`) and a single worker job, so a heavy day costs one transaction sweep, one
`ledger:changed` and one refetch rather than one of each per row. They are best-effort: the
transition table legitimately refuses some rows (a needs-review entry cannot be snoozed), and those
are reported beside the group rather than failing the gesture, because a sweep over rows the view
drew a moment ago will always race something. A bulk untrack is a person's own judgement like the
single verb, so it stamps `untracked_via: manual` and survives a later re-track.

Two things age. A row's **tier** (`fresh` / `aging` / `stale`) is derived at read time by
`ledger::view` from the later of `last_mention` and `last_evidence_check`, against a `today` the
shell supplies and the day thresholds the user set in Settings. Nothing writes when a tier changes,
for the same reason nothing writes when a snooze lapses: a stored tier would need a writer on the
day it turned, and the read model can simply answer the question instead. `updated_at` is
deliberately not part of it, because that is the sync's wall clock and re-indexing an old vault
would otherwise make every commitment in it look freshly discussed. Stale rows lead the undated
band; a row with a date already escalates on its own by becoming overdue.

The other ageing input is the distill pass, which is the ledger's first evidence **producer**.
Before the model call, the pass guesses the project from the raw transcript
(`routing::best_candidate` — the authoritative routing still runs afterwards on the rendered body)
and is shown that project's open commitments as a bounded JSON block. The guess has to clear the
same confidence threshold that decides auto-filing, so a transcript the router could not place
confidently is distilled with no block at all and the pass behaves exactly as it did before this
existed. It classifies any it hears
about as `refresh`, `supersede`, or `completed`, and `ledger::distill_apply` turns those into state:
a refresh advances the mention clock, a supersede links the old entry to the new one, and a
completion claim is recorded as `conversation` evidence that either closes the entry or parks it in
`needs_review` for a human, depending on the user's confidence threshold. A close is the one place
the app ticks a checkbox the user did not: the box belongs to whichever *earlier* note recorded the
promise, and it is written together with a `- Closed <date>: …` line naming the conversation that
reported it, so a box nobody remembers ticking can be traced back
(`docs/FRONTMATTER_SCHEMA.md`). Anything short of that threshold leaves every note untouched. A refresh naming one of
the new note's own lines is also the dedup mechanism — it is handed to the reconciler as a hint, so
a paraphrase (`"send the slide deck"` for `"send the deck"`) links to the existing commitment rather
than minting a second live entry for one promise. The classifications ride the shared
`RESPONSE_SHAPE_SPEC`, so meeting and chat distills both produce them; the map-reduced path for a
very long transcript deliberately does not, since an item index cannot survive a merge that rewrites
the list. The whole second half is best effort and runs after the note is on disk
(`distill_follow_up`): a distill never fails because the ledger was busy or unavailable.

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
