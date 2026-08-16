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
justifying it, and fails a colour literal in a `className`. The app's own code carries no such
disable — the only one left is xterm's third-party stylesheet, which is the shape a new exception
has to argue itself into. See DESIGN_SYSTEM §7 for what those two guards do and do not catch.

**Check the built CSS when a utility is load-bearing.** Several failures in this system are silent
in the markup and obvious in the output: a transition that names `transform` while the utility sets
the `scale` property, a reduced-motion swap that loses on specificity, an arbitrary property that
did not parse. `pnpm build`, then read `dist/assets/index-*.css` for the class you wrote.

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
| `px-10 py-[34px]` | Inside the main panel, owned by `ViewFrame` for every view at once |

These are the prototype's numbers, not a law. They are written down so a new screen starts from the
same rhythm rather than re-deriving one.

The panel's interior gutter shrank from 44/60 when the shell landed: the panel is now an inset pane
with 20px of ground around it, so the old pair stacked two gutters and pushed every view's first
line a third of the way down the glass. A screen that leaves `ViewFrame` for its own frame (ChatView,
the note editor) spells the same `px-10 py-[34px]` by hand, so the left edge holds still across a
navigation either way.

### A view's head

One shape for every view that draws one, so two screens cannot open at different volumes:

| Part | Written |
| --- | --- |
| Eyebrow | `font-data text-[10px] uppercase tracking-[0.22em] text-ink-faint` |
| Title | `text-[26px] font-semibold leading-[1.15] tracking-[-0.01em] text-ink` |
| Summary | `font-data text-[11px] text-ink-dim tabular-nums` |
| Together | Title and summary share a baseline: `flex items-baseline gap-4` |

`ViewFrame` draws it — pass `eyebrow` / `title` / `summary`, never the classes. The note editor is
the one exception, and it spells the same step by hand: its head is not a frame header but the
document's own first line, sitting beside Edit and Delete rather than above a list, and its body is
two columns `ViewFrame`'s single measure cannot hold.

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
| A control's fill | `bg-action-bg` / `bg-action-bg-hover` / `border-action-edge`, and the coral trio `bg-danger-bg` / `bg-danger-bg-hover` / `border-danger-edge` |
| The faintest fill | `bg-wash` (a quiet control's hover, a pill at rest, a menu row's highlight), `bg-wash-hover` |
| A switch's track once it is on | `bg-switch-on` (the resting track is `bg-wash-hover`; the knob's travel is the state, §6) |
| A face | `font-ui`, `font-data`, `font-note` |
| Reading size | `text-note` (carries its own leading) |
| A radius | `rounded-panel`, `rounded-card`, `rounded-dialog`, `rounded-button`, `rounded-pill` |
| A curve | `ease-out-strong`, `ease-in-out-strong` |
| A duration | `duration-140`, `duration-220`, … (bare ms; the canonical four are in DESIGN_SYSTEM §4) |
| A glass surface | `glass-top`, `glass-dock`, `glass-panel`, `glass-card`, `glass-overlay`, `glass-dialog`, `glass-palette`, `glass-pill`, `glass-sheet`, `glass-scrim` |
| A card that lifts under the pointer | `hover:-translate-y-[2px] hover:glass-card-lift` (DESIGN_SYSTEM §5) |
| A well sunk into a panel | `glass-term` (the terminal's pane; DESIGN_SYSTEM §5) |
| A row that enters or leaves a working list | the `motion` variant tables in [`src/components/views/InboxView.tsx`](../src/components/views/InboxView.tsx) — a slot that collapses its own height, a card that travels; see "When motion, when CSS" below |
| The focus ring | `focus-ring`, or `focus-ring-inset` where the control fills its container |

Each recipe carries a whole surface — its fill, blur, lit edge, border, shadow and rung of the radius
ladder — plus its own `.day` branch, so a surface cannot be spelled at the wrong roundness or lose
half its material. The nine thicknesses and `glass-term` each carry a
`prefers-reduced-transparency` branch too; `glass-scrim` is the deliberate exception on both counts
(a fill and nothing else, and no blur to drop), and DESIGN_SYSTEM §5 says which parts each one omits
and why. They are `@utility` rather than
a stack of classes because reduced transparency removes a *property* (`backdrop-filter`) rather than
remapping a value, which no token and no variant can express. Add layout at the call site, not
material: `glass-card p-4`, never `glass-card bg-*`.

Folder hues are chosen by data, not by markup, so they arrive as a lookup from the project rather
than as a literal class — `PROJECT_HUE[project]` returning `"text-coral"`, never a computed
`` `text-${hue}` `` (Tailwind cannot see a constructed class name and will not emit it).

### Text in a control sets `leading-none`, and centring it is two steps

A control whose height should be predictable — a button, a pill, a menu row, a palette row — sets
`leading-none` on its text. Without it the text inherits a 1.5 line-height, so an 11.5px label
occupies a 17.25px box and *that* becomes the control's height: fractional, and dependent on which
glyphs it happens to hold. `Button`, `Menu`, `CommandPalette` and both capture pills all spell it.
Reading surfaces are the opposite case and take an explicit ratio instead (`leading-[1.55]`,
`leading-[1.65]`, or `text-note`, which carries its own).

`leading-none` is not, on its own, a centring fix — this is the part worth knowing before chasing a
label that looks a pixel high. Flex `items-center` centres the *box*, and it does that exactly; but a
font's ascent and descent are not symmetric about its cap band, so a perfectly centred box still
renders text that sits high or low. The half-leading either side of a line box is symmetric, which is
why changing line-height barely moves the glyphs relative to their own box. The correction is a
static `translate-y-px`, which is layout-inert and so cannot re-bias the `items-center` that placed
the box. Two rules for reaching for it:

- **Measure, never eyeball.** [`ListenPill`](../src/components/shell/ListenPill.tsx)'s CENTRING note
  records the method (render against the built stylesheet in headless Edge at DPR 1, then compare the
  ink's mass centroid to the pill's centreline) and the numbers it produced.
- **Only whole pixels exist.** Chromium snaps a text translation to whole device pixels, so `0.5px`,
  `0.75px` and `1px` all rasterise identically. Write the integer.

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
className="transition-[scale] duration-140 ease-out-strong active:scale-97 motion-reduce:active:scale-100"
```

That the swap is visible in the markup is the point (DESIGN_SYSTEM §4).

**`transition-[scale]`, not `transition-transform`.** Tailwind v4's `scale-*` sets the standalone
`scale` property rather than a transform function, so a transition naming `transform` animates
nothing and the press lands as a snap. The failure is silent — check the built CSS, not the screen.

**The swap must carry every guard the thing it swaps carries.** The snippet above works because both
halves weigh the same: `.x:active` against `.x:active`, decided by order, and the redefined
`motion-reduce` sorts last. Add a guard to one side only and the swap goes dead —
`not-disabled:not-aria-disabled:active:scale-97` weighs (0,4,0), because `:not()` takes its
argument's specificity, and a bare `motion-reduce:active:scale-100` at (0,2,0) loses on specificity
whatever the order. Repeat the guards: `motion-reduce:not-disabled:not-aria-disabled:active:scale-100`
(`Button` is the live example). Same failure mode as the one above, same check — read the built CSS.

`hover:` and `motion-reduce:` are **redefined** in `src/index.css`, so both mean more than Tailwind's
defaults everywhere they appear. `hover:` also requires `(pointer: fine)`, because a touch device can
satisfy `(hover: hover)` and then strand a tapped control in its hover state. `motion-reduce:` also
matches `[data-reduce-motion="on"]`, the root attribute the Settings switch writes — without it the
in-app toggle would be inert on every Grove primitive, which is all of them.

### When motion, when CSS

**CSS is the default and `motion` is the exception**, and the exception has a shape: reach for the
package when the movement needs something a transition cannot express — a height nobody measured, an
element that has to survive its own removal, or several movements on one interruptible timeline.
Presses, hovers, crossfades, and surfaces materializing are transitions, and they stay transitions
even on a screen that also uses `motion` for something else. The Inbox is the live example: its list
choreography is `motion`, and the stage crossfade three lines away is still a `transition-[opacity,filter]`.

Two things to know before writing any:

**Variant tables are module constants, never built during render.** A variants object is part of a
`motion` element's identity, so handing it a structurally identical but freshly-allocated one each
render reads as a change and re-reads the whole table. On a list that re-renders on every refetch
that is a measurable cost. Write one frozen constant per reduced-motion setting and pick between them.

**A `filter` belongs only in the state that uses it.** A resting variant carrying `blur(0px)` leaves a
filter in the element's inline style forever, and a filtered element is its own stacking context and a
containing block for fixed-position descendants — which quietly re-anchors any menu, tooltip or dialog
inside it. Motion reads the computed value as the start of the animation anyway.

**Reduced motion needs our hook, not the package's.** `motion`'s own `useReducedMotion()` reads
`prefers-reduced-motion` and nothing else, so it cannot see `[data-reduce-motion="on"]` — the one place
the preference is expressed *in-app* is the one place it would be ignored. Read
[`useReduceMotion`](../src/useReduceMotion.ts) instead, which unions both channels; `AppShell` feeds it
to a `MotionConfig reducedMotion` so every call site inherits the policy. Note what that setting does
and does not cover: it drops transforms and layout animations and keeps opacity — exactly the doctrine
— but it does **not** turn off an animated `height` or `filter`, so a component animating either must
also branch on the hook itself.

Tests run with `MotionGlobalConfig.skipAnimations` on (`src/test/setup.ts`), which lands every value at
its target immediately. An element `AnimatePresence` is holding open for an exit still unmounts a frame
later, so a test asserting it is gone has to drive the frame loop — see `waitForRemoval` in
`InboxView.test.tsx`.

---

## 4. Primitives

`src/components/ui/` holds the shared controls. **Every one of them is Grove**, styled with
utilities in the component; none carries a stylesheet. The contracts below are what the components
actually promise, and a restyle must preserve every one of them.

| Primitive | Is | Variants |
| --- | --- | --- |
| `Button` | Every pressable thing | `action`, `danger`, `quiet`, `pill`, `chip` |
| `Menu` | An anchored menu (base-ui). `Menu.Item` takes the variant | `default`, `suggested`, `foot` |
| `Dialog` | A modal: scrim, glass panel, focus trap (base-ui) | — |
| `Field` | A labelled input in a glass row | — |
| `Switch` | A boolean that takes effect on press; the knob's travel is the state | — |
| `Checkbox` | A box and its label | — |
| `Select` | A hand-rolled combobox (full listbox, no headless library) | — |
| `ViewFrame` | A view's scaffold: gutter, column, header | `queue`, `library`, `panel`, `health`, `doc`, `search`, `terminal` |
| `StatusMessage` | The one way a view says nothing/failed/working | `empty`, `error`, `status` |
| `DestructiveConfirmDialog` | The shared shape of a destructive confirmation | — |

`Button`'s five variants are three shapes: `action`, `danger` and `quiet` are the same rectangle
(`rounded-button`, 8x16) so a rail of them lines up; `pill` is the token shape for a thing you
open rather than a verb you perform (DESIGN_SYSTEM §2); and `chip` is the smaller 10px rectangle for
a control that sits *inside* content rather than in the chrome around it — the source panel's
Reveal and its audio toggle (`SessionPanel.tsx`). **The component owns its whole box** — the
pre-Grove `quiet` deferred padding to each consumer's stylesheet, and that is how the rails stopped
agreeing. A caller passes layout (`w-full`, `self-start`), not geometry, and never a `text-*` size or
colour: there is no `tailwind-merge`, so a call site that restates a property the primitive owns is
decided by build order rather than by the className.

`Menu.Item`'s three variants are that rule paying out. The Inbox's File menu needs two rows that
differ from the rest — the suggested destination, held lit while the menu is open, and the `New
project…` foot — and both were first written as a `className` at the call site. Neither worked:
`text-[13px]` is emitted after `text-[11.5px]`, so the foot stayed at the row size, and `text-ink-dim`
is emitted after `text-ink`, so the suggestion rendered exactly as dim as the rows it was supposed to
stand out from. **This is what the failure looks like — not a build error, not a visibly broken
screen, just an instruction that quietly did not happen.** As variants there is one size utility and
one colour utility on the element and nothing to resolve; `Menu.test.tsx` pins that count.

**It is not a `text-*` problem — it is every property, and motion is where it bites hardest.**
`transition-[…]` and `duration-*` are each one CSS property too, so stacking an element's hover
transition, its entrance and its exit "so all three are ready" gives every leg the longest property
list and the longest duration. Nothing errors; two of the three intentions are simply gone, along
with the exit band (DESIGN_SYSTEM §4) they were spelling out. Pick the recipe from the state with a
ternary rather than layering overrides — a transition is read from the after-change style, so
applying it in the same commit as the leaving values still animates. `InboxView.test.tsx` counts
those two utilities on a row exactly as `Menu.test.tsx` counts these.

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
- **`Switch`'s `busy` is that distinction a third time**, and it is the only inert form it has: a
  switch with nothing to switch is a row that should not be on the screen, so there is no `disabled`
  prop to pass. Its `label` is the accessible name and must be the words printed beside it, verbatim.
  **The knob's travel is the state readout, so it is gated by duration alone** — under reduced motion
  it arrives at once rather than not arriving. No fill quiet enough to sit on a card clears 3:1, which
  is why the position has to carry it (§6 of the design system).
- **`Field` takes `error`, not a hand-rolled message.** The copy and `aria-invalid` have to travel
  together, and every hand-rolled version in the app set one without the other. `hint` is wired
  through `aria-describedby` too, and the error is described first, so it is heard before the hint
  the value just contradicted. The bordered box is the ROW, not the input: the input inside is
  transparent and outline-free so `focus-within` can move the whole surface's border, which is why
  this is the one interactive primitive with no `focus-ring`.
- **`StatusMessage`'s variant fixes the ARIA role**: `error` → `role="alert"`, `status` →
  `role="status"`, `empty` → none. That binding is the whole point of the primitive.
- **`ViewFrame`'s `variant` is required and discriminates the props.** `summary` is a **type error**
  outside `queue` / `library` / `health`, and `action` is a **type error** on `doc` / `search` (the two
  that draw no header of their own) — neither is a silent no-op. `action` is one node and one action;
  a caller with two is in the wrong slot (§5). **`label` is how a composed title still names the
  region:** the frame labels its `<section>` from `title`, but only when `title` is a plain string
  ("[object Object]" being worse than silence), so a view that puts anything beside its name — the
  folder-hue dot on a project — passes `label` too or ships an unnamed landmark. A string title needs
  it not at all, and passing both is two sources for one name.
- **`Dialog` traps focus.** That is the whole reason it exists: base-ui owns the trap, Escape, the
  outside press, the scroll lock and the focus restore, where every caller of the pre-Grove
  `Overlay` it replaced hand-rolled a Tab strategy of its own. Pass `initialFocus` where the first
  tabbable control is the destructive one, so the dialog opens on the safe action. Its centering is
  `inset-0 m-auto h-fit` and must stay margin-based: the `materialize` keyframe animates
  `transform`, so a translate-centred popup opens in the corner.
- **`Menu.Trigger` composes, it does not wrap.** Pass the control through `render`
  (`<Menu.Trigger render={<Button variant="quiet">File</Button>} />`) so there is one `<button>`
  carrying both the Grove chrome and base-ui's wiring, not a button inside a button.
- **`DestructiveConfirmDialog` is presentational and never closes itself.** The caller owns the async
  handler, `busy` / `error` state, and what success means. Being mounted is being open: every caller
  renders it conditionally. Cancel holds initial focus; the confirm is the `danger` box: red belongs
  on the confirm inside a confirmation, never on the button that opens one. (The title bar's close
  button on hover is the one argued exception to red's scope, and DESIGN_SYSTEM §2 owns that
  argument — it is not a licence for a second one.)
- **Its copy is a structure, not prose.** Title asks the question ("Delete this note?"); `subject`
  names the thing in its own truncating strip; `children` is one short consequence line; the
  permanence warning is the dialog's OWN line in the danger tint, so no caller can forget it; the
  rail runs quiet Cancel then the destructive confirm, left to right, so nothing sits under the
  pointer on the way to the confirm. A field dialog follows the same shape and puts its tips in the
  `Field`'s `hint`, never in the body prose.

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

**Five are adopted.** `cva` gives `Button` its variants and `clsx` composes classes wherever a
primitive has conditions (`Field`, `Dialog`, `CommandPalette`); a component with no variants needs no
`cva` table. `@base-ui/react` is behind `Menu` and `Dialog` — the two pieces of behaviour that are
hard, invisible when they work, and not Grove's opinion: anchoring a popup to its trigger with a flip
at the window edge, and trapping focus in a modal. And `cmdk` is behind `CommandPalette`, which uses
**both**: cmdk owns the list (fuzzy scoring, the arrow keys, the listbox ARIA, one highlight shared by
pointer and keyboard) inside base-ui's dialog parts, which own the modal. Note what that rules out —
cmdk's own `Command.Dialog` wraps `@radix-ui/react-dialog`, and taking it would have put a second
dialog implementation in the app beside base-ui's. And `motion` drives the Inbox's capture flow — the
pipeline placeholder growing into the list, the routed card travelling out while its slot closes
behind it — which is the case CSS genuinely cannot hold: an unmeasured height, an element that has to
outlive its own removal, and both on one timeline that can be interrupted. **A library is worth
adopting for the part of it you need, not the whole surface it offers.** Everything visible about all
five is still ours.

**`sonner` is installed, not adopted**, and remains its own decision in its own
ticket — installing a package is not the same as adopting it. The Inbox's filed toast is the live
example of the distinction: it is hand-rolled, on Grove's own glass, and adopting `motion` for the
movement underneath it did not drag `sonner` in with it. The same holds for what base-ui has
NOT taken: `Select` is a working, tested, accessible combobox, and it gets replaced when someone has
read what `@base-ui/react` gives in exchange, not on sight. Its Grove ticket re-skinned it and left
every line of that behaviour alone, which is what the distinction looks like in practice: the list
now wears `Menu`'s material and `Menu`'s row recipe, and the combobox underneath is still ours.

This supersedes the zero-UI-dependency posture and
[`docs/decisions/popover-primitive.md`](decisions/popover-primitive.md), which held against base-ui
in 2026-07 on the strength of one primitive. Grove needs a menu, a dialog, a popover, a tooltip, a
palette, and a toaster; the arithmetic is different at six.

---

## 5. Composition — where a control goes

*Mostly unchanged by Grove. This section is about structure, and Grove restyled surfaces without
moving where things live. The one addition is the transport bar below.*

### The shell is a transport bar over two regions, and a view fills one

`AppShell` renders a transport bar above the dock and a single `<main>`, with everything else
overlaid on top rather than docked beside: the command palette, the consent nudge, the model-download
nudge, and a **notice corner** pinned bottom right that stacks the update notice above the capture
toast. That corner is one container rather than two independently-positioned overlays, because "rare"
is not "never simultaneous" and a failed capture can coincide with a waiting release; it is empty and
zero-size when neither has anything to say, so it never eats a click. `MainContent` is a flat switch
and every destination renders into that one main slot. **There is no inspector, no split, and no
third rail.** A view that needs more room takes depth, not width.

The transport bar holds what belongs to the WINDOW rather than to any view, and the list is closed:
the wordmark (which is also the way home), the listening pill, and a right-hand cluster of two
groups with a short hairline rule between them — the app's chrome (Commands, Settings, inside an
`App` nav landmark) and then, outside it, the window's own caption buttons (minimize,
maximize/restore, close). It is not a third region and no view draws into it.

**The caption buttons are there because the main window is undecorated.** This bar *is* the title
bar: it carries `data-tauri-drag-region` on its own element, so the background drags the window and
double-click maximizes it while every child keeps its own pointer behaviour, and the three buttons
Windows would have drawn are drawn here instead (`TopBar.tsx`). That is also why the shape no longer
says which controls belong to the app and which to the window — the five sit in one uniform, quiet
and square, so the rule between them has to carry the whole separation. Only one of the five breaks
that uniform, and only on hover: the close button's red is DESIGN_SYSTEM §2's one argued exception.

The listening pill lives here for one reason: it is the app's on-air surface, and it must never
move. Below, in a dock that grows, it shared a rail with destinations and sat under a list whose
length it depended on.

The dock therefore holds destinations only, in three groups: the vault-wide three (Inbox, Needs
attention, Search), the folders, and the two tools under a hairline (Chat, Terminal).

That is a decision, not an accident of what got built first. A region that some destinations have and
others don't makes the main column's width depend on where you navigated, which reads as the layout
being unstable rather than as two places being different kinds of place. The two candidates people
reach for — a note's metadata, and its session artifacts — are answered inside the note's own view:
they sit in the details rail described below, which is a track in that view's own grid rather than a
region, and every other destination keeps the full main slot.

The note editor's details rail is not a counterexample. It is a second column the note screen draws
for itself inside the one main slot — not a second region the shell provides, and nothing outside
that view knows it exists. It is also the one place where prose ends against a neighbour instead of
at its own measure (DESIGN_SYSTEM §1's carve-out): the body runs to the rail, so no width is ever
stranded between the two. Past the reading surface's own bound the extra falls outside the pair, as
margin — never back into the gap.

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
link. (Settings used to be the other example here: a `role="tablist"` rail that *filtered* the pane
rather than navigating it. Its Grove ticket deleted the rail rather than defending it — a filter that
hides three quarters of a config panel is a filter over four cards that all fit.) Affordances
*inside* the content belong to the content: a note's tag chips, a recording's
`<audio>` player. A control a **view state** raised belongs to that state and not to the frame,
whether it recovers or announces — Terminal's Restart, Chat's Start a new chat, the error boundary's
Try this screen again, and the Inbox's filed toast all sit inside a state block, and the vocabulary
for those is [`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §3. And the dock's New project button is
global, so it lives in the other region entirely.

---

## 6. Building a screen

Grove is the whole system now: there is no second vocabulary to migrate off, and no frozen layer to
avoid. A screen styles with the utilities and tokens in §3, takes `cva` for any variant it has, and
writes its reduced-motion swap at the call site beside the motion it undoes.

Two habits from the migration are worth keeping, because both catch failures the markup cannot show:

- **Read the built CSS when a utility is load-bearing** (§1). That is how the press was caught
  animating nothing — `transition-transform` against Tailwind v4's standalone `scale` property —
  and how you confirm a `@utility` recipe emitted its `.day` and `prefers-reduced-transparency`
  branches.
- **A token name must not collide with an unlayered declaration.** `@theme` emits into
  `@layer theme`, and unlayered CSS beats every layer, so a shared name silently hands the utility
  the other value rather than conflicting loudly. `rounded-card` shipped at 12px instead of 14px
  that way, against the pre-Grove tokens. One stylesheet leaves the collision nowhere to come from;
  a new one would reopen it, which is half of why the `.css`-import guard exists.
