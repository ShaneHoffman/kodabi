# UTC timestamps

Internal and derived timestamps are stored as UTC RFC 3339 with a `Z` suffix, so
lexical order equals chronological order and nothing depends on the machine's zone.

- **Stored/derived instants are UTC.** The index `date_utc` column is derived from
  the note's frontmatter `date` by `normalize_date_to_utc`
  (`crates/kodabi-core/src/index/note.rs`) precisely so a text sort is a time sort.
  Other writers follow suit: `vault.rs` writes `corrected_at` via
  `Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)`; capture and retention
  compute from `chrono::Utc::now()`.
- **Carve-out — the frontmatter `date` field is different, and correct as-is.**
  Per `docs/FRONTMATTER_SCHEMA.md`, a note's `date` is either an offset-preserving
  RFC 3339 timestamp **or** a local date-only `YYYY-MM-DD`. Quick capture
  legitimately writes a local calendar date
  (`chrono::Local::now().format("%Y-%m-%d")`, `src-tauri/src/quick_capture.rs`).
  Distill is the same carve-out in its offset-bearing form: `frontmatter_date_parts`
  (`crates/kodabi-core/src/distill.rs`) renders the UTC capture instant through
  `with_timezone(&Local)`, so the note's `date` keeps the exact instant but carries
  the device's local offset and files under the user's own day. The index preserves
  both verbatim in `date_raw` and derives `date_utc` only for ordering. **Do not
  "fix" either to UTC** — they are the sanctioned cases.
- **Never `DEFAULT CURRENT_TIMESTAMP` or `datetime('now')`** for a stored value.
  SQLite's default renders without a `Z` and is parsed as local time by consumers.
  Timestamps are written explicitly by Rust with a controlled format. (The v1 index
  DDL is frozen regardless — see [`.claude/agents/migration-safety.md`](../agents/migration-safety.md).)
- **Never `Local::now()` for a stored or compared instant.** The frontmatter `date`
  cases above are the only sanctioned local reads, and neither reads the clock in
  local time for an instant: quick capture takes a calendar date, distill converts an
  instant it already holds.
