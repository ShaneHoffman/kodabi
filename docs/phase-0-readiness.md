# Phase 0 readiness audit

**Date:** 2026-07-12 · **Task:** #14 (`chore/p0-readiness-audit`) · **Verdict: GO for Phase 1.**

Phase 0 is complete. Every deliverable is Done with merged-PR evidence except one small,
non-blocking item (name reservations), now ticketed in the backlog. No hard blocker stands
between the board and the first Phase 1 ticket.

---

## 1. Phase 0 status

The task brief listed P0-8 / P0-9 / P0-10 / #13 as "still in Planning" — all four merged to
`main` after the brief was written (PRs #7, #9, #10, #8 respectively).

| Deliverable | Ticket | Status | Evidence |
| --- | --- | --- | --- |
| Name → Kodama | — | ✅ Done | `docs/FOUNDING_DOC.md` §7 (decided) |
| License → AGPL-3.0-only | #12 | ✅ Done | PR #1; `LICENSE`, README §License, workspace `license` field |
| Moodboard / aesthetic lock | #3 (P0-3) | ✅ Done | PR #3; `docs/DESIGN.md`, `design/moodboard.html` |
| Design tokens | #4 (P0-4) | ✅ Done | PR #4; `design/tokens.css` (+ `tokens.html` demo) |
| Spirit-mark / listening-indicator concept | #5 (P0-5) | ✅ Done | PR #5; `docs/SPIRIT_MARK.md`, `design/spirit-mark.html` |
| Repo scaffold (Tauri + workspace + frontend) | #7 | ✅ Done | PR #6; `src-tauri` + `crates/kodama-core` + `src/` — matches README layout |
| CI pipeline | #8 (P0-8) | ✅ Done | PR #7; `.github/workflows/ci.yml` — eslint (max-warnings 0), `cargo fmt --check`, clippy `-D warnings`, tests; Tauri release build delegated to the `/pull-request` skill |
| Frontmatter schema | #9 (P0-9) | ✅ Done | PR #9; `docs/FRONTMATTER_SCHEMA.md` (amended by this audit — see §3) |
| MCP tool surface v1 | #10 (P0-10) | ✅ Done | PR #10; `docs/MCP_TOOL_SURFACE.md` |
| Git-flow / branch policy | #11 | ✅ Done | PR #2; `CLAUDE.md` + `CONTRIBUTING.md`; every post-#11 branch conforms to `type/slug` |
| docs/ consolidation | #13 | ✅ Done | PR #8 (completed by this audit — two specs had landed at root; see §3) |
| Reserve domain / crates.io / npm names | new | 🔶 Open, ticketed | Backlog item "Reserve names: crates.io / npm / domain variant" (`chore/reserve-namespaces`); **not a Phase 1 blocker** |

### The FOUNDING_DOC.md discrepancy — resolved

The seed finding ("`FOUNDING_DOC.md` does not exist anywhere in the repo") was true when the
brief was written but is stale: commit `fa84657` (PR #8, ticket #13) created
`docs/FOUNDING_DOC.md` as part of the docs/ consolidation. `CLAUDE.md` and `docs/ROADMAP.md`
already pointed at the correct path. The one genuinely dangling reference — a provenance note
*inside* `MCP_TOOL_SURFACE.md` claiming the founding doc "is not yet present in this repo"
(written on a branch that predated PR #8's merge) — is fixed by this audit.

---

## 2. Phase 1 readiness gate

**GO.** The three gates named in the brief have all closed:

- **P0-10 (MCP tool surface)** — done; and it never gated Phase 1 anyway. The surface is consumed
  by the Phase 2 retrieval pipeline and the Phase 3 server. Phase 1 stores *raw* sessions.
- **P0-9 (frontmatter schema)** — done; it gates the *Phase 2 markdown writer*, not Phase 1
  storage. `Persist raw session` and the filename scheme write raw transcripts + timestamps, no
  frontmatter.
- **P0-8 (CI)** — live on every PR to `main` before any Phase 1 code lands.

**Blocks Phase 1 start:** *nothing.*

**Can proceed in parallel (non-blocking):**

- Reserve crates.io / npm / domain names (backlog, `chore/reserve-namespaces`) — anti-squatting
  insurance; the bare `kodama` crate name is likely taken and needs a naming decision.
- Everything in §3 below was cheap enough to fix inside this audit rather than ticket.

### Ticket ordering

The 17 Phase 1 tickets are well-sequenced, with two corrections (annotated directly on the
backlog items):

1. **`Adopt timestamp + device-ID filename scheme` (size-S) before `Persist raw session`
   (size-M)** — the scheme defines the filenames persistence writes.
2. **Two independent tracks can run in parallel:** the capture track (loopback → mic →
   interleave → hotkey/tray → listening indicator) and the transcription track
   (`TranscriptionEngine` trait → Parakeet → whisper.cpp → Silero VAD). They only converge at
   the benchmark pair (`Record a real meeting` needs the capture pipeline + persistence;
   `Benchmark both engines` needs that fixture + both engines). The glossary post-pass is
   engine-agnostic and can land before the benchmark. Resource-budget tuning goes last.

No ticket needed splitting; sizes look right.

---

## 3. Fixes applied by this audit

- **Moved `FRONTMATTER_SCHEMA.md` and `MCP_TOOL_SURFACE.md` to `docs/`** — both merged from
  branches in flight while #13 consolidated docs, so they landed at root. Zero inbound links
  existed; nothing broke.
- **Fixed the stale provenance note** in `docs/MCP_TOOL_SURFACE.md` (see §1).
- **Reconciled the two specs: added the stable `id` field to `docs/FRONTMATTER_SCHEMA.md`.**
  P0-10 merged *after* P0-9 with an explicit "Recommendation to P0-9" — a stable `n_…` note id,
  "the invariant the entire tool surface depends on as its write handle" — that was never
  absorbed. The schema now carries `id` (first in canonical key order, `^n_[0-9a-z]{6,}$`,
  never rewritten on move); the MCP spec's recommendation section is marked **Adopted**.
- **Pared `docs/FOUNDING_DOC.md` §6** so the roadmap has one home: phase sections now carry only
  goal + milestone + pointers (`ROADMAP.md` holds the Phase 2–4 checklists; the board holds
  Phases 0–1; Phase 5 detail stays in the founding doc, which `ROADMAP.md` §5 references by
  name). Ticked the completed Phase 0 items, closed the License and Frontend-stack rows in §7
  (both decided), and defined the previously "undefined" Phase 4 milestone.
- **`CLAUDE.md`:** documented that the Executing column is literally named **"Write Me Code"**
  on the board (a `kangentic_move_task` to "Executing" would fail), and added an
  **Engineering rules** section (see §5).
- **Backlog hygiene:** every Phase 1 item now carries a suggested `type/slug` branch name — set
  it as `branchName` at promotion — plus dependency/ordering notes. This defuses the known trap
  where a task promoted without `branchName` gets a locked, non-conforming auto-generated name.

## 4. Remaining board hygiene (informational, no action needed)

- Three archived tasks (#3, #11, #12) predate the branch policy and carry non-conforming branch
  names (`build-moodboard-and-8e676ff5`, …). Historical; renaming would break PR tracking.
- Archived task #11 has no labels. Cosmetic.
- The committed `kangentic.json` names the first column "Backlog" while the live board calls it
  "Ready". Harmless drift; worth syncing next time the board config is touched.

---

## 5. Claude Code rules & skills for Phase 1

### Rules — built now (added to `CLAUDE.md` § Engineering rules)

| Rule | Rationale |
| --- | --- |
| Pre-commit gates mirror CI exactly (`fmt --check`, clippy `-D warnings`, `cargo test`, eslint, `pnpm build`) | Phase 1 is the first phase with real Rust churn; catching CI failures locally keeps the board's Code Review stage about substance |
| Core vs shell: logic in `crates/kodama-core`, `src-tauri` commands stay thin | The founding doc's "dumb, testable data layer" made enforceable at the rule level before the first capture ticket writes code |
| Design tokens: never hard-code color/type/spacing; consume `design/tokens.css` via `src/index.css` | The listening-indicator ticket is the first UI work; this is `DESIGN.md`'s "never duplicate" rule where agents will actually see it |
| Spec agreement: `FRONTMATTER_SCHEMA.md` ↔ `MCP_TOOL_SURFACE.md` stay in sync | The exact failure this audit had to repair (the unabsorbed `id` field), prevented from recurring |
| Branch/commit policy | Already in `CLAUDE.md` and demonstrably followed — no change needed |

### Skills

| Skill | Trigger | Automates | When |
| --- | --- | --- | --- |
| `preview` (`.claude/skills/preview`) | "preview the app", verifying any runtime-touching change from a task worktree | Launch `pnpm tauri dev` from the current tree, health checklist, smoke-test the changed flow, clean shutdown | **Built now** — every Phase 1 capture ticket needs this loop; the generic `/run` and `/verify` skills pick it up as the project skill |
| `transcription-benchmark` | The `Benchmark both engines` ticket | Run both engines over the recorded fixture; score proper-noun accuracy, silence behavior, speed; emit a comparison | **Build later** — needs both engines + the fixture to exist; noted on that backlog item so it isn't forgotten |
| Frontmatter validator | Phase 2 markdown writer | Validate emitted notes against `docs/FRONTMATTER_SCHEMA.md` | **Build later** — added as a Phase 2 line in `docs/ROADMAP.md`; the writer is its first real consumer |
| "New crate module" scaffolder | — | — | **Skipped** — Rust modules are cheap by hand; revisit only if Phase 1 shows real repetition |

## 6. Follow-up tickets

Only one gap needed a new ticket; everything else was fixed directly (§3) or annotated onto
existing backlog items:

1. **Reserve names: crates.io / npm / domain variant** — created in the backlog, suggested
   branch `chore/reserve-namespaces`, labels `phase-0, infra, size-S`. Parallel to Phase 1.
