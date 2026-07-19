---
name: add-tauri-command
description: Add a Tauri command end-to-end — core function, thin wrapper, registration, typed TS caller — then audit layer parity. Use when the frontend needs a new backend capability.
argument-hint: [command name and what it should do]
---

# Add a Tauri command

Wire a new backend capability through every layer so it works at runtime and
typechecks. Follow [`.claude/rules/tauri-command-parity.md`](../../rules/tauri-command-parity.md)
— the five layers and the "touch them all in one change" rule.

**Hard rule: the command body stays thin.** Logic lives in a `kodabi-*` crate; the
`#[tauri::command]` wrapper only owns DTOs, resolves state/paths, calls core once,
and maps errors. If the wrapper grows a body, move the body to kodabi-core.

What to build (may be empty): $ARGUMENTS

## 1. Locate the homes

- The owning `crates/kodabi-*` crate for the logic.
- The `src-tauri/src/*_cmds.rs` module for the wrapper (or a new module + its `mod`
  line and `use` in `lib.rs`).
- The owning `src/` TS module for the caller (`useNotes.ts`, `useSettings.ts`, …).

Read a nearby command in each layer first to copy its shape.

## 2. Core function first

Implement the capability in the crate with its validation and **unit tests**. This
is where correctness is proven (core-vs-shell).

## 3. Thin wrapper

Add the `#[tauri::command]` in `*_cmds.rs`: serde DTOs, managed-state/path
resolution, one core call, `Result<T, String>` via `.map_err(|e| e.to_string())`.
Match the `note_cmds.rs` module-doc convention; add a doc comment naming the core
function it wraps.

## 4. Register

Add the command to the single `tauri::generate_handler![…]` list in
`src-tauri/src/lib.rs`. Missing this compiles fine and fails only when invoked.

## 5. Typed TS caller

In the owning `src/` module, export a typed function wrapping
`invoke<T>("command_name", { … })` from `@tauri-apps/api/core`. The invoke string
**must equal the Rust function name exactly**. Add a wire-shape `type` whose doc
comment cites the Rust DTO. If the command emits an event, add the matching
listener.

## 6. Run the gates

Run the gates for the surface you touched (see the `/commit` matrix): Rust ⇒ fmt +
clippy + test (with `dist/` present); frontend ⇒ `pnpm exec eslint . --max-warnings=0`
+ `pnpm test` + `pnpm build`.

## 7. Verify parity

Spawn the `tauri-command-auditor` agent on the change. Fix every **blocker** it
reports (unregistered command, caller/command mismatch, fat wrapper) before
reporting done.
