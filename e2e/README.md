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
pnpm e2e:build   # pnpm build && node e2e/build.mjs
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

The fixture catalogue the slices seed from also drives a manual preview, so a
change to a note surface can be looked at in every state it reaches:

```powershell
pnpm seed:vault -- --list                    # the catalogue (pnpm eats bare flags)
pnpm seed:vault C:\kodabi-fixture            # all ten scenarios
pnpm seed:vault C:\kodabi-fixture retention/recording-only sessions/needs-attention
```

It prints the two environment lines to paste. **Set both**, in the same shell you
launch from — see *Isolation*.

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

Kodabi runs three webviews in one process. The CDP debug port is baked into
every window's `additionalBrowserArgs` at compile time by `e2e/build.mjs`
(`src-tauri/tauri.e2e.conf.json`, merged via `TAURI_CONFIG`) — **not** set at
launch via `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`, which is the harness's
original mechanism and still the one to reach for on an ordinary dev machine,
but which GitHub's hosted `windows-latest` runner silently ignores (confirmed:
the app and its WebView2 children ran fine, the CDP port just never opened).
Because the port is fixed at compile time rather than chosen per run, it's a
constant (`CDP_PORT` in `lib/app.mjs`, currently `9339`), and `launchKodabi`
fails fast if that port is already bound rather than risk attaching to the
wrong process. `/json/list` then enumerates all three webviews:

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

## The CSP gate

The buffered webview console is not only diagnostic any more. This is the only
build in the repo that *enforces* the shipping Content Security Policy — under
`pnpm tauri dev` the frontend comes from Vite, which sends no CSP header at all,
so the whole policy is inert in the one build mode anyone runs day to day and
first bites in the build that ships. Two scenarios in
`quick-capture.test.mjs` close that gap: one asserts the inlined `data:` font
face actually loads, then one asserts neither webview logged a CSP refusal or
Tauri's `IPC custom protocol failed` fallback warning. The policy is annotated
source-by-source at the top of that file, since `tauri.conf.json` is strict JSON
and cannot hold a comment.

`source-pairing.test.mjs` extends that to `media-src`, which nothing exercised
before it: the app's only asset-protocol consumer is the `<audio>` in
`SessionPanel`, and quick capture never opens a note with a recording.
It also asserts the recording actually *loads*, which is a different failure —
Tauri refuses an out-of-scope asset before the CSP is consulted, so a scope that
does not follow `KODABI_KB_ROOT` leaves the console clean and playback dead.

## Isolation

Each run gets a throwaway everything under a fresh `mkdtemp` directory, via
**one** environment variable: `KODABI_SANDBOX`, pointed at that directory.
Rust derives the rest (`kodabi_core::sandbox`) — vault root, index at
`.index/index.db`, config dir, and the WebView2 profile at `.webview2`. Removed
on teardown unless `stop({ keepArtifacts: true })`.

This is the same switch `pnpm dev:sandbox` and the `/preview` skill use; the
harness has no isolation mechanism of its own. See
[`docs/DEV_SANDBOX.md`](../docs/DEV_SANDBOX.md).

The lower-level seams the switch drives are still there —  `KODABI_KB_ROOT`
(`transcribe::knowledge_base_dir`) and `KODABI_INDEX_DB`
(`index_state::open_index`) — but setting either **alongside** the switch is
refused at startup, and the harness deletes both from the child's environment so
a developer's shell cannot trip that refusal. The pairing is why the switch
exists: setting only the vault root is destructive, because
`IndexState::initialize` hands that root to the watcher and to a startup
reconcile job, so an index still living in the real app-data dir would be
converged against the foreign temp vault and lose every row for the notes it
could no longer see.

Config **is** isolated now — `settings.toml`, `device.toml` and the `_claude/`
wiring resolve through `sandbox::config_dir`, the third seam this section used
to reserve for "when something here needs to write config". Two consequences
worth knowing when reading a slice: a run boots consent-unacknowledged (the
nudge is push-only, opening on a backend event rather than on mount, so it still
lands straight on the Inbox), and on `RetentionPolicy::KeepAll`, so a
developer's own retention policy can no longer prune fixtures mid-run.

## The fixture vault

`e2e/lib/vault.mjs` holds a catalogue of named scenarios, shared by the slices
(`launchKodabi({ seed: [...] })`) and by `pnpm seed:vault` — so a preview and a
test can never disagree about what a scenario means. Scenarios are named after
the **rule** they exercise, not the data they hold:

| Scenario | What it is for |
|---|---|
| `retention/both` | transcript and recording both survived |
| `retention/transcript-only` | the recording was pruned |
| `retention/recording-only` | the transcript was pruned; the sentence shows at rest |
| `retention/nothing` | both pruned — a section with nothing to press |
| `retention/empty-transcript` | a transcript that exists and holds nothing |
| `composition/at-ceiling` | a filed session note at the 3-cluster/4-control ceiling |
| `sessions/needs-attention` | two unclaimed captures, one behind the dismissed shelf |
| `confidence/low-score` | an Inbox note well under the 0.6 routing threshold |
| `transcript/fifty-turns` | a long transcript, to prove it is uncapped |
| `source/keyword-only` | the two shapes that must *not* pair |

Four things about it are load-bearing:

- **It writes files, never index rows.** `IndexState::initialize` reconciles over
  whatever is on disk at startup, so the app indexes the fixture itself and the
  seeder exercises the real convergence path. Writing into `index.db` directly
  would bypass it and drift from what ships.
- **Seeding happens before the app starts.** Both because of that startup
  reconcile, and because the vault watcher ignores everything under `sessions/`
  outright — a transcript written after launch is never seen at all.
- **Timestamps are minted at seed time, not frozen into the catalogue.**
  Retention runs a prune sweep at launch and settings are *not* isolated (above),
  so on a machine with a `keep_days` policy a fixture dated last month would be
  deleted out from under the run: every retention scenario would collapse into
  `retention/nothing` and the slice would go red for something that is not a bug.
- **`pnpm seed:vault` refuses a directory it did not write.** It wipes before it
  seeds, and the root is caller-supplied, so a `.kodabi-fixture-vault` marker is
  what separates "my scratch fixture" from "my actual notes". Re-seeding a marked
  directory is one command; anything else needs `--force`.

The `.wav` files are generated (a RIFF header plus a tone), never committed —
this repo stays text-only, and `<audio>` needs real PCM to exercise playback at
all.

On failure the harness keeps the temp vault, prints its path, dumps both
webview consoles plus the app's own stdio, and snapshots whether `kodabi.exe`
and any `msedgewebview2.exe` children are alive (`tasklist`). Ask for that
output first; it is usually the whole diagnosis — it's what distinguished "the
app crashed" from "the app is fine, the CDP port never opened" when this
harness first ran in CI.
