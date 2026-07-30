---
name: add-migration
description: Append a schema migration to the note index safely — append-only, tested, and doc-synced. Use when the SQLite index schema in kodabi-core needs to change.
argument-hint: [what the schema change is and why]
---

# Add a note-index migration

Change the note index schema in `crates/kodabi-core/src/index/migrations.rs` without
breaking databases in the field.

**Hard rules:** never edit or reorder an existing `migration_XXXX_*` entry — append a
new one. The v1 DDL is frozen (byte-stable, including `FLOAT[768]`).

What's changing (may be empty): $ARGUMENTS

## 1. Read the module doc first

Open `migrations.rs` and read its top comment: `user_version` tracking, the
append-only list, one transaction per migration, and the rebuildable-cache doctrine
(FOUNDING_DOC §3.6 — a breaking change may bump the version and recreate tables).

## 2. Decide the shape

Additive DDL (new table/column/index) or drop-and-recreate. Drop-and-recreate is
legal because the index is a rebuildable cache — but write down, in the migration's
doc comment, what a field database *loses* when it runs (the way
`migration_0002_chunked_embeddings` documents the 768→`EMBEDDING_DIM` recreation).

## 3. Append the builder

Add `migration_XXXX_<name>() -> String` and one new entry at the end of the
`migrations()` vec. Interpolate current-value constants (e.g. `EMBEDDING_DIM`) only
into current tables — never back into a frozen migration. Leave `apply()` untouched.

## 4. Align the Rust

Update the structs and the INSERT/SELECT column lists in `index/` (`note.rs`,
`mod.rs`, `query.rs`) so they match the new schema; update the `NoteType` handling
if the `type` CHECK changed.

## 5. Tests

- Add an upgrade test that reconstructs the **prior** version *with data* and asserts
  the upgrade (copy `a_v1_database_upgrades_to_v2_on_open`).
- Bump the expected version in the all-tables/version test.
- Keep the idempotency and dimension tests green.

Run `cargo test -p kodabi-core index` (no `dist/` needed for a `-p` run).

## 6. Gates

Run the full Rust gates (fmt + clippy + test, `dist/` present). Four CI jobs are
path-filtered to `crates/` and so cover `kodabi-core` too. Two of them add a
clippy leg to run here; the other two add no gate (`App (release + Parakeet)`
only release-builds, which is `/pull-request`'s job, and `UI (E2E, quick
capture)` is non-required and never gates a commit):

- `cargo clippy -p kodabi-embed --features bge --all-targets --locked -- -D warnings`
- `cargo clippy -p kodabi --features parakeet --all-targets --locked -- -D warnings`

## 7. Verify

Spawn the `migration-safety` agent; fix every FAIL.

## 8. Sync the docs

If the change touches a frontmatter-visible or `NoteSummary` field, run `/sync-docs`
(at minimum `node .claude/skills/frontmatter-validator/validate.mjs --check-schema`)
so `docs/FRONTMATTER_SCHEMA.md` and `docs/MCP_TOOL_SURFACE.md` stay mirrored.
