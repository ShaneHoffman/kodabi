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
chrome is compact, a note still opens like a page. Weights, eyebrow, cap, and display were unchanged
by that move; tracking and leading step separately, on size — see below.

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

### Tracking and leading are size-specific, not just role-specific

The eyebrow steps above vary by *depth*. The title steps vary by **size**, which is the other half of
the same idea. Letters read further apart the larger they get, so a single tracking value cannot
serve 26px and 36px: the step that looks composed on a Settings title leaves a note title looking
spaced out. Leading runs the same way, inverted — a ratio that reads as one block at 26px opens into
two drifting lines by 36px.

Each title step is therefore a **triple**. The size never travels alone:

| Step | px | Tracking | Leading |
| --- | --- | --- | --- |
| `text-row` (sans) | 18 | `tracking-row` -0.005em | the interface ramp's |
| `text-title-panel` | 26 | `tracking-title-panel` -0.012em | `leading-title-panel` 1.12 |
| `text-title-health` | 28 | `tracking-title-health` -0.014em | `leading-title-health` 1.1 |
| `text-title-library` | 34 | `tracking-title-library` -0.019em | `leading-title-library` 1.06 |
| `text-title-doc` | 36 | `tracking-title-doc` -0.02em | `leading-title-doc` 1.05 |

`ViewFrame` emits all three for the variants it draws, so its titles cannot drift. A title spelled by
hand — the note editor's, a dialog heading — must spell all three too; the size alone renders at body
leading with no tracking, which is what the three dialog headings used to do.

**Enforced, not aspirational.** `src/titleSteps.test.ts` (in `pnpm test`) scans every `src/**/*.tsx`
for a class string carrying a `text-title-*` and fails it if the matching `leading-title-*` and
`tracking-title-*` are not in the same string. That covers the `ViewFrame` variants no component test
exercises, every hand-spelled title, and any new one added later — a half-spelled step renders fine
and typechecks fine, so nothing else would catch it.

**Nothing compensates for us.** Source Serif 4 is loaded as static weights (`src/fonts.ts`) with no
`opsz` axis, so every title from 26 to 36px is set from the same text-optimised master. A variable
face with optical sizing would do some of this itself; this one will not.

**`tracking-wordmark` (+0.02em at 22px) is deliberately off this ramp** — the wordmark is a logotype,
letterspaced on purpose, not a title that happens to be that size. Don't "fix" it to a negative step.

The values are sub-pixel per letter (-0.019em at 34px is -0.65px) and are meant to be: the effect is
cumulative across a word, not visible on any single pair.

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
| Row padding | `--row-queue-*` · `--row-library-*` · `--row-search-*` | A content row consumes its own pair (20/16, 16/14, 15/12) |
| Title ↔ action column gap | `--gap-row-columns` (28px) | `.project__row`'s grid; folded into the Inbox row's `--row-queue-trail` reservation |
| Card stack gap | `--gap-card` (14px) | Pre-lifted cards (`.attention__stack`) |
| Nested row indent | `--space-sm` (16px) | A settings row inside a `role="group"`, subordinate to it |
| Header → list lead-in | `--lead-*` | The gap between a view's header and the thing it heads |

A content row's geometry is not on the 4px step scale and is not shared between views: the row is
where a queue, a library and a health view differ most, so each names its own in `design/tokens.css`
(Layer 4) and consumes it from a co-located `*.css` (see `docs/UI_CONVENTIONS.md`, *Layer 4*). Being
off the step scale is why they are named, not an excuse to leave them as literals. What is shared is
the *rhythm* of a nav list, which is chrome rather than content and stays on the step scale.

Rows align `items-start` when the row has a multi-line body, `items-baseline` when it is a single
line. Pick by content, not by view.

**A list is not a table.** No column rules, no zebra striping, no borders (DESIGN.md refuses
admin-panel density). Separation is space and value.

**Rank between rows is indent, proximity and semantics.** A row that depends on the one above it is
indented one step and carries no rule, no rail, no fill and no expander; the cluster is a
`role="group"` with a name, so the dependency reaches a screen reader instead of living only in the
pixels. A group that no row heads draws a heading, and that heading ranks up with `--fw-medium` and
`text-text` per *Weight and colour carry rank before size does* above — not with a larger size, and
not with a fourth eyebrow step. The rhythm *inside* a group is the view's own row pair, unchanged;
only the air around it grows, because a nested row is a whole row and a shorter box would say it is
not. And an indent is a claim about dependency, so it has to be true: two independent options are
peers under a heading, never one nested inside the other. See `docs/UI_CONVENTIONS.md`,
*A dependent setting is grouped, not just listed*.

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

**It really is one.** Four fields carried `outline: none` and no ring — the quick-capture textarea,
the search query input, and the note editor's title and body. Only the first was ever argued for,
on the grounds that its window held a single control; that argument expired when the window gained
a visible **File it** button, and the paragraph saying so shipped while the code did not. All four
draw the ring now. An exception justified by "there is only one control here" expires the moment
that stops being true.

**When the focusable element is not the thing the user sees, the wrapper wears the ring.** The
search query field is a padded pill containing a chrome-less `<input>`; ringing the input draws a
second box inside the first. `ui.css` carries `.ui-focus-ring-within` for that shape — the *same*
declaration at a `:has(:focus-visible)` trigger, one rule with two selectors, never a second copy
of the recipe.

### Hover may enhance, never reveal

Every action is reachable and discoverable without a pointer. Hover adds emphasis to an
already-visible control. A control that only appears on hover does not exist for keyboard or touch.

**Hover and focus must target the same element.** Putting `hover:` on an inner span while the outer
button carries the ring means keyboard users get a ring with no colour change, and hovering the rest
of the row does nothing.

### Destructive is a confirmation, not a colour

There is no red token, and DESIGN.md refuses hue as a ranking device. So a destructive action is not
marked by painting its control: it is marked by **making the user confirm**, and by the confirming
control being the non-default one (`quiet` weight beside a `primary` cancel).

The first destructive flow is **Delete project** (`DeleteProjectDialog`), and it landed
`variant="destructive"` together with this rule. The variant is deliberately not a fourth look: it
wears the quiet ghost's exact chrome (shared selectors in `Button.css`, never a second copy) and
exists so call sites state intent. Its contract: a destructive button may only ever appear inside a
confirmation dialog, as the non-default control beside a `primary` Cancel — and the Cancel is what
holds initial focus (`useDialogFocus`), so the keyboard's first Enter dismisses rather than
destroys.

This shape is now a shared primitive, `DestructiveConfirmDialog` (`src/components/ui/`), so the rule
above lives in one place. Both destructive flows compose it: **Delete project** and the Needs
Attention **capture delete**, which replaced an inline second-click confirm with the same modal.
Every new destructive action confirms through it rather than hand-rolling a fourth dialog.

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
| `--dur-plane` | 180ms | A row rising onto the raised plane; the toggle knob's travel; a fresh-filed row's fill-in |
| `--dur-settle` | 200ms | A row leaving or entering a list; the Inbox placeholder's vanish-left; the filed toast's entrance and fade; the chat answer's arrival |
| `--dur-enter` | 280ms | The Inbox placeholder arriving at the top of the queue |
| `--dur-wake` | 450ms | The spirit-mark waking and settling |
| `--dur-wave` | 1000ms | One waveform bar's rise and fall |
| `--dur-pulse` | 1600ms | Starting / reconnecting pulse |
| `--dur-glow` | 2600ms | The listening dot's breath; the Inbox placeholder's working dot |
| `--dur-breath` | 4200ms | The listening breath cycle |
| `--dur-drift` / `--dur-drift-slow` | 15s / 21s | The aura's counter-rotating blobs |

`--delay-wave-1` … `--delay-wave-4` (0 / 220 / 420 / 140ms) are the four offsets one waveform
animation is started at, so the group reads as a voice rather than a metronome. The order is
deliberately not monotonic.

Easings: `--ease-standard` (state changes), `--ease-breath` (the continuous listening motion),
`--ease-drift` (constant-rate rotation). Never a bare `0.2s` or `ease-in-out` in a component.

**`--ease-standard` is a stated curve, not the CSS keyword.** It is
`cubic-bezier(0.2, 0, 0, 1)` — leaves immediately, arrives softly. It used to be the bare keyword
`ease`, whose symmetric ease-*in* makes a 150ms hover feel like it hesitates before it starts.
Every state change in the app runs on this one curve, so the shape of the motion is a single
decision made in `tokens.css` rather than a per-component one. `--ease-breath` stays symmetric on
purpose: a breath has no direction.

### What animates

**Animates:** a control's own state change (`--dur-quick`); a row leaving a list (`--dur-settle`);
the spirit-mark, which is the app's one continuous motion; and the Inbox pipeline placeholder,
which spends both halves of the "one deliberate motion" FOUNDING_DOC §4 reserves for
distill-and-route. It arrives at the top of the queue (`--dur-enter`) as a capture stops, and when
it resolves to a different project it travels left — toward the sidebar, where projects live — and
vanishes (`--dur-settle`) before handing off to the filed toast. Resolved to the Inbox itself, there
is nowhere to travel to, so nothing travels: the placeholder's slot fills in with the routed note
using a plain fade (`--dur-plane`) instead.

And **the chat answer's arrival** (`--dur-settle`): the live answer block in `ChatView` fades and
rises a few px into place as it first appears, matching the filed toast's direction because it is the
same gesture — content joining a surface at its live edge. An answer materialising fully formed reads as
breakage rather than as arriving, which is this section's whole warrant for spending motion. Note
what is *not* animating: **a token feed is not a licence to animate**, and nothing here reacts to a
delta. The block mounts once per assistant block and then grows in place, so its `@starting-style`
resolves once per arrival rather than once per delta. A turn that stops to call a tool has several
such blocks (prose, tool line, more prose), and each one is a real arrival, so each gets the
entrance — what stays banned is a *stagger*, not a second arrival. See the list-of-unknown-length
bullet below for the boundary this sits inside.

**Never animates:**

- **A data refresh.** `vault:changed` refetches constantly. Animating it would make the app twitch
  whenever a file changed on disk.
- **Overlay entrance, with one sanctioned exception.** The palette, the consent nudge, quick
  capture, the capture pill, and `CaptureToast` all appear instantly — showing the surface *is* the
  transition, and a hotkey surface that fades in feels slow. The Inbox's filed toast (`InboxView.tsx`)
  is the exception: it is not a surface arriving cold, it is the second half of a gesture already in
  motion (the placeholder's vanish, immediately before it), so its own short entrance (`--dur-settle`)
  reads as one continuous motion rather than two unrelated ones.
- **Layout.** Nothing reflows under the user.
- **Anything on a list of unknown length.** Staggered row animations are decoration. The
  placeholder's arrival, a fresh-filed row's fill-in, and the chat answer's arrival are each a
  one-shot reaction to a real event on exactly one row, not a stagger applied across a list of
  unknown length.

  **The chat log is the case that shows where the line is**, because it is both a list of unknown
  length and a surface with a real arrival on it. Only the *live* answer block animates. Every
  entry in the log — your messages, completed answers, tool lines, permission cards, errors —
  carries no entrance transition, which is what makes "scrollback never animates" provable rather
  than asserted: there is nothing to fire on arrival. (The approval card's Allow/Deny buttons keep
  the ordinary `--dur-quick` control states every `Button` has; a control answering the pointer is
  not the log animating.) The completed entry an answer hands off to is
  deliberately denied the entrance class, so finishing a turn does not re-fade prose already being
  read. A per-entry or staggered entrance across that log stays banned, and adopting
  `@starting-style` for the one block does not license it (see below: it settles how, not whether).

**Two sanctioned layout transitions, and they are the only two.** "Never animates: layout" is about
content moving under the reader, and neither of these does:

| Where | Property | Why it stands |
| --- | --- | --- |
| `.inbox__fill` | `width` | The progress instrument is a 3px rule with rounded caps. `scaleX` would stretch the cap into an ellipse, and the element is three pixels tall — there is no reflow cost worth the distortion. |
| `.inbox__slot` | `grid-template-rows` (`1fr` → `0fr`) | A filed row collapsing as it leaves. This *is* the minimal form of the distill-and-route motion FOUNDING_DOC §4 reserves, and the reflow is the point: the gap closes so the list does not jump when the refetch lands. |

Recorded here so the next audit does not re-flag them. Anything else animating a layout property is
still a bug.

### An entrance is a starting style, not a mount flag

**Where the list above sanctions an entrance, it is a `transition` plus `@starting-style` in the
component's own CSS.** Never a `mounted` flag flipped after the first render, and never a second
`@keyframes` block for what is a two-state change.

```css
.thing {
  transition: opacity var(--dur-settle) var(--ease-standard);
}

@starting-style {
  .thing { opacity: 0; }
}
```

The React idiom for this is a `useState` flag set from a `useEffect`, and it fails twice over. A
boolean is not an external system with cleanup, so it cannot be a bridge hook
([`.claude/rules/no-use-effect.md`](../.claude/rules/no-use-effect.md)) — licensing one would cost
three edits for a fade. And it is a frame late by construction: the element paints at rest, then
jumps back to animate. `@starting-style` hands the browser the transition's first computed style
*before* the first paint, so there is nothing to flip and nothing to catch up to.

**This settles how, not whether.** The Never-animates list above still decides which surfaces get an
entrance at all. Adopting the mechanism licenses nothing new.

**`@keyframes` keeps the looping and multi-step motion.** An entrance has exactly two states, which
makes it a transition — and a transition is interruptible, where an `animation … both` plays out
regardless. Anything with a third state or a repeat (the breath, the waveform, the starting pulse)
stays an animation. The dividing line is now checkable: **every `@keyframes` left in `src/` loops.**

**Reduced motion is one declaration, and still two rules.** `transition: none` in both blocks (see
below) leaves `@starting-style` with nothing to run from, so the element paints at rest. The
app-wide 1ms floor is not enough on its own — it shortens the entrance rather than removing it, so a
frame of the starting style can still show.

**No `@supports` guard.** `@starting-style` is Chromium 117; the shipped WebView2 is 150.0.4078.105
and Tauri v2's own documented floor is 125, both above it. (CSS anchor positioning, at 131, is the
case that *does* need gating.) Measured inside the running app's WebView2 rather than read off the
registry — [`docs/decisions/popover-primitive.md`](decisions/popover-primitive.md) §5.1.

### Reduced motion

`src/index.css` applies an app-wide floor: under `prefers-reduced-motion: reduce`, animations and
transitions collapse to 1ms.

A component needing a *different* reduced-motion resting state overrides it in its own CSS. The
spirit-mark is the precedent and the reason the floor is 1ms rather than `none`: it must settle to a
**still green mark**, because the green carries the recording state and a blank mark would imply
privacy that does not exist. Never let reduced motion remove *information*.

**An override is TWO rules, not one.** There are two ways into this state — the OS setting
(`@media (prefers-reduced-motion: reduce)`) and Settings → Appearance, which sets
`:root[data-reduce-motion="on"]` (`src/reduceMotion.ts`). A media query and a plain selector cannot
share a rule, so a component that overrides the floor states its resting position in both blocks or
it only honours one of the two switches. `QuickCapture.css` is the shape to copy;
`ListeningIndicator.css` and `SpiritMark.css` shipped with only the media half, which meant the
in-app switch fell through to the generic 1ms floor — and that floor shortens a *duration*, it
cannot restore a resting *shape*. The spirit-mark's two bloom lobes are deliberately lopsided while
they drift, so the in-app switch froze them mid-drift as two skewed blobs instead of settling them
to `border-radius: 50%`.

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
a pigment *darker* than the ground, so every button, dropdown, selected row, and modal panel read as
a grey box stamped onto the page. Paper does not work that way: a lifted sheet catches more light
than what it sits on. That pigment (`mist`, from the original moss/fern/mist family) is **gone**,
along with the rest of that family — the re-tune replaced the whole ladder with the washi/sumi and
night/paper sets in `design/tokens.css`, and nothing in Layer 1 is named for a recessive tone any
more.

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

### Text on the active-row wash

`--menu-hover` is the ground under every selected or keyboard-focused row (it
is what `--wash-active` and `.ui-wash` resolve to), and it is a *fill*, not one
of the three planes — so the matrix above does not cover it. It has to, because
it is the row the user is looking at.

| | on `--menu-hover` (light #F0EEE6) | on `--menu-hover` (dark #37331F) |
|---|---|---|
| `text` #211F17 / #EFEDE3 | 14.20 | 10.81 |
| `text-soft` #55524A / #B3AFA1 | 6.72 | 5.78 |
| `text-faint` #8B8879 / #7B7768 *(do not use here)* | **3.07** | **2.83** |

`--text-faint` on this fill is the worst pair in the app, and the dark value
misses even the 3:1 a *graphic* is held to. Two things wore it: the command
palette's `↵` hint on the active row, and `Select`'s check glyph on the chosen
option — which is very often also the active one. Both are `--text-soft` now.

### `--text-faint` is a metadata register, and it is not spent on anything else

**It measures 3.12–3.50 in the light theme and 3.37–4.04 in the dark one,
against a 4.5:1 requirement for text at these sizes.** The value comes from the
redesign's locked palette (`ink-3`), where it is specified for "metadata,
counts, eyebrows, placeholders" — which are text, not graphics, so the 3:1
graphic allowance does not apply to them.

Two exits were on the table: darken the pigment (`--k-stone` needs roughly
#6F6C5F, `--k-paper-faint` roughly #93907F — both visibly darker than the
design specifies, and both flatten the gap between `text-soft` and `text-faint`
that the three-step ink ladder depends on), or stop spending it where it
carries weight. **The second one is taken, applied narrowly.** The pigment is
unchanged; what changed is which sites consume it.

**It had already been widened well past its brief**, which is what forced the
issue. Twelve sites wore it on things that are not metadata at all — the
Commands nav row, an unselected Settings tab, the consent value, *Dismiss* on a
needs-attention card, the note editor's back link, its remove-tag `×` and its
`+ tag` ghost, the toast and overlay-pill dismiss buttons, `Select`'s entire
`token` trigger (the Inbox's only filing affordance), and `StatusMessage`'s
`status` variant. All are `--text-soft` now (6.84–7.67 light, 6.89–8.26 dark).

**What may still wear it**, and the whole list:

- Section eyebrows, and the sidebar's rail eyebrow
- List counts, and mono meta lines on a reading surface
- The Inbox progress caption, and search breadcrumbs
- Keyboard hints and key-sequence chips (`⌘K`, `Ctrl + Shift + K`)
- Input placeholders
- A capture status line in its *not live* state, where the value **is** the
  information (`CaptureStatusLine`, and the label beside the listening dot)

**Anything a user acts on, or is meant to read as a sentence, is `--text-soft`
or darker.** A control label, an error, a status announcement, and a value the
user may need to report back are none of them metadata. That rule is the
resolution; the residual gap is the list above, and it is small, bounded, and
deliberately near-silent.

### The pill toggle's boundary is under 3:1, and the knob is what carries it

Measured, since the toggle is a graphic: the ON track (`--toggle-on`) is
**1.21:1** against `bg` in the light theme and **1.43:1** in the dark one, and
the resting track plus its ring composites to about **1.10:1**. None of that
clears 3:1.

What does, comfortably, is the **knob** — `--text` on the raised plane, about
15:1 — moving `--toggle-travel` (18px) across the track. The component is
identifiable and its two states are distinguished by a large positional
difference rather than by a fill nobody can see, which is what the requirement
is actually asking for. The track's ring was `--edge-strong` (.09/.12) and is
now `--edge-check` (.28/.32) — the same ring an unchecked `Checkbox` wears, so
the app's two booleans agree — which roughly doubles the boundary without
turning a quiet control into a bordered box.

This is recorded, not fixed: reaching 3:1 on the track alone needs an edge at
roughly 0.5 alpha, which is a visible frame and against DESIGN.md's *space
instead of borders and boxes*. If a second toggle-like control ever ships, this
is the paragraph to revisit.

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
  **`Select` and the Settings `Toggle` honour this too.** Both take a `busy` prop with the same
  contract as `Button`'s `loading` — `aria-disabled` + `aria-busy`, focusable, activation swallowed
  by the component — and `disabled` stays a genuine "there is nothing to choose here". Every
  in-flight write in the app now goes through `busy`: the Inbox's file picker, all three Settings
  writes, and the consent nudge's retention picker. The nudge is why this matters most: it is a
  modal whose Escape and Tab handling lives on the panel, so dropping focus to `<body>` left the
  user inside a dialog they could no longer close from the keyboard. Its day-count field takes
  `readOnly` rather than `disabled` for the same reason — read-only keeps a field focusable.
- **`aria-current="page"`** marks the selected navigation row.
- Items in an `aria-activedescendant` listbox are deliberately not tabbable; focus stays on the
  controlling input.

### Live regions

| Role | For | Politeness |
| --- | --- | --- |
| `role="status"` | Progress — the Inbox placeholder's "Transcribing the capture" → "Distilling the meeting"; capture state (`ListeningIndicator`); the filed-toast's "Filed to \<project\>" | polite |
| `role="alert"` | Failures the user did not cause and may not be looking at | assertive |

**Every async failure gets `role="alert"`.** A retry that fails silently while the user looks
elsewhere is a failure the app never reported.

**Debounce a live region fed by a flapping source.** `ListeningIndicator` is the pattern to copy: the
mark reacts instantly for visual feedback, but the announced label follows
`useDebouncedValue(state, 400)`, so a flapping voice-activity detector cannot spam a screen reader.
The visual and the announcement are allowed to run at different speeds.

**One region per concern, but a concern can be a narrative.** Capture state is its own region
(`ListeningIndicator`) — a different concern from what happens after a capture stops. Transcription
and distill, though, are chapters of one story up to the point they resolve, and the Inbox
placeholder (`InboxView.tsx`) reports that story as a single `role="status"` region whose text
advances — "Transcribing the capture" → "Distilling the meeting". A screen reader announces the
rewritten text each time it changes, so nothing is lost by not spawning a new node per stage.

The story then ends one of two ways, and only one of them needs a new region. Routed to the Inbox
itself, the placeholder goes silent and the row it becomes is the result — there is nothing left to
announce. Routed to a different project, the placeholder vanishes and hands off to a **fourth**
region: the filed toast, a freshly inserted `role="status"` node announcing "Filed to \<project\>"
the moment it appears. Spawning a new node here is correct rather than a risk, unlike the case above
— the placeholder is already leaving, so there is no existing region left to rewrite, and a
freshly-inserted live region is the ordinary, well-supported way a screen reader picks up new
content. `CaptureToast` is the one `role="alert"` region: only a transcription or distill failure
ever reaches it, because a failed capture never produces a note for the placeholder or the toast to
describe.

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
  and every `src/**/*.css`, and fails on a literal colour, font-family, duration, or spacing value
  (padding / margin / gap, in px, rem or em) outside `tokens.css` — plus two structural assertions:
  each `--k-*` pigment is declared exactly once, and **every semantic token is mapped in all four
  theme blocks**. The second one is the quiet one and it earns its place: a new semantic key added to
  `:root` and forgotten in one of the two dark paths keeps its *light* value down that path, and
  nothing about that looks broken until someone runs the app in the OS-dark theme specifically.
- **[`eslint.config.js`](../eslint.config.js)** (`pnpm exec eslint .`) fails numeric spacing utilities
  (`p-3`, `gap-4`) and arbitrary values (`text-[13px]`) inside `className`.

Spacing is the one value checked on both sides, because it can be written in either place: eslint
reads the class strings, the token test reads the stylesheets, and neither can see the other's half.

**What neither guard can see at all is geometry.** The token test scopes its spacing check to
`padding` / `margin` / `gap`, so a `width`, a `height`, a `border` thickness or a `translate` distance
in a stylesheet passes both guards untouched. That is how the checkbox skin came to be written out
twice in two files and the pill toggle's five numbers left unnamed. The `--check-*` and `--toggle-*`
families exist because the rule holds whether or not a guard enforces it — see
[`docs/UI_CONVENTIONS.md`](UI_CONVENTIONS.md), *Layer 4*.

What they cannot catch — an eyebrow using the wrong tracking utility, a hover on the wrong element,
a missing `role="alert"` — is review's job, and this document is the checklist.
