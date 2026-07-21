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
move: a config panel and a note must not open at the same size, and `ViewFrame`
picks the step from its `variant` so two views of the same kind cannot drift.

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
value and twelve other call sites used the Tailwind default, so the same role rendered 8.8× apart.

The sidebar's rail eyebrow is the one that is sans-600 rather than mono, matching the design
reference: it is chrome sitting directly above sans nav labels, not a label on content.

An eyebrow labels a *section*. It is not a field label (that is `text-cap text-text-soft`, owned by
`TextField`) and not a status line (that is `text-cap`).

### Weight and colour carry rank before size does

Per DESIGN.md, value carries the hierarchy. In practice:

- **Rank a row** with `--fw-medium` (`font-medium`) and `text-text`, not with a larger size.
- **Recede** with `text-text-soft`, then `text-text-faint`. Never with a smaller size than `text-cap`.
- **Never rank with hue.** `--accent` marks *interactive*, `--accent-dot` marks *recording*. Neither
  ranks anything.

### List density

| Property | Value | Applies to |
| --- | --- | --- |
| Row vertical padding | `py-2xs` (8px) | Every list row |
| Title ↔ action column gap | `gap-md` (24px) | Rows with a trailing control |
| Row group gap | `gap-3xs` (4px) | Tight nav lists (Sidebar) |
| Between list sections | `gap-lg` (40px) | Views composing several lists |

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
| `--dur-quick` | 120ms | Hover and colour changes on a control |
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
box-shadow: var(--hairline);                    /* a full ring */
```

Use `--edge-faint` for a divider inside a surface, `--edge` for the edge of a control. Two adjacent
elements must not use different edge tokens for the same visual line. The three edge alphas opened
one step on `feat/screen-overhaul` (`--edge-faint` .16, `--edge` .26, `--edge-strong` .36): the day
ground moved to a near-white and the old alphas went invisible on it. They remain a single mid-grey
at low alpha, so one set of values reads on a near-white card and a near-black one alike.

### A raised plane lifts, it does not fill

`--surface` is **lighter than the page in both themes**, and `--overlay` is lighter again. In the
light theme that ladder is #F2F0E9 → #FBFAF6 → #FEFDFB; in the dark theme #17160F → #221F16 →
#2A2618.

That is an inversion in the light theme, and it is the point of this pass. `--surface` used to map to
`mist` — a fill *darker* than the ground — so every button, dropdown, selected row, and modal panel
read as a grey box stamped onto the page. Paper does not work that way: a lifted sheet catches more
light than what it sits on. `mist` remains a Layer-1 pigment (the recessive tone) but no longer
paints a surface.

So the recipe for "raised" is **value shift + `--hairline` + `--lift`** — three quiet signals, no
box. If a raised element still doesn't read, the answer is the hairline or the lift, never a darker
fill and never a border.

### The lifts are three-stop shadows

`--lift` and `--lift-soft` are built from three stops each, not two:

| Stop | Job |
| --- | --- |
| Contact | `0 1px 1px` — the hard line where the plane meets the page; what makes the edge read crisp |
| Key | the mid-distance offset shadow; the direction of the light |
| Ambient | the wide, low-alpha spread; the softness around it |

`--lift-soft` drops the ambient stop and is for something barely off the page; `--lift` is the full
three for dropdowns and modals. The geometry differs per theme (the night lifts are darker and
throw further, because a shadow on a near-black ground has less value to work with), which is why
`--lift-day` / `--lift-night` are two full Layer-1 sets rather than one recipe. Never write a
`box-shadow` with literal offsets in a component; use the token.

### Modals

Every modal goes through the `Overlay` primitive, which owns the scrim (`--scrim`), the layer, and
backdrop dismissal. Dismissal fires on **click, not pointerdown**, and only when the gesture both
started and ended on the backdrop — otherwise a drag that begins inside the panel dismisses it, and
an unmount at pointerdown lets the rest of the gesture fall through to whatever was underneath.

**The scrim is dark in both themes.** `--scrim` is
`color-mix(in srgb, var(--k-night-sink) 46%, transparent)` — mixed from the deepest pigment, not
from a semantic token. It used to mix from `--bg-sink`, which meant the day theme dimmed washi with
washi and a palette opened over the app barely registered as being in front of it. A scrim is a
shadow, and a shadow does not re-theme. This is the same inversion as `--sheen`, which is white in
both themes. It is still a Layer-3 derived recipe referencing a Layer-1 pigment; the ten semantic
keys are unchanged.

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
  `Select` and `Checkbox` do not do this yet: their `disabled` prop is a genuine disable, so a write
  in flight behind one still costs the user their place in the page.
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
