# Kodabi — Session Filename Scheme

**Status:** Locked (Phase 1, `feat/session-filename-scheme`). Specifies how captured-session
artifacts are named on disk. Implements [`FOUNDING_DOC.md`](FOUNDING_DOC.md) §3.6b ("Filenames
include timestamp + device ID"); the Phase 1 raw session store
(`feat/persist-raw-session`), the Phase 2 markdown writer, the file watcher/indexer, and the
import-as-merge design must all produce and parse filenames that follow this scheme.

---

## Why

The knowledge base is a plain folder synced by whatever the user chooses — Syncthing, OneDrive,
Dropbox, a git repo. Two devices can capture a session at the same instant, and import is defined
as *merge, never overwrite* (§3.6b). For that to be safe, filenames alone must guarantee two
devices never write the same name. Composing a filename from **when** it was captured and
**which device** captured it makes a collision structurally impossible, with no coordination
between devices required.

---

## The scheme

```
{timestamp}-{deviceID}[-{slug}].{ext}
```

Example: `20260712T140335123Z-k4m2xp7q-paradise-golf-sync.jsonl` (53 characters)

| Part | Format | Notes |
| --- | --- | --- |
| `timestamp` | `YYYYMMDDThhmmssSSSZ` | UTC, millisecond precision, literal trailing `Z`. Contains no `-` or `:`. |
| `deviceID` | 8 lowercase base36 chars (`[0-9a-z]{8}`) | Stable per machine (see below). Contains no `-`. |
| `slug` | optional, `[a-z0-9-]+`, capped at 40 chars | Derived from a human-readable label (e.g. a meeting title); sanitized and truncated. |
| `ext` | e.g. `jsonl`, `md` | The artifact's natural extension. |

### Timestamp

UTC basic ISO 8601 with milliseconds: `YYYYMMDDThhmmssSSSZ`, e.g. `20260712T140335123Z`.

- **UTC, not local time** — matches `FRONTMATTER_SCHEMA.md`'s warning that cross-offset string
  sorts don't reflect chronological order; a single fixed offset (UTC) makes lexical sort order
  and chronological order the same thing.
- **No `:`** — colons are invalid in Windows filenames; basic (non-extended) ISO format omits
  them entirely.
- **Millisecond precision** — reduces the chance that one device produces two sessions with an
  identical timestamp (the device ID and optional slug still disambiguate even in that case).

### Device ID

An 8-character lowercase base36 string (e.g. `k4m2xp7q`), generated once per machine and stored
in that machine's **local app config** — not inside the synced knowledge-base folder. This is a
deliberate departure from a literal reading of "config that syncs with the folder": a value whose
entire purpose is to differ per device cannot itself be synced, or two machines would converge on
the same ID and reintroduce the collision this scheme exists to prevent. (Glossaries, project
config, and routing examples are the config that syncs with the folder, per §3.6b.)

The ID only needs to distinguish a single user's own handful of devices, not be globally unique —
36⁸ (≈2.8×10¹²) possibilities makes collision negligible at that scale, so a short ID was chosen
over a full UUID to keep filenames (and therefore full paths under deeply-nested synced folders)
short. See `crates/kodabi-core/src/device.rs` for generation and persistence.

### Slug

An optional, sanitized, lowercase-kebab label truncated to 40 characters, kept short so that
`{timestamp}-{deviceID}-{slug}.{ext}` stays well under both the NTFS 255-character per-component
limit and comfortably within Windows' legacy 260-character full-path limit, leaving headroom for
a deeply-nested synced folder path.

### Parsing

Because neither the timestamp nor the device ID ever contains `-`, splitting the filename stem
(after removing the extension) on `-` with a limit of 3 unambiguously recovers
`[timestamp, deviceID, slug]` — the slug, if present, may itself contain `-`. See
`parse_session_filename` in `crates/kodabi-core/src/naming.rs`.

### The recording sibling

A session's retained recording is a `.wav` (16-bit PCM stereo, 48 kHz, left = mic/you,
right = system/them) sharing the **exact** stem of its `.jsonl` transcript — numeric
disambiguator included — so the pairing is derivable from either filename with no index:

```
20260712T140335123Z-k4m2xp7q-paradise-golf-sync.jsonl   the transcript
20260712T140335123Z-k4m2xp7q-paradise-golf-sync.wav     its recording
```

The recording is written *after* the transcript's atomic link claims its final name, and its own
name is derived from the claimed path (`audio_sibling` in `crates/kodabi-core/src/naming.rs`),
never re-composed — which is what keeps the stems identical even when a same-millisecond
collision appended a numbered slug.

### The dismissed-marker sibling

A needs-attention session the user has dismissed carries a `.dismissed` marker under the same
exact-stem rule (`dismissed_sibling` in `crates/kodabi-core/src/naming.rs`):

```
20260712T140335123Z-k4m2xp7q-paradise-golf-sync.dismissed
```

Presence is the whole signal — the content (one RFC 3339 UTC line, the dismissal instant) is
never parsed. The marker is written and cleared by `crates/kodabi-core/src/sessions.rs`
(`dismiss_session` / `restore_session`); a successful distill clears it, and deleting a session
removes it with the transcript and recording.

---

## Consumers

Every component that names or reads a captured-session filename must follow this scheme:

- **`feat/persist-raw-session`** — writes the raw transcript file using this naming.
- **Phase 2 markdown writer** — the `source:` frontmatter field on `meeting`/`chat` notes
  (`FRONTMATTER_SCHEMA.md`) points at a raw artifact named per this scheme.
- **Phase 2 file watcher / indexer** — rebuilds the SQLite index from files on disk; relies on
  filenames sorting chronologically and never colliding across devices.
- **Import/export (merge, never overwrite)** — import relies on timestamp+device-ID filenames to
  detect that two files from different devices are distinct, never overwriting one with the other.
- **Retention** (`crates/kodabi-core/src/retention.rs`) — ages a `.jsonl` transcript, its `.wav`
  recording, and its `.dismissed` marker by the shared filename timestamp, so the trio expires
  together; the post-distill discard removes all three.

## Reference implementation

`crates/kodabi-core/src/device.rs` — `DeviceId` generation, validation, and per-machine
persistence (`load_or_create`).
`crates/kodabi-core/src/naming.rs` — `session_filename` (compose) and `parse_session_filename`
(decompose).
