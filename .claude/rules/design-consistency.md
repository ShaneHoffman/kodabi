---
paths:
  - src/**
---

# Design consistency

Grove's doctrine is [`docs/DESIGN_SYSTEM.md`](../../docs/DESIGN_SYSTEM.md); its mechanics are
[`docs/UI_CONVENTIONS.md`](../../docs/UI_CONVENTIONS.md). Four of their claims are machine-enforced,
as `no-restricted-syntax` selectors in `eslint.config.js`: the colour-literal guard, the
`.css`-import ban and the reduced-motion partner guard (the three it calls the Grove guards), plus
the em-dash guard (the copy guard). (The effect ban in the same file is
[`no-use-effect`](no-use-effect.md)'s, not a design claim.) Everything else the design system says is review's job, and it says so itself
(DESIGN_SYSTEM §7): *"this document is the checklist"*, and **"the absence of a guard is not
permission."**

This file is that checklist in the form review reads it: the questions to ask of a diff that
touches `src/**` UI, each naming the section that owns the answer. Open the docs — they are the
authority, and this list deliberately does not restate them.

**Two bullets below are now half-mechanical, and the half that is left is the harder half.** A guard
can see that a movement is accompanied and that a sentence has no em dash. It cannot see whether the
partner is the right one, whether it survives specificity, or whether the sentence is worth reading.
Where a bullet says a guard holds something, read it as *that part is no longer worth your attention*
— not as *this bullet is handled.*

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
  is reading. **Eslint now holds the bare presence of a partner** for the `animate-*` rows, in the
  same class string — that is the check that would have caught `animate-breathe` on a placeholder
  dot. It holds nothing else here, and the rest is where the real failures are: whether the partner
  is the *correct* one for that animation, the two rows a class-string guard structurally cannot read
  (`active:scale-97` and the switch knob's `translate`), the loops applied from `src/index.css`
  rather than a className, and above all that the swap **must repeat every guard the thing it swaps
  carries** or it loses on specificity and goes silently dead (UI_CONVENTIONS §3, which also holds
  the `transition-[scale]`-not-`transition-transform` trap). A green lint means the swap is written,
  never that it works: those failures are invisible on screen, so check the built CSS.
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
  code comments or repo docs. **Eslint holds the em dash itself** over `src/` strings and JSX text
  (tests and the dev gallery exempt, comments unreachable), so what is left for review is everything
  copy-style does not spell as a character: whether an error names what failed and what happens next,
  whether the register is right, whether a label reads as an apology. Two register claims from
  DESIGN_SYSTEM §6 are checkable in the same pass: `ink-faint` is a metadata register, so the moment
  it carries a sentence the user has to read it is the wrong token; and a hue never colours running
  text.

**The rest of the doctrine is not restated here and is not thereby optional.** Green's closed list of
meanings, rectangles versus pills, the one press spec, the nine glass thicknesses, the radius ladder,
the measured contrast floor, the `.hc` three-token budget, focus order and live regions: when a diff
touches any of those, open [`docs/DESIGN_SYSTEM.md`](../../docs/DESIGN_SYSTEM.md).

## What stays review-only, and why

Four claims are mechanized because each is a fact about a *string*: a colour literal, an import
path, a token beside its partner, a character. Everything below is a judgment about what the code
does when it runs, or about what a person reading the screen understands. No selector reaches any of
it, and writing one that half-reached it would be worse than none — a guard that passes is read as
permission (DESIGN_SYSTEM §7), which is exactly how a design system ends up with green CI and a
screen nobody checked.

- **View states** (bullet 1). Whether a view answers all four questions is a claim about branches
  that exist and what they render, not about classNames. An empty state can be present and still be
  an apology; a `role` can be correct and the message useless.
- **The focus ring and focus order** (bullet 3). `focus-ring` is greppable, but the finding is
  usually its *absence* alongside an `outline-hidden`, on some other element, or an order that reads
  wrong to a keyboard — none of which is a property of one class string.
- **Composition and the six slots** (bullets 4 and 5). Which slot an action belongs in follows from
  what the action acts on; whether a primitive should have been composed rather than written follows
  from what the primitive already does. Both are intent, and intent is not in the AST.
- **The remainder each mechanized bullet leaves**, named in the bullets above: the right partner
  rather than a partner, the specificity that decides whether it works, the rows and stylesheet
  loops the guard cannot see, and copy that is clean of em dashes and still wrong.

If one of these later turns out to be a string fact after all, mechanize it — that is exactly the
move that produced the motion guard, and the `animate-breathe` bug is the argument for making it
again. Until then, absence of a guard is not permission.

Enforcement is the four eslint guards, plus the four tests DESIGN_SYSTEM §7 names
(`src/theme.test.ts` and `src/contrast.test.ts` pin the two variant classes;
`src/motionGuardParity.test.ts` pins the motion guard's token list to §4's swap table;
`src/components/dev/PrimitiveGallery.test.tsx` renders every Grove control under all four grounds),
plus this rule read at the Code Review stage
([`code-review-fix`](../skills/code-review-fix/SKILL.md) step 2, for every diff touching `src/**`).
Nothing above is scanned in CI beyond those.
