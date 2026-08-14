---
paths:
  - src/**
---

# Design consistency

Grove's doctrine is [`docs/DESIGN_SYSTEM.md`](../../docs/DESIGN_SYSTEM.md); its mechanics are
[`docs/UI_CONVENTIONS.md`](../../docs/UI_CONVENTIONS.md). Exactly two of their claims are
machine-enforced — the colour-literal guard and the `.css`-import ban, the two
`no-restricted-syntax` selectors `eslint.config.js` calls the Grove guards. (The effect ban in the
same file is [`no-use-effect`](no-use-effect.md)'s, not a design claim.) Everything else the design
system says is review's job, and it says so itself (DESIGN_SYSTEM §7): *"this document is the
checklist"*, and **"the absence of a guard is not permission."**

This file is that checklist in the form review reads it: the questions to ask of a diff that
touches `src/**` UI, each naming the section that owns the answer. Open the docs — they are the
authority, and this list deliberately does not restate them.

- **Every view answers four questions, not one.** Loading, empty, error, success
  (DESIGN_SYSTEM §3). A view that only renders its content is unfinished. Loading normally renders
  *nothing* — a skeleton flash is the finding, not its absence; the exception is work with a real
  duration (transcription, distillation), which gets the pipeline placeholder. Empty is first-run
  copy, not an apology. Error names what failed and what happens next, never leaks an exception
  string, and leaves the data reachable. Success is the absence of noise, plus a toast where the
  thing left the screen. `StatusMessage` is the one way a view says nothing/failed/working, and its
  variant fixes the ARIA role (UI_CONVENTIONS §4) — a hand-rolled state block with its own `role` is
  a finding.
- **Every movement carries its reduced-motion partner, at the call site.** The swap table is
  DESIGN_SYSTEM §4: `materialize` / `rise-in` → `fade-in`, `dissolve` → `fade-out`, `halo` / `ring`
  → `halo-still`, `breathe` / `drift` / `drift-back` → nothing, `active:scale-97` → no press
  transform, a switch knob's `translate` → `duration-0`. Movement is the accessibility problem; life
  is not, so opacity-only animations (`animate-caret`, `animate-pending`) are correct unpaired, and a
  duration is gated instead of swapped *exactly* when the end state, not the travel, is what the user
  is reading. The swap is written in the className because that makes it visible in review, and
  it **must repeat every guard the thing it swaps carries** or it loses on specificity and goes
  silently dead (UI_CONVENTIONS §3, which also holds the
  `transition-[scale]`-not-`transition-transform` trap). Both failures are invisible on screen:
  check the built CSS.
- **One focus ring, and removing it needs a replacement.** `focus-ring`, or `focus-ring-inset` where
  the focusable thing fills its container (DESIGN_SYSTEM §2). Never a colour change alone.
  `outline-hidden` with nothing in its place is a finding; so is a ring that lives on a different
  element from the hover it belongs with. The one argued exception is `Field`, where the bordered row
  is the control and `focus-within` moves the whole surface's border (UI_CONVENTIONS §4) — a new
  exception argues its case the same way, in a comment, or it is a finding.
- **Compose from the primitives before writing a new one.** `src/components/ui/` holds the catalogue
  and UI_CONVENTIONS §4 holds the contracts a restyle must preserve (`loading` / `busy` is never
  `disabled`; `Field` takes `error`; `Menu.Trigger` composes via `render`; `ViewFrame`'s variant
  discriminates its props; `Dialog`'s centring stays margin-based). There is no `Textarea`,
  `ListRow`, or `PlaceholderView`, and a live reference to one is itself a bug. **A caller passes
  layout, never geometry, type, colour, `transition-[…]` or `duration-*`** — there is no
  `tailwind-merge`, so a restated property is decided by build order rather than by the className,
  and the failure is a silently ignored instruction rather than an error. Pick the recipe from the
  state with a ternary; add a variant rather than a call-site override.
- **A view's actions sit in one of six slots, chosen by what the action acts on** — frame header,
  view-owned header, contextual chrome, row affordance, footer/composer, disclosure — never by where
  there was room, and each with its own ceiling (UI_CONVENTIONS §5). Four kinds of control sit
  outside the list on purpose, including one a *view state* raised: that belongs to the state block,
  whose vocabulary is the first bullet above. There is no inspector, no split, and no third rail — a
  view that needs more room takes depth, not width.
- **Spacing is Tailwind's numeric scale on the 4px grid; the number is the name**
  (UI_CONVENTIONS §2). The pre-Grove named steps (`px-xs`, `py-2xs`, `gap-sm`) are **retired**, along
  with the eslint rule that enforced them, so a named step reappearing is a finding — and so is a new
  alias layer proposing to bring them back. Arbitrary values are the sanctioned spelling where the
  design has a reason for the value (`text-[13px]`, `max-w-[66ch]`); the one thing they may not carry
  is a colour, which is the guard eslint does hold. A view's head is one shape for every view that
  draws one, and `ViewFrame` draws it: pass `eyebrow` / `title` / `summary`, never the classes.
- **Copy the user reads follows [`copy-style`](copy-style.md)** — no em dashes, and note that rule's
  scope: UI strings, labels, captions, tray text, and user-visible error and status messages, but not
  code comments or repo docs. Two register claims from DESIGN_SYSTEM §6 are checkable in the same
  pass: `ink-faint` is a metadata register, so the moment it carries a sentence the user has to read
  it is the wrong token; and a hue never colours running text.

**The rest of the doctrine is not restated here and is not thereby optional.** Green's closed list of
meanings, rectangles versus pills, the one press spec, the nine glass thicknesses, the radius ladder,
the measured contrast floor, the `.hc` three-token budget, focus order and live regions: when a diff
touches any of those, open [`docs/DESIGN_SYSTEM.md`](../../docs/DESIGN_SYSTEM.md).

Enforcement is the two Grove guards, plus the three tests DESIGN_SYSTEM §7 names
(`src/theme.test.ts` and `src/contrast.test.ts` pin the two variant classes;
`src/components/dev/PrimitiveGallery.test.tsx` renders every Grove control under all four grounds),
plus this rule read at the Code Review stage
([`code-review-fix`](../skills/code-review-fix/SKILL.md) step 2, for every diff touching `src/**`).
Nothing above is scanned in CI beyond those.
