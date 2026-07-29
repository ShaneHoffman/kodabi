# UI end-to-end harness: CDP over WebView2 vs. tauri-driver

**Status:** Living (Phase 4 spike, branch `test/ui-e2e-harness`).

**Decision: drive the real app window over WebView2 remote debugging (CDP) from a
zero-dependency Node harness, as one opt-in Windows-only tier. `tauri-driver` is
not adopted. Closed 2026-07-28.**

Kodabi had no UI end-to-end testing of any kind. `pnpm test` is vitest +
Testing Library under jsdom, mocking `invoke`/`listen` at the Tauri IPC boundary
(`src/test/tauri.ts`), so the React app was never once driven against the real
Rust backend. Nothing caught "the button doesn't actually call the command", a
renamed invoke string, or a DTO field-casing mismatch — the exact failures
[`.claude/rules/tauri-command-parity.md`](../.claude/rules/tauri-command-parity.md)
exists to prevent, and which that rule could only ask reviewers to watch for.

`preview-mock.js` looks like it filled this role and does not: it fakes
`window.__TAURI_INTERNALS__` so the app renders in a plain browser, with no
assertions and no CI presence. It is a design-review harness.

## What the tier has to prove

Three failure classes, in increasing order of what it takes to catch them:

1. **A renamed or unregistered invoke string.** Compiles, typechecks, fails only
   when a user presses the button.
2. **A DTO field-casing mismatch.** Same, and worse: the jsdom suite *mocks* the
   wire shape, so it cannot see a Rust-side rename by construction.
3. **A control that renders but is not wired** — an `onClick` never attached, an
   always-true `disabled`, an early return. No static analysis catches this.

## Setup — what "run the real app" costs

The build under test is not one any existing gate produces, and it is not a
plain `cargo build` either — `pnpm e2e:build` (`e2e/build.mjs`) does two things
together:

```
cargo build -p kodabi --features tauri/custom-protocol --locked
# plus TAURI_CONFIG=<contents of src-tauri/tauri.e2e.conf.json>
```

`src-tauri` declares no `custom-protocol` feature of its own, so it is addressed
through the dependency. This flips tauri's `dev` cfg off (verified:
`cargo:dev=false`, `cargo:rustc-cfg=custom_protocol`), so the exe serves the
embedded `dist/` from `http://tauri.localhost` instead of expecting a Vite dev
server — while staying on the **debug profile**, which keeps the `MockEngine`
STT stub and keeps the release-only `compile_error!` engine guard quiet. So the
tier needs no STT model, no release build, and no Vite server.

The `TAURI_CONFIG` half bakes the CDP debug port into every window's
`additionalBrowserArgs` at compile time (`tauri_build::build()` merges it via
JSON Merge Patch — RFC 7396 — so `tauri.e2e.conf.json` restates every field of
all three windows, not just the new one, or the merge would wipe the rest of
each window's config). **This replaced the harness's original mechanism**,
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` set at launch: it worked on an ordinary
dev machine but was silently ignored on GitHub's hosted `windows-latest`
runner — confirmed by process inspection (below), not assumed. The port is
therefore fixed (`CDP_PORT` in `e2e/lib/app.mjs`) rather than chosen per run;
`launchKodabi` fails fast if that port is already bound rather than silently
attaching to the wrong process.

Two always-on environment seams point a run at a throwaway vault:
`KODABI_KB_ROOT` and `KODABI_INDEX_DB`, reusing the exact names `kodabi-mcp`
already reads and the generated `.mcp.json` already writes. They must be set
together — see **Caveats**. Details in [`e2e/README.md`](../e2e/README.md).

Teardown is `taskkill /PID <pid> /T /F`: `/T` because WebView2 spawns
`msedgewebview2.exe` children that do not reliably die with the host, `/F`
because the app intercepts `CloseRequested` and hides to tray, so a polite kill
hangs forever.

## Alternatives considered

- **A. `tauri-driver` + WebDriver (msedgedriver)** — the official answer.
- **B. CDP over WebView2 remote debugging** — chosen.
- **C. No UI tier; a static invoke-string parity check instead.**
- **D. Status quo** — jsdom with mocked IPC only.

C was not treated as a rival: it was **also adopted**, as `src/invokeParity.test.ts`.
It is complementary, not competing, and the table shows why.

## Results

Rows are what we need; **bold** marks where an option wins.

| Axis | A. tauri-driver | **B. CDP over WebView2** | C. Static check |
|---|---|---|---|
| Catches a renamed / unregistered invoke string | yes | **yes** (verified) | **yes** (verified) |
| Catches a DTO field-casing mismatch | yes | **yes** (verified) | partial (name only) |
| Catches a control that renders but is not wired | yes | **yes** (verified) | **no** |
| Catches a control behind an overlay / `pointer-events: none` | **yes** | no | no |
| Reaches the second, `visible: false` webview | unproven | **yes** (verified: 3 targets) | n/a |
| Survives the `DismissArmed` blur guard | at risk | **yes** (never touches focus) | n/a |
| New dependencies | ~500–1000 npm + a cargo binary | **0** | **0** |
| Pinned to an auto-updating runtime | msedgedriver ↔ WebView2 | **no** | **no** |
| Wall-clock for the slice | not measured | ~1.3 s | **<0.1 s** |
| Warm CI minutes (windows-latest) | ~8–12 | ~6–9 | **0** (rides the existing Linux job) |
| Reporters, retries, screenshots | **yes (wdio)** | roll your own | n/a |
| Cross-platform if Kodabi leaves Windows | **yes** | no | **yes** |
| Code the repo owns forever | ~50 lines of config | ~400 lines | ~100 lines |

### Reading the table — "official support" is not the axis that decides this

`tauri-driver` wins on paper: it is a Tauri-org crate, it produces **trusted**
OS-level input, and wdio brings reporters, retries and screenshot-on-failure
that this harness does not have. Two rows decide against it anyway, and both are
specific to *this* app rather than general claims about the tools.

### Why real OS input, tauri-driver's headline win, is a liability here

The slice spans two webviews, and one of them hides itself on real focus loss —
the `DismissArmed` guard in `src-tauri/src/quick_capture.rs`. Trusted input
requires the target window to be foreground, so every focus transfer between
`capture.html` and `index.html` races the app dismissing the window under test.
CDP `Runtime.evaluate` never touches OS focus, so the race cannot fire — and the
harness can drive the `visible: false` quick-capture window *without ever
showing it*, which removes the race from the design rather than papering over it.

The second row is an **evidence asymmetry, not a technical superiority claim**:
CDP's `/json/list` enumerates all three webviews by construction, and this was
confirmed on the first probe. Whether msedgedriver's WebView2 mode surfaces a
hidden sibling webview as a window handle is unknown, and every published Tauri
E2E example drives only the main window. That unknown costs a day to resolve for
A and cost an hour for B.

### What the static check buys, and the one thing it cannot

C catches failure class 1 outright, in under a second, on the existing Linux
job, with no Windows runner and no possibility of flake. It is the best value in
the table by a wide margin, which is why it was adopted too.

It cannot catch failure class 3. **That single gap is the entire justification
for the expensive tier**, and it was measured rather than argued: unwiring
`onClick={submit}` in `QuickCapture.tsx` left the static check green and the
jsdom suite green, and turned the E2E slice red. If that gap ever stops
mattering, this tier should be deleted.

## Decision & rationale

**CDP over WebView2, as one opt-in Windows-only tier, plus the static check.**

- **Coverage** — it is the only option that catches all three failure classes,
  and each one is verified by a mutation rather than assumed.
- **Robustness** — no driver/runtime version pin, and the blur-guard race is
  designed out.
- **Weight** — zero dependencies, against a repo that hand-rolled its own
  combobox rather than take one (`.claude/rules/typescript-style.md`), and a
  worktree whose `node_modules` is shared with the main checkout.
- **Speed** — ~1.3 s for the slice, on a debug build that needs no model.

**What `tauri-driver` remains right for**, and this tier explicitly does not
cover: the global hotkey `Ctrl+Alt+Space`, real focus behaviour, and the
blur-dismiss path itself. Those are OS integration, `Runtime.evaluate` cannot
reach them at all, and they keep their Rust-side tests. If Kodabi ever ships
beyond Windows, revisit A on the cross-platform row alone.

## Caveats / threats to validity

**One slice is not a suite.** Exactly one path is covered — quick capture into
the Inbox. A green run says that path is wired, nothing more. The value is the
harness plus the precedent; further slices are cheap only in proportion to how
many `data-testid`s exist.

**The two vault seams are a pair, and the pairing is load-bearing.**
`IndexState::initialize` hands the KB root to the watcher and to a startup
reconcile job. Setting `KODABI_KB_ROOT` while the index stayed under the real
app-data dir would converge a developer's real index against the empty temp
vault and delete every row for the notes it could no longer see. The seams are
always-on rather than `#[cfg(debug_assertions)]` precisely so the tier exercises
the code path that ships — but that means an environment variable can now
relocate a user's notes.

**Driving a hidden webview skips the show path.** Nothing here exercises
`show_quick_capture`, the hotkey, or the dismiss-on-blur behaviour. Those are
untested by this tier and must not be assumed covered.

**Runner-image coupling — confirmed, not hypothetical.** The tier depends on
the WebView2 Evergreen runtime being present, and its first live CI run showed
that a runtime *can* diverge from a dev machine in exactly the way this section
originally only speculated about: `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` was
silently ignored on GitHub's hosted `windows-latest` runner. Confirmed by
process inspection, not assumed — `kodabi.exe` and seven `msedgewebview2.exe`
children were alive with real memory usage (up to 134 MB) for the full wait
window, so the app launched and rendered fine; the CDP port simply never
opened. Switched to the config-level `additionalBrowserArgs` mechanism (see
**Setup**), verified locally; CI confirmation is the next run on this branch.
The observed runtime version was `Edg/150.0.4078.105`; the harness logs it so a
future CI-only failure names the build it happened on. If a future runner
image also ignores `additionalBrowserArgs`, the next fallback is driving the
OS-level `--remote-debugging-port` via a registry policy rather than any
Tauri-level mechanism.

**The build-order trap is real and bit us during the spike.** `dist/` is
embedded at compile time, so `cargo build` without a preceding `pnpm build`
tests a stale frontend and produces confidently wrong results. `pnpm e2e:build`
exists to make that unforgettable; CI sidesteps it by downloading the `dist`
artifact the frontend job already publishes.

## Promotion / retirement criteria

The CI job lands **non-required**, and deliberately so: the required checks live
in GitHub branch-protection settings rather than in the tree, so the setting
cannot flip atomically with the code. Flip it first and the PR adding the job can
never merge, because the check does not yet exist on `main`.

- **Promote to required** after 20 consecutive green runs on `main` with zero
  failures that were not real bugs. The job already copies the in-job change gate
  used by the `app` job, so it always reports — promotion needs no YAML change.
- **Retire it** if a flake that is not a real bug is not fixed within one
  attempt. Delete the job, keep the harness local-only, and amend this doc to
  say so. A merge-blocking check nobody can fix from the tree is worse than no
  check.

## Reproducing

```powershell
pnpm install --frozen-lockfile
pnpm e2e:build
pnpm test:e2e
```

Prerequisites: Windows and the WebView2 Evergreen runtime. Node 24+ is the repo
prerequisite; the harness itself needs only 22+, for the global `WebSocket`. No
npm dependencies are added by this tier.

To confirm the tier still catches what it claims, mutate and observe — each of
these was run for this decision:

| Mutation | Expected result |
|---|---|
| Rename the invoke string in `src/quickCapture.ts` | static check **and** slice go red |
| Replace `onClick={submit}` with a no-op in `QuickCapture.tsx` | **only** the slice goes red |
| Rename `inbox_note_count` in `note_cmds.rs` | **only** the sidebar-count assertion goes red |
