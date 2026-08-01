---
name: scaffold-feature
description: Plan and build a full-stack Kodabi feature in dependency order (core → wrapper → registration → TS → UI), with a confirmation gate before any code is written. Use for a feature that spans the Rust backend and the React frontend.
argument-hint: [feature description]
---

# Scaffold a feature

Build a feature that crosses the stack, bottom-up, so each layer rests on a tested
one below it.

What to build (may be empty): $ARGUMENTS

## 1. Explore

Find the crates, `src-tauri` modules, and screens the feature touches, and an
existing analogue to copy the shape from. Read before planning.

## 2. Plan, then confirm

Write a layer-by-layer file list: core types/functions and their tests → any new
Tauri command(s) → the `generate_handler!` registration → the TS module functions
and wire types → the UI. Present it and get the user's go-ahead with
`AskUserQuestion`. **Write no code before confirmation.**

## 3. Implement in dependency order

1. **Core** — types and logic in the owning `kodabi-*` crate, with unit tests
   (core-vs-shell).
2. **Wrapper(s)** — thin `#[tauri::command]`s (DTOs + one core call + error map);
   see [`.claude/rules/tauri-command-parity.md`](../../rules/tauri-command-parity.md)
   or run `/add-tauri-command` per command.
3. **Register** in `src-tauri/src/lib.rs`.
4. **TS** — typed `invoke` wrappers and wire types in the owning `src/` module.
5. **UI** — compose the primitives in `src/components/ui/` and style with **Grove
   utilities**, never a colour literal and never a new stylesheet (the `@theme` block in
   `src/index.css` is the source; `docs/DESIGN_SYSTEM.md` is the doctrine and
   `docs/UI_CONVENTIONS.md` is the discipline).

## 4. Verify

Run the gates for every surface touched (the `/commit` matrix). If the change has a
visible runtime surface, run `/preview` and smoke-test the flow.

## 5. Follow-ups

Offer a `/test` audit of the new code, and note that `/add-tauri-command` already
spawned `tauri-command-auditor` for any commands added.
