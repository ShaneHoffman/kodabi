# Kodabi — UI conventions (how to spell it)

*Status: Living (Phase 4, the Grove redesign). Rewritten when Grove replaced the pre-Grove system.*

Three documents describe the look, and they divide cleanly:

| Document | Fixes |
| --- | --- |
| [`docs/DESIGN.md`](DESIGN.md) | The **aesthetic** — the four principles, the reference class, what we refuse |
| [`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) | The **doctrine** — what colour means, what shape means, what may move, what must be legible |
| **This document** | The **mechanics** — how to write it down, and where a control goes |

The material all three describe is the `@theme` block in [`src/index.css`](../src/index.css).

DESIGN_SYSTEM decides *what a thing should be*. This document decides *how you type it*.

---

## 1. The base is Tailwind, and the exception is CSS

**Components are styled with utility classes.** Variants come from `cva`, conditionals from `clsx`.
There is one stylesheet, [`src/index.css`](../src/index.css), and it holds the `@theme` tokens, the
keyframes, the `.day` / `.hc` blocks, and a short list of things utilities genuinely cannot express.

Plain CSS is permitted where a utility honestly cannot do the job — a multi-stop gradient composited
over a background colour, a complex pseudo-element composition, a vendor scrollbar. **Such an
exception lives in the entry file with a comment saying why.** CSS is the deliberate exception, never
the habit.

Enforced: eslint fails a `.css` import outside `src/index.css` unless it carries a disable comment
justifying it, and fails a colour literal in a `className`. See DESIGN_SYSTEM §7 for what those two
guards do and do not catch.

### Reach for `@utility`, not a class

When something really does need CSS but is used at many call sites, write it as a Tailwind
`@utility` rather than a bare class. It stays a real utility (composable, variant-able, dropped from
the build when unused) instead of becoming a component class that competes with the utility system.
`grove-ground` is the worked example.

---

## 2. Spacing

**Tailwind's numeric scale, on the 4px grid.** `p-2` is 8px, `gap-4` is 16px, `px-5` is 20px. There
is no named-step system in Grove and no alias layer: the number *is* the name.

The named-steps rule that governed the pre-Grove app (`px-xs`, `py-2xs`, `gap-sm`) is **retired**, and
so is the eslint rule that enforced it. It existed to stop a second spacing vocabulary appearing
alongside the tokens; with one stylesheet and one grid there is no second vocabulary to prevent.

**Arbitrary values are allowed** — `text-[13px]`, `max-w-[66ch]`, `backdrop-blur-[26px]`. The
prototype's type sizes land on half-pixels and its measures are in `ch`; forcing those onto a
rounded scale would change the design to suit the tooling. Prefer the scale where the scale fits, and
reach for a bracket when the design has an actual reason for the value.

The one thing arbitrary values may **not** carry is a colour (DESIGN_SYSTEM §7).

### The rhythm that is worth being consistent about

| Gap | Between |
| --- | --- |
| `gap-1.5` / `gap-2` | Items inside one control (icon and label, dot and name) |
| `gap-2.5` / `gap-3` | Sibling controls in a cluster |
| `gap-3.5` | Cards in a list |
| `gap-5` | The dock and the main panel |
| `p-5` | The body's frame around dock + panel |
| `px-10 py-8` | Inside the main panel |

These are the prototype's numbers, not a law. They are written down so a new screen starts from the
same rhythm rather than re-deriving one.

---

## 3. Colour, type, radius, motion

Always the token utility, never the literal:

| Want | Write |
| --- | --- |
| The page ground | `bg-ground`, or `grove-ground` for the ground with its glows |
| Ink | `text-ink`, `text-ink-read`, `text-ink-dim`, `text-ink-faint` |
| A boundary | `border-edge`; the inset highlight along a raised surface's top is `--color-edge-lit`, read from a `shadow-[inset_0_1px_0_var(--color-edge-lit)]` |
| The kodama, a match, the caret | `text-kodama`, `bg-kodama`, `caret-kodama`, `text-kodama-ink` |
| Failure | `text-warn`, `text-danger` |
| A project | `text-coral`, `bg-cobalt`, `border-teal`, `text-plum` |
| A face | `font-ui`, `font-data`, `font-note` |
| Reading size | `text-note` (carries its own leading) |
| A radius | `rounded-panel`, `rounded-card`, `rounded-dialog`, `rounded-button`, `rounded-pill` |
| A curve | `ease-out-strong`, `ease-in-out-strong` |
| A duration | `duration-140`, `duration-220`, … (bare ms; the canonical four are in DESIGN_SYSTEM §4) |

Folder hues are chosen by data, not by markup, so they arrive as a lookup from the project rather
than as a literal class — `PROJECT_HUE[project]` returning `"text-coral"`, never a computed
`` `text-${hue}` `` (Tailwind cannot see a constructed class name and will not emit it).

### The two variants

`.day` and `.hc` are root classes, set imperatively by [`src/theme.ts`](../src/theme.ts) and
[`src/contrast.ts`](../src/contrast.ts) — one-time document bootstrap, which
[`.claude/rules/no-use-effect.md`](../.claude/rules/no-use-effect.md) names as explicitly not an
effect.

**Almost nothing should need a `day:` or `hc:` variant in a className.** Both are token remaps: if a
component looks right at night, it looks right in day because its tokens moved. Reach for the variant
only where the *alpha* genuinely differs — night lightens a surface with white, day darkens it with
ink, and no single token can hold both. A `day:` in a className is a claim that no token could have
carried it, and it should read as one.

### Motion, at the call site

Animations are `animate-*` utilities, and the reduced-motion swap is written beside them:

```tsx
className="animate-materialize motion-reduce:animate-fade-in"
className="animate-rise-in motion-reduce:animate-fade-in [animation-delay:45ms]"
className="transition-transform duration-140 ease-out-strong active:scale-97 motion-reduce:active:scale-100"
```

That the swap is visible in the markup is the point (DESIGN_SYSTEM §4).

---

## 4. Primitives

`src/components/ui/` holds the shared controls. **Their styling is pre-Grove**, and six of the eight
still carry their own stylesheet (`StatusMessage` and `DestructiveConfirmDialog` never had one — they
compose other primitives and utilities); the primitives' Grove ticket restyles them. **Their
behaviour is not pre-Grove** — the contracts below are live, they are what the components actually
promise, and a restyle must preserve every one of them.

| Primitive | Is | Variants |
| --- | --- | --- |
| `Button` | The action rectangle | `primary`, `quiet`, `filled`, `destructive` |
| `TextField` | A labelled input | — |
| `Checkbox` | A box and its label | — |
| `Select` | A hand-rolled combobox (full listbox, no headless library) | `boxed`, `token` |
| `Overlay` | The scrim, stacking layer, and raised panel behind modal surfaces | — |
| `ViewFrame` | A view's scaffold: gutter, column, header | `queue`, `library`, `panel`, `health`, `doc`, `search`, `terminal`, `chat` |
| `StatusMessage` | The one way a view says nothing/failed/working | `empty`, `error`, `status` |
| `DestructiveConfirmDialog` | The shared shape of a destructive confirmation | — |

### The contracts worth not breaking

- **`Button`'s `loading` is not `disabled`.** A busy control takes `aria-disabled` + `aria-busy`,
  stays focusable, swallows its own activation, and swaps only its label. The native `disabled`
  attribute blurs a focused button to `<body>` mid-task, which is the bug `loading` exists to avoid.
  `disabled` means "there is nothing here to do"; passing both puts the focus loss back, because
  `disabled` wins.
- **`Select`'s `busy` is the same distinction**, for the same reason, and `disabled` beats it the same
  way. Pass `busy` while a write is in flight, `disabled` only when there is nothing to choose.
- **`Select` is a real combobox.** Focus stays on the trigger; ↑/↓ move a virtual highlight via
  `aria-activedescendant`; Enter/Space selects; Escape closes and returns focus to the trigger;
  outside click closes; typing jumps. `hideLabel` keeps the accessible name and drops the visual row.
  `emptyLabel` is what the open list says when there is nothing in it.
- **`TextField` takes `error`, not a hand-rolled message.** The copy and `aria-invalid` have to travel
  together, and every hand-rolled version in the app set one without the other. `hint` is wired
  through `aria-describedby`.
- **`StatusMessage`'s variant fixes the ARIA role**: `error` → `role="alert"`, `status` →
  `role="status"`, `empty` → none. That binding is the whole point of the primitive.
- **`ViewFrame`'s `variant` is required and discriminates the props.** `summary` is a **type error**
  outside `queue` / `library` / `health`, and `action` is a **type error** on `doc` / `search` (the two
  that draw no header of their own) — neither is a silent no-op. `action` is one node and one action;
  a caller with two is in the wrong slot (§5).
- **`Overlay` dismisses on click, not pointerdown**, and only when the gesture both started and ended
  on the backdrop. It deliberately does **not** trap focus — each caller passes its own `onKeyDown`,
  because the palette swallows Tab on one input while the consent nudge wraps it across several.
- **`DestructiveConfirmDialog` is presentational and never closes itself.** The caller owns the async
  handler, `busy` / `error` state, and what success means.

### There is no `Textarea`, `ListRow`, or `PlaceholderView`

All three existed and nothing composed them; a live reference to any of them is itself a bug.

`ListRow` is the instructive one, and its lesson survives Grove intact: **a row is the view's stance.**
Inbox rows lift when touched (these are items you clear), project rows only tint (a reading room,
nothing is waiting on you), and an attention card arrives already raised with no hover at all (it
flagged itself). One affordance, three meanings — which is what a single shared row was flattening.
Each list-bearing view draws its own.

The rule that outlived it: **hover and the focus ring live on the same element.** `ListRow` recoloured
an inner span while the ring sat on the outer one, which is the exact failure its own doc comment
claimed to fix.

### The stack Grove builds on

The curated stack is installed and is the direction of travel:

| Package | For |
| --- | --- |
| `cva` (`class-variance-authority`) + `clsx` | Variants and conditional classes. Use these for every new component |
| `@base-ui/react` | Headless behaviour: menu, dialog, popover, tooltip |
| `cmdk` | The command palette |
| `sonner` | Toasts |
| `motion` | Motion that CSS cannot express — gestures, layout animation, interruptible transitions |

**`cva` and `clsx` are the two to reach for immediately.** The other four replace hand-rolled
behaviour, and each replacement is its own decision in its own ticket — installing a package is not
the same as adopting it. In particular, `Select` is a working, tested, accessible combobox: it gets
replaced when someone has read what `@base-ui/react` gives in exchange, not on sight.

This supersedes the zero-UI-dependency posture and
[`docs/decisions/popover-primitive.md`](decisions/popover-primitive.md), which held against base-ui
in 2026-07 on the strength of one primitive. Grove needs a menu, a dialog, a popover, a tooltip, a
palette, and a toaster; the arithmetic is different at six.

---

## 5. Composition — where a control goes

*Unchanged by Grove. This section is about structure, and Grove restyled surfaces without moving
where things live.*

### The shell has two regions, and a view fills one

`AppShell` renders the dock beside a single `<main>`, with the capture toast, the command palette and
the consent nudge overlaid on top rather than docked beside. `MainContent` is a flat switch and every
destination renders into that one main slot. **There is no inspector, no split, and no third rail.**
A view that needs more room takes depth, not width.

That is a decision, not an accident of what got built first. A region that some destinations have and
others don't makes the main column's width depend on where you navigated, which reads as the layout
being unstable rather than as two places being different kinds of place. The two candidates people
reach for are already answered by stacking: a note's metadata is one line under the title, and the
session artifacts sit under the body behind a single hairline — chrome below the document, not part
of it.

The note editor's details rail is not a counterexample. It is what the *leftover* width becomes once
prose has stopped at its measure (DESIGN_SYSTEM §1), inside the one main slot — not a second region
the shell provides.

**What would overturn it:** a majority of destinations wanting the *same* persistent secondary
content, which has to stay visible while the main column is being used. One line of metadata is not
that, and a recording you play once is not either.

### The six slots

**A view's actions sit in one of six places**, and the slot is chosen by what the action *acts on* —
never by where there happened to be room.

| Slot | Acts on | Ceiling |
| --- | --- | --- |
| **Frame header** — `ViewFrame`'s `action` | the view: the one thing you came here to do | **one** |
| **View-owned header** — the document and search views only | the open document, or the query | **one cluster** |
| **Contextual chrome** | whatever summoned it — a selection, a pending prompt | no count; *one job* |
| **Row affordance** | one item in a list | **two** |
| **Footer / composer** | the surface as a whole: commit it, or abandon it | **two** |
| **Disclosure** — the toggle that summons a subordinate section | content stacked below the view's subject | **one per section, and it does not nest** |

**Four kinds of control are deliberately outside that list.** Getting around is not acting — a back
link, and Settings' tab rail, which *filters* the pane rather than navigating (`role="tablist"`, so a
screen reader announces it as a filter and not as a second set of destinations competing with the
dock). Affordances *inside* the content belong to the content: a note's tag chips, a recording's
`<audio>` player. A control a **view state** raised belongs to that state and not to the frame,
whether it recovers or announces — Terminal's Restart, Chat's Start a new chat, the error boundary's
Try this screen again, and the Inbox's filed toast all sit inside a state block, and the vocabulary
for those is [`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §3. And the dock's New project button is
global, so it lives in the other region entirely.

---

## 6. Migrating a screen

Grove lands screen by screen. While that runs, two systems are live at once and both work.

**A migrated screen:**

- styles with Grove utilities and the tokens in §3
- deletes its own `Component.css` and the `eslint-disable` comment on its import
- uses `cva` for any variant it has
- writes its reduced-motion swap at the call site

**An unmigrated screen** keeps `design/tokens.css`, the `@theme inline` bridge at the bottom of
`src/index.css`, and its own stylesheet. It is frozen: fix bugs in it, but do not extend it, and do
not add a new token to `design/tokens.css`.

**Nothing new may consume the legacy layer.** A new component is Grove even if the screen around it
is not — the two vocabularies coexist in one tree without conflict, because the Grove tokens are
namespaced apart from the bridged ones (`--color-ground` vs `--color-bg`, `font-ui` vs `font-sans`).

The final cleanup ticket deletes `design/tokens.css`, the `@theme inline` block, `src/fonts.ts`, and
the three `@fontsource` dependencies once the last screen has moved. Grove's three faces ship with
Windows, so the finished app fetches no font.
