# End-to-end harness

Drives the **real app window** — the real React frontend, over the real Tauri
IPC bridge, against the real Rust backend and real files on disk.

This is the tier `pnpm test` structurally cannot be. That suite mocks
`invoke`/`listen` at the IPC boundary (`src/test/tauri.ts`), so an `onClick`
that was never wired, a renamed invoke string, and a DTO field-casing mismatch
all pass it and reach users. Why CDP over `tauri-driver`, and what this tier
deliberately does *not* cover, is in
[`docs/UI_E2E_HARNESS.md`](../docs/UI_E2E_HARNESS.md).

## Running it

```powershell
pnpm e2e:build   # pnpm build && cargo build -p kodabi --features tauri/custom-protocol
pnpm test:e2e
```

**Run `pnpm e2e:build`, not `cargo build` alone.** `dist/` is embedded into the
exe at compile time by `tauri::generate_context!`, so a frontend change that has
not been through `pnpm build` is simply not in the binary under test — the run
then reports a failure that no longer exists in the source, or passes over a bug
that does. The combined script exists to make that ordering unforgettable.

Requires Windows and the WebView2 Evergreen runtime. Node 24+ is the repo
prerequisite (the harness itself needs only 22+, for the global `WebSocket`).
No npm dependencies, and no `pnpm install` — the harness is Node built-ins only.

## Why `--features tauri/custom-protocol`

`src-tauri` declares no `custom-protocol` feature of its own, so the flag is
addressed through the dependency. It flips tauri's `dev` cfg off, so the exe
serves the embedded `dist/` from `http://tauri.localhost` rather than expecting
a Vite dev server on port 1420 — while staying on the **debug profile**, which
keeps the `MockEngine` STT stub (no models, no native audio deps) and keeps the
release-only `compile_error!` engine guard quiet.

That combination is the whole reason this tier is cheap: it needs no STT model,
no release build, and no Vite server.

## How it drives the app

Kodabi runs three webviews in one process. `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`
opens a CDP port, and `/json/list` enumerates all three:

| Window | Target URL |
|---|---|
| `main` | `http://tauri.localhost/` |
| `quick-capture` | `http://tauri.localhost/capture.html` |
| `capture-overlay` | `http://tauri.localhost/overlay.html` |

Note `main` is a **bare path**, not `/index.html`: it declares no `url` in
`tauri.conf.json` and so takes the `WebviewUrl` default.

Everything is driven through `Runtime.evaluate` rather than OS-level input.
That is a deliberate design choice, not a shortcut: the quick-capture window
hides itself on real focus loss (the `DismissArmed` guard in
`src-tauri/src/quick_capture.rs`), so a harness that moved OS focus between the
two windows would race against the app dismissing the very window under test.
CDP evaluation never touches focus, so the race cannot fire — which also means
the slice can drive the `visible: false` quick-capture window without ever
showing it.

Two constraints the page CSP imposes on evaluated expressions, both free to
honour: never inject a `<script>` element (page-originated, and blocked), and
never use `eval`/`new Function`. Plain expressions only.

## Isolation

Each run gets a throwaway vault and index under the system temp dir, via **two**
environment variables that must always be set together:

- `KODABI_KB_ROOT` — the vault root (`transcribe::knowledge_base_dir`)
- `KODABI_INDEX_DB` — the full path to the index db (`index_state::open_index`)

Setting only the first is destructive: `IndexState::initialize` hands the KB
root to the watcher and to a startup reconcile job, so an index still living in
the real app-data dir would be converged against the foreign temp vault and lose
every row for the notes it could no longer see.

`WEBVIEW2_USER_DATA_FOLDER` is also per-run, so the harness never shares a
browser process with a developer's already-running instance.

Not isolated: `app_config_dir()` (settings and consent) is still the real one.
The slice reads it and writes nothing, and the consent nudge is push-only — it
opens on a backend event, never on mount — so an un-onboarded machine still
boots straight to the Inbox. Add a third seam only when something here needs to
*write* config.

On failure the harness keeps the temp vault, prints its path, and dumps both
webview consoles plus the app's own stdio. Ask for that output first; it is
usually the whole diagnosis.
