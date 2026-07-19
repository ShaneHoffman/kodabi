---
paths:
  - src-tauri/**
  - src/**
---

# Tauri command parity

A capability the frontend calls crosses several layers between React and a
`kodabi-*` crate. Miss one and it fails — at runtime for a missing registration,
at typecheck for a missing TS type, silently for a naming mismatch. Adding,
renaming, or removing a command touches **every applicable layer in the same
change**.

The layers, each with its real home:

1. **Core function** — in the owning `crates/kodabi-*` crate. The logic,
   validation, and unit tests live here (the core-vs-shell rule in `CLAUDE.md`:
   if a command grows a body, the body belongs in kodabi-core).
2. **Thin `#[tauri::command]` wrapper** — in `src-tauri/src/*_cmds.rs`. It owns
   only the serde IPC DTOs, resolves managed state / paths, calls one core
   function, and maps the result to a message string:
   `Result<T, String>` via `.map_err(|e| e.to_string())`. The `note_cmds.rs`
   module doc is the standard to match ("these commands only own the serde IPC
   DTOs … Errors collapse to a message string").
3. **Registration** — one entry in the single
   `tauri::generate_handler![…]` list in `src-tauri/src/lib.rs`. An unregistered
   command compiles fine and fails only when the frontend invokes it.
4. **Typed TS caller** — in the owning `src/` module (`useNotes.ts`,
   `useSettings.ts`, `quickCapture.ts`, …): an exported function wrapping
   `invoke<T>("command_name", { … })` from `@tauri-apps/api/core`, plus a
   wire-shape `type` whose doc comment cites the Rust DTO it mirrors.
5. **Events (when used)** — a command that emits an event needs a matching
   frontend listener; the event name is part of the contract.

The invoke string in step 4 **must equal the Rust function name exactly**
(snake_case); DTO field casing must match the serde attributes and the TS type.

Enforcement: the `/add-tauri-command` skill scaffolds all layers; the
`tauri-command-auditor` agent cross-checks parity and flags fat wrappers.
