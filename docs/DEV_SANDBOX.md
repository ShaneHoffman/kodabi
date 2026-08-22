# Dev sandbox

Where Kodabi keeps its state, and how a dev or agent run is kept out of it.

## The problem

Kodabi's dev build is not a separate profile. It reads and writes the same
vault, the same `settings.toml`, the same index and the same recordings a real
install does — and on Windows all of that is one folder, because
`app_config_dir()` and `app_data_dir()` both resolve to
`%APPDATA%\com.kodabi.app`, which is also the default vault root.

That is correct for a developer testing against their own notes, and wrong for
an automated one. An agent that opens the app to check a change can start a
capture into real sessions, acknowledge consent, change retention policy, or let
the launch-time retention sweep prune real notes.

So: **plain `pnpm tauri dev` is unchanged and still opens real data — that is the
user's own workflow. Agent-driven launches set one switch and touch none of it.**
See [`.claude/rules/dev-sandbox.md`](../.claude/rules/dev-sandbox.md).

## The state map

Every persistent location, what resolves it, and what a sandboxed run does with
it.

| State | Default location | Resolver | Sandboxed to |
|---|---|---|---|
| Vault root — `sessions/`, `chats/`, project note folders, `_glossary.yml`, `_routing_examples.yml`, `_category.yml`, `_ledger.yml` | `app_data_dir()` | `transcribe::knowledge_base_dir` (`KODABI_KB_ROOT`) | `<base>` |
| Note index — `index.db` (+ `-wal`, `-shm`) | `app_data_dir()/index.db` | `index_state::index_db_path` (`KODABI_INDEX_DB`) | `<base>/.index/index.db` |
| Commitment ledger — `ledger.db` (+ `-wal`, `-shm`) | `app_config_dir()/ledger.db` | `ledger_state::ledger_db_path` (`KODABI_LEDGER_DB`) | `<base>/ledger.db` |
| Settings — `settings.toml` (consent, retention, overlay, appearance, mic check, commitment and meeting-kind tuning, your name) | `app_config_dir()` | `sandbox::config_dir` → `lib.rs` setup | `<base>` |
| Device identity — `device.toml` | `app_config_dir()` | `sandbox::config_dir` → `kodabi_core::device` | `<base>` |
| Claude Code wiring — `_claude/kodabi.mcp.json`, `_claude/terminal-settings.json` | `app_config_dir()` | `sandbox::config_dir` → `terminal_cmds` | `<base>/_claude` |
| WebView2 profile (localStorage, webview state) | `%LOCALAPPDATA%\com.kodabi.app\EBWebView` — the one location *outside* the app-data folder | `WEBVIEW2_USER_DATA_FOLDER` | `<base>/.webview2` |
| Downloaded models — Parakeet, Silero VAD, bge-small | `app_data_dir()/.models` | `sandbox::models_dir` | `<base>/.models` |
| Session artifacts — `<stem>.jsonl`, `.wav`, `.dismissed` | `<vault>/sessions/` | follows the vault root | inside `<base>` |
| In-flight capture spill — `sessions/inflight/<session>/` | `<vault>/sessions/inflight/` | `kodabi_core::inflight` | inside `<base>` |
| Chat transcripts — `chats/<stem>.jsonl` | `<vault>/chats/` | `kodabi_core::chat` (`CHATS_DIR`) | inside `<base>` |
| MCP sidecar (`kodabi-mcp`) reads, and writes the ledger | its own `KODABI_KB_ROOT` + `KODABI_INDEX_DB` + `KODABI_LEDGER_DB` | inherited from the generated `.mcp.json` | follows the sandbox |

Retention keeps no bookkeeping file: membership is derived from disk on every
sweep, so there is no separate state to isolate.

**The two databases above differ in kind, not just in contents.** `index.db` can
be deleted and reconstructed from the Markdown at any time; `ledger.db` holds
judgements (a waiver, a snooze, a closure and its evidence) that exist nowhere
else, so it is the one *database* that is durable rather than derived. Its backup is the vault, not the config dir:
every change to an *entry* is mirrored into a per-project `_ledger.yml`, and a
missing or empty `ledger.db` is rebuilt from those at startup. The one thing that
never reaches a snapshot is `ledger_meta`, which holds device-local viewing state
(the triage marker) rather than a judgement, and is re-stamped on a rebuilt
database. A sandboxed run therefore gets both
halves under `<base>` — the database directly, the snapshots inside the fixture
vault — so re-seeding the fixtures discards the two together and leaves them
consistent.

**Deliberately not sandboxed:**

- **The tray-promotion registry key.** `tray_promotion` writes `IsPromoted` under
  `HKCU\Control Panel\NotifyIconSettings\<id>`, which Explorer keys by executable
  path. It is machine-global, best-effort, and write-once-if-unset, and a
  sandboxed run uses the same exe as a real dev run — so there is nothing to
  separate it by. The blast radius is one taskbar-icon preference.
- **Claude Code's own state** (`~/.claude`, session history). Outside Kodabi's
  process entirely.
- **Symlinks and junctions.** The overlap check below is lexical, not
  canonicalising — a link pointing into the real app dir is not detected.

## The switch

`KODABI_SANDBOX` takes one of two forms:

| Value | Sandbox base |
|---|---|
| `1` or `true` (case-insensitive) | `%APPDATA%\com.kodabi.app-dev` — the real app dir's `-dev` sibling |
| an absolute path | that path |

Everything else derives from the base, in `kodabi_core::sandbox::resolve`. The
base is both the vault root and the config dir, which mirrors the shape a real
Windows install has, so a sandboxed run exercises the code paths that ship. The
index, models and WebView2 profile are dot-prefixed subdirectories: vault
enumeration skips `.`/`_` entries, so none shows up as a phantom project, and
the seeder's wipe leaves dot-entries alone so `.index/` survives a re-seed.

A sandboxed `.models/` is normally empty, and that is correct rather than a gap:
a debug build transcribes with the mock engine and reads no model at all, so
borrowing the real directory would mean reaching into the real app dir for
nothing. A sandboxed run that genuinely needs real models uses the
`PARAKEET_*` / `KODABI_EMBED_MODEL_DIR` developer override, which is read
before the models directory and is not part of what the sandbox relocates.

`<base>/.index/index.db` is byte-identical to what `indexDbFor()` in
`e2e/lib/vault.mjs` computes — a seeded directory and a sandboxed launch agree
on where the index lives, and the two constants cross-reference each other.

**One switch, not three variables.** `KODABI_KB_ROOT`, `KODABI_INDEX_DB` and
`KODABI_LEDGER_DB` are still the low-level seams, and they are only safe when
moved together: `IndexState::initialize` hands the KB root to a startup reconcile
job, so relocating the vault while the index stays behind converges the real
index against a foreign vault and deletes every row for the notes it can no
longer see, and a ledger left behind while the vault moves judges commitments
that no longer exist. The sandbox derives all three from one base so that
grouping cannot be half-set.

## Refusals

A sandboxed launch that cannot be made safe exits non-zero with a message
naming what to change, rather than falling back to real directories — a caller
who asked for a sandbox and silently got real data is the outcome worth crashing
over.

| Condition | Message |
|---|---|
| `KODABI_KB_ROOT`, `KODABI_INDEX_DB`, or `KODABI_LEDGER_DB` set alongside the switch | `<var> is set alongside KODABI_SANDBOX, which derives the vault, index and ledger itself. Unset <var>, or drop KODABI_SANDBOX to use it directly.` |
| A relative base | ``KODABI_SANDBOX must be `1`, `true`, or an absolute path; got the relative path …`` |
| A base that equals, contains, sits inside, or walks (`..`) into a real app dir | `sandbox base … overlaps the real app directory …. Refusing to touch real data in sandbox mode; choose a base outside it.` |

Paths are compared component-wise and case-insensitively. Component-wise
matters: a string-prefix test would call `com.kodabi.app-dev` a child of
`com.kodabi.app` and refuse the default base. Nothing is canonicalised (the base
usually does not exist yet), so a `..` component anywhere in the base takes the
same refusal rather than being walked: `…\com.kodabi.app-dev\..\com.kodabi.app`
compares unequal component-wise but resolves to the real app dir.

Once activation succeeds, resolving to real data is structurally impossible
rather than merely checked — `sandbox::activate` overwrites `KODABI_KB_ROOT`,
`KODABI_INDEX_DB` and `KODABI_LEDGER_DB` in the process environment before the
Tauri builder exists, so
every existing resolver, the asset-protocol scope widening, the generated
`.mcp.json` and every spawned child follow the sandbox with no sandbox-specific
branch of their own.

## Chat distill

Sandbox activation also sets `KODABI_DISABLE_CHAT_DISTILL` when it is absent.
Meeting distill is feature-gated off in dev builds, but chat text is real in
every build, so without this every chat ended in a preview session would spend a
live Claude call distilling fixture conversation.

It only *defaults* it. To exercise the distill path inside a sandbox, set the
variable to the empty string — `distill_disabled()` reads empty as unset:

```powershell
$env:KODABI_DISABLE_CHAT_DISTILL=""
pnpm dev:sandbox
```

## The three launch surfaces

| Surface | Base | Notes |
|---|---|---|
| `pnpm dev:sandbox` | worktree-local `.sandbox/` (gitignored) | Seeds the fixture catalogue on first run; reuses existing state after. The agent-facing default. |
| `/preview` skill | same | Always sandboxed. See [`.claude/skills/preview/SKILL.md`](../.claude/skills/preview/SKILL.md). |
| e2e harness | a fresh `mkdtemp` dir per run | `launchKodabi` sets the same switch; removed on teardown. See [`UI_E2E_HARNESS.md`](UI_E2E_HARNESS.md). |
| `pnpm tauri dev` | **none** | Real vault, real settings, real index. Unchanged, and the user's own workflow. |

Worktree-local rather than a shared `-dev` sibling because Kangentic runs
several task worktrees at once: a shared sandbox would have two sessions
fighting over one settings file and one index, and its state would outlive the
branch that made it.

Seeding a sandbox by hand (the app must be closed — Windows will not unlink the
`.wav` a running app holds open):

```powershell
pnpm seed:vault -- --list                       # the scenario catalogue
pnpm seed:vault .sandbox                        # all of them
pnpm seed:vault .sandbox retention/recording-only sessions/needs-attention
```

Any other absolute base works the same way:

```powershell
pnpm seed:vault C:\kodabi-fixture
$env:KODABI_SANDBOX="C:\kodabi-fixture"
pnpm tauri dev
```

## Where the code lives

- [`crates/kodabi-core/src/sandbox.rs`](../crates/kodabi-core/src/sandbox.rs) —
  the pure resolver: env values plus the real app dirs in, paths or a refusal
  out. All the layout and overlap rules, unit-tested.
- [`src-tauri/src/sandbox.rs`](../src-tauri/src/sandbox.rs) — the shell:
  activation, the process-environment rewrite, and `config_dir`, the third seam
  for `settings.toml` / `device.toml` / `_claude/`.
- [`scripts/dev-sandbox.mjs`](../scripts/dev-sandbox.mjs) — `pnpm dev:sandbox`.
- [`e2e/lib/vault.mjs`](../e2e/lib/vault.mjs) — the fixture catalogue and
  `indexDbFor`.
