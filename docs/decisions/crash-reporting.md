# Crash reporting: none in v1, and opt-in only if ever

**Decision: v1 ships no crash reporting of any kind. Should it ever be added, it is strictly opt-in,
local-capture-first, and never transmits user-derived content automatically. Closed 2026-08-14.**

This closes the Phase 4 bullet in `docs/ROADMAP.md` that had read only "Crash reporting decision
(opt-in only)" — a parenthesis asserting a constraint on a feature nobody had specified. This
document specifies it, so the constraint has something to bind, and so the next person to ask "should
we add crash reporting?" starts from a recorded position rather than from scratch.

No production change lands with this decision. Only this document and its two cross-links commit.

## 1. What was asked

Two signed releases (v0.1.0 and v0.2.0) have shipped to real installs with zero visibility into what
happens when the app falls over on someone else's machine. The ROADMAP named the leftover but not the
answer, and the parenthetical "(opt-in only)" was the entire recorded thinking.

The question is genuinely two questions, and conflating them is why it stayed open:

1. **Does anything capture a crash locally?** (Today: no.)
2. **Does anything leave the machine?** (Today: no, and this document says it stays that way by
   default.)

## 2. What exists today, surveyed 2026-08-14

**No crash reporting, and no telemetry of any kind.** A repo-wide grep for
`set_hook|sentry|minidump|breakpad|crashpad|telemetry` across `src/`, `src-tauri/`, `crates/` and
`package.json` returns only false positives: the substring `sEntry` inside `isEntrySelected`
(`src/useProjects.ts:42`), and the word "telemetry" inside sample meeting prose in a test fixture
(`crates/kodabi-core/tests/pipeline_composition.rs:586`). There is no dependency, no config, and no
dead code from an abandoned attempt.

**No panic hook.** `std::panic::set_hook` appears nowhere in the workspace. A panic on any thread
other than the two contained below unwinds into nothing.

**Panic *containment* exists, and is not reporting.** Two sites wrap a job in `catch_unwind` so a
panic inside it still yields a terminal event rather than a stuck spinner:
`src-tauri/src/distill_cmds.rs:253` (the meeting distill) and
`src-tauri/src/chat_distill_cmds.rs:119` (the chat distill), sharing the payload extractor
`panic_message` at `src-tauri/src/distill_cmds.rs:341`. That extracted string becomes the status of
*that one job* — surfaced to the user as "distill panicked: …" — and is written nowhere. It does not
survive the session, and no other panic anywhere in the app is captured at all.

**No general logging infrastructure, therefore nothing to report even if reporting existed.** There
is no `tauri-plugin-log`, no `log`, `env_logger`, `tracing` or `tracing-subscriber` in
`src-tauri/Cargo.toml` (the only Tauri plugins are `global-shortcut`, `notification`, `updater` and
`process`). Diagnostics are 98 `eprintln!` calls to stderr across `src-tauri/src/` and `crates/`.
**In a windowed release build on Windows, that stderr goes nowhere the user can retrieve.**

**The one file sink is opt-in, and carries no crash data.** Setting `KODABI_METRICS` to a path makes
each completed transcription append a timings line to it (`src-tauri/src/transcribe.rs:832`, appended
at `:878`; documented at `docs/RESOURCE_BUDGET.md:42`). Unset — the default — it emits nothing. It
exists for resource-budget measurement passes and records pipeline durations, not panics, so it is
not a crash log; but it is a real appender, and the sentence "the app writes no log file" is only
true with it named.

This is the finding that shapes the decision: the gap is not "we have crash data and choose not to
send it." The gap is that no crash data exists. Local capture is the prerequisite; transmission is a
second, separable question that is not owed an answer yet.

**One crash handler ships regardless, and it is not ours.** WebView2's Chromium fleet includes a
crashpad helper process, noted in `docs/RESOURCE_BUDGET.md:149` only as memory accounting
("GPU process, renderer, network service, crashpad, etc."). Kodabi neither chose nor configured it.
**Whether it reports anywhere, and to whom, was not investigated** — it is Microsoft's component
under Microsoft's own policy, and it would remain exactly as it is under every option below. It is
recorded here so a future reader does not mistake it for evidence that the app has crash reporting.

## 3. The posture any answer has to square with

- `README.md:97` — **"Everything stays on your machine as plain files — audio and transcripts never
  leave except through your own Claude account."** Sourced from `docs/FOUNDING_DOC.md:36`, which
  states it as a core belief. Note the carve-out is narrow and names exactly one egress.
- `docs/FOUNDING_DOC.md:60` — "no vendor economics, no data custody questions."
- The repo already suppresses *third-party incidental* logging of user content: both places Kodabi
  spawns `claude`, it sets `CLAUDE_CODE_SKIP_PROMPT_HISTORY` so transcript text does not linger in
  Claude Code's own session logs (`docs/FOUNDING_DOC.md:185`). A project that goes out of its way to
  stop someone *else's* tool from writing user text to disk cannot casually start shipping its own.
- **The updater is the one unasked outbound call, and it is precedent for the opposite conclusion.**
  It checks GitHub on every launch with no consent gate (`src/useUpdater.ts:87`, switched on outside
  dev at `src/components/providers/UpdaterProvider.tsx:18`), and nothing after that check moves
  without a click (`src/useUpdater.ts:17`) — as the Settings copy tells the user verbatim:
  "Kodabi checks for a new version at startup. Nothing downloads or installs until you say so."
  (`src/components/views/SettingsView.tsx:360`). That is acceptable because it sends **no user
  content**. A crash report is the inverse: its entire value *is* content, and panic messages and
  stack frames routinely carry file paths (hence usernames and note titles), and can carry fragments
  of whatever was being processed. The updater's carve-out does not extend to it. **That asymmetry is
  the whole reason the answer here is opt-in rather than "quiet and content-free like the update
  check."**

## 4. Decision

**v1 ships no crash reporting.** Nothing is built for this ticket.

The following clauses are the standing policy. They are not a plan to implement anything; they are
the constraints any future implementation inherits, so that a revisit argues within them rather than
reopening them:

1. **Strictly opt-in.** Off unless the user turns it on, by an explicit act. Never a pre-checked box,
   never bundled into accepting something else, never a prompt whose dismissal enables it. The
   consent nudge and the model download are the in-repo shape to copy: the app asks once, plainly,
   and does nothing until told.
2. **Local capture first, and it is separable.** The first increment is ever only a panic hook
   writing a crash log the user can open and read in full. That has standalone value — it makes a
   field crash diagnosable *at all*, via a user pasting it into a GitHub issue — and it involves no
   egress, so it needs no consent gate beyond writing a file the user owns. It is a feature ticket,
   not this one.
3. **Sharing is a manual act.** A user attaching a log they have read to an issue they chose to file
   is the supported path. It costs nothing to build and it is unambiguously consented.
4. **No third-party crash service.** Sentry and equivalents are ruled out: they add vendor economics
   and a data-custody question to a project whose pitch is having neither
   (`docs/FOUNDING_DOC.md:60`), for content the project has committed to keeping local.
5. **No automatic transmission of user-derived content, ever, opt-in or not.** Should automatic
   submission ever be wanted, it needs its own decision doc and its own README amendment — it is a
   change to the local-first promise, not an implementation detail of this one.

**No README amendment is proposed**, because nothing changes: the README makes no claim about
diagnostics today, and "the app collects nothing" is what it already implies. The clause becomes owed
only when clause 2 or 5 is acted on.

## 5. What this decision changes

- `docs/ROADMAP.md` — the Phase 4 crash-reporting bullet is checked and points here.
- `docs/FOUNDING_DOC.md` §7 — a born-closed row in Open Decisions.

Deliberately untouched:

- `README.md` — see §4. Nothing about the app's behaviour changed, so a new privacy sentence would
  describe an absence the section already implies.
- The two `catch_unwind` sites — they are job containment, not crash handling, and this decision
  neither widens nor removes them.
- No ticket is filed for local capture. Filing one would assert it is wanted; §6 is the condition
  under which it becomes wanted.

## 6. Revisit triggers

Testable conditions, not vibes. Any one reopens the question — starting from §4's clauses:

1. **A field crash is reported that cannot be diagnosed from the reporter's description**, and a
   local log would plausibly have resolved it. One such report is enough to justify clause 2's panic
   hook; it does not touch clauses 3–5.
2. **Public launch happens** (the ROADMAP's held final bullet) and the install base outgrows
   issue-by-issue diagnosis.
3. **A distribution channel requires a stated crash-handling posture** — a store listing, or a
   security review that asks the question directly.
4. **Logging infrastructure lands for another reason.** If the app grows a general log — a durable
   sink that is on by default, unlike today's opt-in `KODABI_METRICS` timings appender (§2) — then
   clause 2's prerequisite is largely paid and the cost of the panic hook drops to near zero.

## 7. Reproducing

- **The absence claims** — from the repo root:
  `grep -rniE "set_hook|sentry|minidump|breakpad|crashpad|telemetry" --include=*.rs --include=*.ts --include=*.tsx --include=*.toml --include=*.json src src-tauri crates package.json`
  should return only the two false-positive classes named in §2.
- **The logging claim** — `grep -nE "tauri-plugin|^log|tracing|env_logger" src-tauri/Cargo.toml`
  lists four plugins and no logging crate; `grep -rn "eprintln!" src-tauri/src crates --include=*.rs`
  counts the stderr sites (98 on 2026-08-14).
- **The containment sites** — `grep -rn "catch_unwind" src-tauri/src` returns exactly the two call
  sites in §2, each preceded by its explaining comment (four lines).
