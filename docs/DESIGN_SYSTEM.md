# Kodabi — design system (states, motion, elevation, accessibility)

*Status: Living (Phase 3, design-system pass). Three documents describe the look, and they divide
cleanly:*

| Document | Fixes | Changes when |
| --- | --- | --- |
| [`docs/DESIGN.md`](DESIGN.md) | The **aesthetic** — the four principles, the reference class, what we refuse | Almost never. The principles are fixed; the material was re-tuned once, on `feat/screen-overhaul` |
| **This document** | The **system** — how a control behaves, what a view shows when it has nothing, what may move, what must be legible | A new interaction or state appears |
| [`docs/UI_CONVENTIONS.md`](UI_CONVENTIONS.md) | The **mechanics** — spacing steps and the primitive catalogue | A primitive is added or changed |

[`design/tokens.css`](../design/tokens.css) is the material all three describe.

**The point of this document is that a contributor styling a new component makes no judgment calls.**
Every question below is answered with a named token. If you find yourself choosing a value, the
answer is missing here and belongs here.

---

## 1. Typography and density

### The scale

| Step | px | Role | Never |
| --- | --- | --- | --- |
| `text-eyebrow` | 11 | Section eyebrows, always mono + `uppercase` + a tracking step | Body copy |
| `text-micro` | 11.5 | Dense mono: the capture footer, a progress caption, a search breadcrumb | — |
| `text-cap` | 12 | Mono counts, meta lines, hints, status text | A paragraph someone reads |
| `text-meta` | 12.5 | Mono meta on a reading surface (a note, a library row) | — |
| `text-action` | 13 | A compact action inside an overlay window | A view-level action |
| `text-label` | 14 | Control labels, settings values, view actions | — |
| `text-body` | 15 | Interface body, nav items, menu items | — |
| `text-snippet` / `-sm` | 15 / 14.5 | A serif snippet in a list (the smaller one is search density) | Interface chrome |
| `text-lead` | 16 | A queue masthead; a search result's title | — |
| `text-input` | 17 | A query field (palette, search) | — |
| `text-row` | 18 | A working-list row title (sans) | — |
| `text-read` | 18 | A note body paragraph (serif) | Interface chrome |
| `text-h3` | 19 | A note's section header; a library index row | — |
| `text-capture` | 21 | A captured thought | — |
| `text-wordmark` | 22 | The sidebar wordmark | Anywhere else |
| `text-title-panel` | 26 | The Settings title | — |
| `text-title-health` | 28 | The Needs-attention title | — |
| `text-title-library` | 34 | A project title | — |
| `text-title-doc` | 36 | A note title | — |

**Fixed px, not rem, and not fluid.** The window is 960×640 and does not reflow
into a phone; the old `clamp()` steps only made the same role render at two
sizes for no reader's benefit.

**There are four view-title steps, not one.** That is the redesign's loudest
move: a config panel and a note must not open at the same size. `ViewFrame`
picks three of them from its `variant` (`panel`, `health`, `library`), so two
views of the same kind cannot drift. `text-title-doc` is the exception, because
`doc` renders no header of its own: the note editor spells the step itself, on
the title it draws beside its back link.

Sizes always come from this scale. Never `text-[13px]`, never a raw `font-size`. Enforced by the
eslint rule described in §7.

### Two voices: the interface ramp and the reading ramp

The scale above is not one ramp, it is two, and they diverge on purpose.

- **The interface ramp** — eyebrow through lead — is the voice of lists, controls and chrome, and it
  is compact: 11–16px, at `--lh-body` 1.5.
- **The reading ramp** — `--fs-read` 18 at `--lh-read` 1.62, plus `--fs-capture` and the four title
  steps — is the voice of a note, a snippet, and a captured thought. It is generous on purpose.

Before, one loose ramp served both, so the interface wore reading-sized type at reading line-height
and every list read like a document. The gap between `--fs-body` and `--fs-read` is now the point:
chrome is compact, a note still opens like a page. Weights, letter-spacings, eyebrow, cap, and
display are unchanged.

**Don't close the gap.** Reaching for `text-read` to make an interface element feel more generous
re-merges the two voices; the interface answer is space (§1, list density), not a larger size.

### The eyebrow is one thing, at three depths

A section eyebrow is `font-mono text-eyebrow uppercase text-text-faint` plus **one of three tracking
steps**, and the step says how deep it sits:

| Utility | Token | Where |
| --- | --- | --- |
| `tracking-eyebrow` | 0.16em | A view's own eyebrow: `UNFILED`, `SYSTEM`, `PROJECT` |
| `tracking-eyebrow-menu` | 0.14em | A section inside an overlay: `JUMP TO`, `ACTIONS` |
| `tracking-rail` | 0.12em | The sidebar's `PROJECTS` |

Nesting reads through the letter-spacing before it reads through the size. **Never Tailwind's
`tracking-wide`** (0.025em) for any of them: before the tokens were bridged, the Sidebar used one
value and twelve other call sites used the Tailwind default, so the same role rendered 6.4× apart.

The sidebar's rail eyebrow is the one that is sans-600 rather than mono, matching the design
reference: it is chrome sitting directly above sans nav labels, not a label on content.

An eyebrow labels a *section*. It is not a field label (that is `text-cap text-text-soft`, owned by
`TextField`) and not a status line (that is `text-cap`).

### Weight and colour carry rank before size does

Per DESIGN.md, value carries the hierarchy. In practice:

- **Rank a row** with `--fw-medium` (`font-medium`) and `text-text`, not with a larger size.
- **Recede** with `text-text-soft`, then `text-text-faint`. Never with a smaller size than `text-cap`.
- **Never rank with hue.** There is exactly one hue left in the palette: `--accent-dot` marks
  *recording*, and it ranks nothing. The old `--accent` that marked *interactive* is gone as a token,
  so there is no second hue to reach for by accident.

### List density

| Property | Value | Applies to |
| --- | --- | --- |
| Row group gap | `gap-3xs` (4px) | Tight nav lists (Sidebar) |
| Row padding | Layer 4, per view | A content row draws its own (`.inbox__row` 20/16, `.project__row` 16/14) |
| Title ↔ action column gap | Layer 4, per view | `.inbox__row` and `.project__row` both sit at 28px |
| Card stack gap | `--gap-card` (14px) | Pre-lifted cards (`.attention__stack`) |
| Header → list lead-in | `--lead-*` | The gap between a view's header and the thing it heads |

A content row's geometry is not on the 4px step scale and is not shared between views: the row is
where a queue, a library and a health view differ most, so each states its own in a co-located
`*.css` (see `docs/UI_CONVENTIONS.md`, *Layer 4*). What is shared is the *rhythm* of a nav list, which
is chrome rather than content and stays on the step scale.

Rows align `items-start` when the row has a multi-line body, `items-baseline` when it is a single
line. Pick by content, not by view.

**A list is not a table.** No column rules, no zebra striping, no borders (DESIGN.md refuses
admin-panel density). Separation is space and value.

**A subordinate section may not out-measure its subject.** An exception block (the Inbox's "Needs
attention") that grows unbounded will bury the content the view is named for. Cap it and let it
expand, so the view's actual subject always leads.

---

## 2. Interaction states

Every control shows all five where they apply. The primitives in `src/components/ui/` bake these in,
so a screen composing primitives gets them for free and must not restate them.

| State | Treatment | Token |
| --- | --- | --- |
| **Rest** | Per variant | — |
| **Hover** | Value step toward `--text`, over `--dur-quick` | `--dur-quick`, `--ease-standard` |
| **Focus-visible** | 2px **ink** outline, offset by its own width | `.ui-focus-ring` |
| **Active** (pressed) | One value step past hover, no transition (a press must feel immediate) | — |
| **Disabled** | `text-text-faint` + `cursor-not-allowed`; controls with no text to fade use `--disabled-opacity` | `--disabled-opacity` |
| **Destructive** | See below — a confirmation, not a colour | — |

### Focus is one recipe, applied by class

```css
.ui-focus-ring:focus-visible {
  outline: var(--focus-width) solid var(--text);
  outline-offset: var(--focus-offset);
}
```

**The ring is ink, not a hue.** The old interactive-green accent is retired: the
palette has exactly one green and it means audio is being recorded, so a focused
button wearing it would claim something false. Ink also reads at full contrast
against all three planes in both themes, which the green never managed in the
light one.

Defined once in [`src/components/ui/ui.css`](../src/components/ui/ui.css). Add the class; never
retype the declarations, and never use plain `:focus` (a pointer press would draw it).

**One sanctioned exception:** the command palette input. Its dialog's only tabbable element is that
input, and it shows a caret, so a ring would be noise rather than information. A form field always keeps
its ring.

(The quick-capture textarea used to be a second exception on the same reasoning. It stopped being one
when the window gained a visible **File it** button: with two tab stops, the ring is the only thing that
says which one has focus. An exception justified by "there is only one control here" expires the moment
that stops being true.)

### Hover may enhance, never reveal

Every action is reachable and discoverable without a pointer. Hover adds emphasis to an
already-visible control. A control that only appears on hover does not exist for keyboard or touch.

**Hover and focus must target the same element.** Putting `hover:` on an inner span while the outer
button carries the ring means keyboard users get a ring with no colour change, and hovering the rest
of the row does nothing.

### Destructive is a confirmation, not a colour

There is no red token, and DESIGN.md refuses hue as a ranking device. So a destructive action is not
marked by painting its control: it is marked by **making the user confirm**, and by the confirming
control being the non-default one (`quiet` beside a `primary` cancel).

No destructive action ships today — nothing in the app deletes anything — so no `variant="destructive"`
exists either. Shipping an unused variant would be inventing a look with nothing to check it against.
The first delete lands the variant and this rule together.

### A state change never changes the box

**Hover, focus and selection may change the fill, the elevation and the weight.
They may not change the padding, the size, or the position of anything.**

The design reference draws the sidebar's active pill larger than the row beside
it (12/16 against 7/8). A static mock never transitions between the two, so the
difference is invisible there; live, selecting a project slid its label 8px
right, grew the row 10px, and pushed every row below it down. Each nav group
now has exactly one box in every state.

The same rule is why an Inbox row's hover lift is `background` and `box-shadow`
only, why the Settings tab's underline is a `box-shadow` rather than a border,
and why `Button loading` swaps the label instead of the control.

### Selected is value, never hue

```css
background: var(--wash-active);   /* resolves to --menu-hover, both themes */
```

The row a pointer is over and the row the keyboard has landed on are the same colour, because they
mean the same thing.

Available as `.ui-wash`. **Never `--accent-dot`** for selection: the reserved green means audio is
being recorded, and a selected row wearing it is a lie.

### Selected *text* is ours too

```css
--selection: var(--highlight);
```

Applied once, by the `::selection` rule in `src/index.css`. Highlighted text used to wear the
platform's blue — the only colour in the app from outside the palette, and it appeared the moment
anyone dragged across a note. It is now the same ink wash a search match wears, so dragging across a
note and finding a term read as the same act of marking. Never the reserved green: selecting three
words is not a recording.

---

## 3. The view state vocabulary

Every view that reads from disk has four states. `useVaultQuery` returns `{data, loading, error}`,
so all three are always available; there is no excuse for an unhandled one.

| State | Treatment | Component |
| --- | --- | --- |
| **Loading** | Nothing. The view appears when ready | — |
| **Empty** | One sentence at `text-body text-text-soft`, left-aligned, saying what *would* be here and how it gets here | `StatusMessage variant="empty"` |
| **Error** | One sentence at `text-body text-text-soft` with `role="alert"`, prefixed with what failed | `StatusMessage variant="error"` |
| **Success** | Usually the result itself. A discrete confirmation only where the surface closes | `StatusMessage variant="status"` |

### Loading renders nothing, deliberately

No spinners, no skeletons. A local disk read completes in milliseconds, and a spinner that flashes
for 30ms is worse than nothing. **But `loading` must still be consumed** — a view that ignores the
flag renders its *empty* state during the fetch, so every cold start flashes "No projects yet."
before the data lands. That is the bug this rule exists to prevent.

Gate the empty state on `!loading && !error && items.length === 0`. All three conditions.

For an operation the user *initiated* (a save, a retry, a re-route), pending state is shown on the
control via `Button loading` — never as a page-level treatment.

### Empty states are first-run copy

Never a blank pane. The copy says what belongs here and how it arrives:

> "Nothing waiting. Notes the router can't place land here."

Not "No items." An empty Inbox on first launch is the user's first impression of the product.

### Errors name what failed and never leak an exception

> "Couldn't load projects: {message}"

Not the bare string. Before this pass three surfaces rendered a raw
`TypeError: Cannot read properties of undefined` straight into the UI. **Prefix every error with the
action that failed**, so the sentence is readable even when the message is not.

Errors are `text-text-soft`, never red: there is no red token, and DESIGN.md does not rank with hue.
Weight and the `role="alert"` announcement carry the urgency.

### Success

Most successes are self-evident (the note appears). Confirm explicitly only when the surface
disappears before the user can see the result — quick capture flashes its destination for
`FLASH_MS` before the window hides. A settings toggle that visibly moved needs no "Saved".

---

## 4. Motion

> **Motion as feedback, not decoration.** The distill-and-route moment gets one satisfying
> transition. Everything else is near-instant. (FOUNDING_DOC §4)

### The durations

| Token | Value | Spent on |
| --- | --- | --- |
| `--dur-quick` | 150ms | Hover and colour changes on a control |
| `--dur-settle` | 200ms | A row leaving or entering a list |
| `--dur-wake` | 450ms | The spirit-mark waking and settling |
| `--dur-pulse` | 1600ms | Starting / reconnecting pulse |
| `--dur-breath` | 4200ms | The listening breath cycle |
| `--dur-drift` / `--dur-drift-slow` | 15s / 21s | The aura's counter-rotating blobs |

Easings: `--ease-standard` (state changes), `--ease-breath` (the continuous listening motion),
`--ease-drift` (constant-rate rotation). Never a bare `0.2s` or `ease-in-out` in a component.

### What animates

**Animates:** a control's own state change (`--dur-quick`); a row leaving a list (`--dur-settle`);
the spirit-mark, which is the app's one continuous motion.

**Never animates:**

- **A data refresh.** `vault:changed` refetches constantly. Animating it would make the app twitch
  whenever a file changed on disk.
- **Overlay entrance.** The palette, the consent nudge, quick capture and the capture pill all appear
  instantly. Showing the surface *is* the transition. A hotkey surface that fades in feels slow.
- **Layout.** Nothing reflows under the user.
- **Anything on a list of unknown length.** Staggered row animations are decoration.

### Reduced motion

`src/index.css` applies an app-wide floor: under `prefers-reduced-motion: reduce`, animations and
transitions collapse to 1ms.

A component needing a *different* reduced-motion resting state overrides it in its own CSS. The
spirit-mark is the precedent and the reason the floor is 1ms rather than `none`: it must settle to a
**still green mark**, because the green carries the recording state and a blank mark would imply
privacy that does not exist. Never let reduced motion remove *information*.

---

## 5. Elevation and overlay layers

Three planes, and nothing invents a fourth.

| Plane | Background | Elevation | z |
| --- | --- | --- | --- |
| Page | `bg-bg` (the sidebar shares it) | none | auto |
| Raised | `bg-surface` | `--lift`, `--lift-card`, `--lift-row`, `--lift-chip*` | auto |
| Overlay — dropdown | `bg-overlay` | `--lift-menu`, `--lift-toolbar` | `--layer-dropdown` (10) |
| Overlay — window | `bg-overlay` | `--lift-palette`, `--lift-capture` | `--layer-overlay` (50) |

Three planes, and the sidebar is not a fourth. It used to sit on a recessed
`--bg-sink` fill, which made it a second, darker box beside the content rather
than part of the same sheet; that token is gone and the rail shares the page,
separated by one hairline.

**Every `--lift-*` token carries its own `0 0 0 1px` ring in the same
declaration.** A ring and a shadow that are set separately drift apart the
first time one of them is overridden, so they ship as one value.

Separation between adjacent planes is a **value shift plus a hairline**, never a border
(DESIGN.md: space instead of borders and boxes). Hairlines are inset shadows:

```css
box-shadow: inset 0 -1px 0 var(--edge-faint);   /* one edge   */
box-shadow: inset 0 0 0 1px var(--edge);        /* a full ring */
```

There is no `--hairline` token to reach for: a standalone ring recipe would be a second place for an
edge to be defined, and every lifted plane already carries its ring inside its own `--lift-*` value.
Spell the inset shadow out with the edge token the line calls for.

Use `--edge-faint` for a divider inside a surface, `--edge` for the edge of a control. Two adjacent
elements must not use different edge tokens for the same visual line. The ladder is **per theme, not
one shared mid-grey**: day maps to the warm ink alphas (`--edge-faint` .05, `--edge` .06,
`--edge-strong` .09 over `rgba(30, 28, 20, …)`), night to the paper alphas (.06 / .08 / .12 over
`rgba(255, 250, 235, …)`). A single set cannot serve both, because the day ground is a near-white
that swallows a dark line long before the night ground swallows a light one.

### A raised plane lifts, it does not fill

`--surface` is **lighter than the page in both themes**, and `--overlay` is lighter again. In the
light theme that ladder is #F2F0E9 → #FBFAF6 → #FEFDFB; in the dark theme #17160F → #221F16 →
#2A2618.

That is an inversion in the light theme, and it is the point of this pass. `--surface` used to map to
`mist` — a fill *darker* than the ground — so every button, dropdown, selected row, and modal panel
read as a grey box stamped onto the page. Paper does not work that way: a lifted sheet catches more
light than what it sits on. `mist` remains a Layer-1 pigment (the recessive tone) but no longer
paints a surface.

So the recipe for "raised" is **value shift + a `--lift-*` token** — the fill steps lighter, and the
token brings the ring and the shadow with it in one declaration. Two quiet signals, no box. If a
raised element still doesn't read, the answer is a heavier lift, never a darker fill and never a
border.

### A lift is a ring plus a shadow, in one value

Every `--lift-*` is written as `0 0 0 1px <edge>` followed by the shadow itself:

| Stop | Job |
| --- | --- |
| Ring | `0 0 0 1px` at an edge alpha — the crisp line where the plane meets its ground |
| Shadow | the offset, blurred stop; how far off the page the plane sits |

There is one lift per plane role rather than a soft/full pair: `--lift-chip` (a control at rest, a
1px contact shadow) through `--lift-card` and `--lift-row` up to `--lift-menu` and `--lift-toolbar`.
The two window recipes — `--lift-palette` and `--lift-capture` — are the only ones that add a
*second* shadow stop, a wide negative-spread throw under a near-full-height surface. The geometry
differs per theme, not just the alpha (the night lifts throw further, because a shadow on a
near-black ground has less value to work with), which is why `--lift-*-day` / `--lift-*-night` are
two full Layer-1 sets rather than one recipe with a swapped colour. Never write a `box-shadow` with
literal offsets in a component; use the token.

### Modals

Every modal goes through the `Overlay` primitive, which owns the scrim (`--scrim`), the layer, and
backdrop dismissal. Dismissal fires on **click, not pointerdown**, and only when the gesture both
started and ended on the backdrop — otherwise a drag that begins inside the panel dismisses it, and
an unmount at pointerdown lets the rest of the gesture fall through to whatever was underneath.

**The scrim is dark in both themes, but it is not the same dark.** `--scrim` is a Layer-2 semantic
key like any other: it maps to `--k-scrim-day` (`rgba(28, 25, 16, 0.34)`, a warm ink) in the light
theme and `--k-scrim-night` (`rgba(0, 0, 0, 0.5)`) in the dark one. Two flat pigments, no mix — a
scrim is the one thing in the app whose whole job is to take value *away* from what is behind it, so
it is stated as an alpha directly rather than derived from a plane that would move with the theme.
It re-themes because the two grounds need different amounts of it: half-strength black would crush a
near-black page, and a 34% warm ink over washi is exactly enough to say the palette is in front. It
used to mix from `--bg-sink`, which meant the day theme dimmed washi with washi and a palette opened
over the app barely registered as being in front of it; that token is gone.

`--sheen` is the genuinely theme-independent one — white in both themes, and it lives once in Layer 3
rather than per theme.

A modal traps focus (§6), takes `role="dialog"` + `aria-modal="true"`, and closes on Escape.

---

## 6. Accessibility floor

### Contrast

Measured, not estimated. Text pairs against WCAG AA (4.5:1); a graphic needs
3:1. Re-measured from the redesign's token values.

**Light (day washi)** — `bg` #F2F0E9, `surface` #FBFAF6, `overlay` #FEFDFB

| | on `bg` | on `surface` | on `overlay` |
|---|---|---|---|
| `text` #211F17 | 14.47 | 15.79 | 16.23 |
| `text-read` #2E2C24 | 12.26 | 13.39 | 13.75 |
| `text-soft` #55524A | 6.84 | 7.47 | 7.67 |
| `text-faint` #8B8879 | 3.12 | 3.41 | 3.50 |
| `accent-dot` #5F7D4F *(graphic)* | 4.06 | 4.44 | 4.56 |

**Dark (night sumi)** — `bg` #17160F, `surface` #221F16, `overlay` #2A2618

| | on `bg` | on `surface` | on `overlay` |
|---|---|---|---|
| `text` #EFEDE3 | 15.45 | 14.03 | 12.88 |
| `text-read` #DCD8CC | 12.73 | 11.55 | 10.61 |
| `text-soft` #B3AFA1 | 8.26 | 7.50 | 6.89 |
| `text-faint` #7B7768 | 4.04 | 3.67 | 3.37 |
| `accent-dot` #86AE6B *(graphic)* | 7.15 | 6.49 | 5.96 |

`text`, `text-read` and `text-soft` clear 4.5 everywhere with room to spare, in
both themes.

### `--text-faint` does not meet the text floor, and that is a known deviation

**It measures 3.12–3.50 in the light theme and 3.37–4.04 in the dark one,
against a 4.5:1 requirement for text at these sizes.** The value comes from the
redesign's locked palette (`ink-3`), where it is specified for "metadata,
counts, eyebrows, placeholders" — which are text, not graphics, so the 3:1
graphic allowance does not apply to them.

What currently wears it: section eyebrows, list counts, mono meta lines, the
Inbox progress caption, search breadcrumbs, keyboard hints, placeholders, and
the muted half of a two-action pair.

This is recorded rather than silently corrected because the palette is locked
upstream and the value is a deliberate part of the design's near-silent
register. It is a real accessibility gap all the same, and the two ways out are
both one-line changes here:

- **Darken the pigment.** `--k-stone` needs roughly #6F6C5F to clear 4.5 on
  `bg`, and `--k-paper-faint` roughly #93907F. Both are visibly darker than the
  design specifies and would flatten the gap between `text-soft` and
  `text-faint` that the three-step ink ladder depends on.
- **Stop spending it on text.** Move metadata to `text-soft` (6.84–7.67 light,
  6.89–8.26 dark) and keep `text-faint` for genuinely decorative marks.

Until one is chosen, do not widen its use: a new label reaching for
`text-text-faint` is adding to a known deficit.

**`--accent-dot` is a graphic, never text.** At 4.06–4.56 in the light theme it
does not meet the text floor, and it never has to: it is spent on the listening
dot, the waveform, and the text caret, where 3:1 applies and it passes
everywhere in both themes. The label beside it is `--text` when live and
`--text-faint` when not, so the state reads through value like every other
status line, and the green stays the mark's alone.

Any new colour pair is measured before it ships.

### Focus order

- **Every interactive element is a real `<button>`, `<a>`, or form control.** Never a `div` with
  `onClick`.
- **A modal traps focus** and restores it on close (`useDialogFocus`).
- **Focus must survive an optimistic swap.** Replacing the focused control with a `<span>Filing…</span>`
  drops focus to `<body>` mid-task and silently strips the user's place in the page. Keep the control
  mounted and use `Button loading` instead.
- **`disabled` drops focus too, so a *busy* control is `aria-disabled`, not `disabled`.** An element
  that is focused when it becomes disabled is blurred and focus resets to `<body>` (the HTML focus
  fixup rule) — the same failure as unmounting it, and worse inside a modal, whose Escape and Tab
  handling lives on an ancestor the focus has just left. `Button loading` therefore sets
  `aria-disabled` + `aria-busy` and swallows its own activation; the native attribute stays for
  `disabled`, which means "there is nothing to do here", not "something is in flight". A caller must
  not pass both for the same condition — `disabled` wins, and the focus goes.
  `Select` does not do this yet: its `disabled` prop is a genuine disable, so a write in flight behind
  one still costs the user their place in the page. (`Checkbox` has the same gap, but nothing composes
  it today, so nothing is currently exposed to it.)
- **`aria-current="page"`** marks the selected navigation row.
- Items in an `aria-activedescendant` listbox are deliberately not tabbable; focus stays on the
  controlling input.

### Live regions

| Role | For | Politeness |
| --- | --- | --- |
| `role="status"` | Progress and success — "Transcribing…", "Note saved" | polite |
| `role="alert"` | Failures the user did not cause and may not be looking at | assertive |

**Every async failure gets `role="alert"`.** A retry that fails silently while the user looks
elsewhere is a failure the app never reported.

**Debounce a live region fed by a flapping source.** `ListeningIndicator` is the pattern to copy: the
mark reacts instantly for visual feedback, but the announced label follows
`useDebouncedValue(state, 400)`, so a flapping voice-activity detector cannot spam a screen reader.
The visual and the announcement are allowed to run at different speeds.

**One region per concern.** Capture state, transcription, and distill are three concerns and get
three regions. Do not merge them into one node whose text rewrites itself.

### Keyboard paths

Every mouse flow has a keyboard flow, **and the reverse**: a hotkey-first surface still needs a
visible control. Quick capture is submitted with Enter and also with a real button, because a
pointer-only user cannot press Enter into a window they never focused.

A value that commits only on blur is invisible. Commit on Enter as well, and show that it committed.

**One sanctioned exception:** the capture overlay pill's drag. Its window is `focusable: false` so
appearing over a full-screen app cannot steal focus, which also removes it from the tab order. The
keyboard paths that matter remain: the capture hotkey stops the capture and the pill with it, and the
Settings toggle disables it permanently.

---

## 7. Enforcement

Both guards run in gates CI already runs. Neither adds a dependency.

- **[`src/designTokens.test.ts`](../src/designTokens.test.ts)** (`pnpm test`) reads `design/tokens.css`
  and every `src/**/*.css`, and fails on a literal colour, font-family, or duration outside
  `tokens.css` — plus asserts each `--k-*` pigment is declared exactly once.
- **[`eslint.config.js`](../eslint.config.js)** (`pnpm exec eslint .`) fails numeric spacing utilities
  (`p-3`, `gap-4`) and arbitrary values (`text-[13px]`) inside `className`.

What they cannot catch — an eyebrow using the wrong tracking utility, a hover on the wrong element,
a missing `role="alert"` — is review's job, and this document is the checklist.
