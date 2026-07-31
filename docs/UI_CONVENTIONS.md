# Kodabi — UI conventions (spacing & primitives)

*Status: Living (Phase 2, UI foundation; extended by the Phase 3 design-system pass).*

Three documents describe the look, and they divide cleanly:

| Document | Fixes |
| --- | --- |
| [`docs/DESIGN.md`](DESIGN.md) | The **aesthetic** — the four principles, the reference class, what we refuse |
| [`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) | The **system** — interaction states, the view state vocabulary, motion, elevation, the accessibility floor |
| **This document** | The **mechanics** — the spacing steps, the primitive catalogue, and where a control goes |

[`design/tokens.css`](../design/tokens.css) is the material all three describe.

The tokens already exist and are bridged into Tailwind by the `@theme inline` block in
[`src/index.css`](../src/index.css). What was missing — and what this document supplies — is *usage
discipline* and a small set of components that encapsulate it. This is a conventions + components layer,
not a new-tokens layer.

---

## The one rule: named steps only

**Use the named spacing steps (`px-xs`, `py-2xs`, `gap-sm`, `p-md`…). Never the numeric utilities
(`px-3`, `py-2`, `gap-4`…) for padding, margins, or gaps.**

`src/index.css` deliberately aligns Tailwind's numeric grid to the same 4px base as the named `--space-*`
steps and *also* exposes the named aliases (the `--spacing-*` block in `src/index.css`, which is where
`--spacing` sets the 4px base). So `py-2` and `py-2xs` compile to the
**identical** declaration — they are the same 8px. The named step is the only form that reads as
*deliberate*: it names the role instead of restating a multiple of 4. Numeric spacing utilities are how
drift starts (the same control turning up as `px-3 py-1`, then `py-2`, then `px-lg py-lg` across screens).

> Scope: the rule governs the **spacing roles** below — padding, margin, and gaps between elements. A
> genuinely off-scale one-off that isn't a spacing role (a scroll-height cap like `max-h-64`, a `z-*`
> layer) uses the plain utility; there's no named step to reach for and it isn't where drift lives.

### Role → token-step mapping

| Role | Step | Utility | px |
| --- | --- | --- | --- |
| Control padding (button / select / field) | xs / 2xs | `px-xs py-2xs` | 12 / 8 |
| View gutter | (Layer 4) | `--gutter-view-y` / `--gutter-view-x` | 44 / 60 |
| Field stack gap (vertical) | sm | `gap-sm` | 16 |
| Section gap | lg | `gap-lg` | 40 |
| Inline label ↔ control gap | 2xs | `gap-2xs` | 8 |
| Panel / container padding | md | `p-md` | 24 |
| Tight list gap (sidebar nav rows) | 3xs | `gap-3xs` | 4 |
| Content list rhythm | (Layer 4) | `--gap-row-columns`, in the view's own `*.css` | 28 |
| List row padding | (Layer 4) | `--row-queue-*` / `--row-library-*` / `--row-search-*` | 20/16 · 16/14 · 15/12 |
| Reading / writing column width | (Layer 4) | `--measure-doc` / `--measure-search` | 660 / 640 |

The view gutter is owned by [`ViewFrame`](../src/components/ui/ViewFrame.tsx), so a screen never spells
it out. It is a Layer-4 value rather than a step on this scale (see *Layer 4* below), and it is the same
on every view and on all four sides. The header lead-in is Layer 4 too, but it belongs to the view: each
one applies its own `--lead-*` in its co-located CSS (`.inbox__list`, `.project__index`,
`.attention__stack`).

**A content list's rhythm is Layer-4 geometry, and it is named in `design/tokens.css`.** There is no
shared list gap and no shared row padding: an Inbox row is `--row-queue-y` / `--row-queue-x` (20/16)
inside a shell that carries the matching negative inline margin — written
`calc(-1 * var(--row-queue-x))`, so the bleed provably tracks the pad — that lets its hover plane
reach into the gutter (the row itself is one full-surface button whose `--row-queue-trail` right
padding reserves the overlaid File picker's ground), a `.project__row` is
`--row-library-*` (16/14) and only tints, a `.search__row` is `--row-search-*` (15/12), and
`.attention__stack` separates its already-lifted cards at `--gap-card` (14px).
Those numbers are off the 4px step scale on purpose — what separates rows has to beat what a row
stacks its own lines at, or the list stops reading as rows, and the Inbox once shipped three-line
rows at `gap-3xs` and became one undifferentiated block. Off the step scale is not the same as
unnamed: the numbers live in Layer 4 and are consumed from the co-located CSS, where the three
stances can be read against each other.

Control padding is **`px-xs py-2xs` (12 / 8)**. (The tokens are named by *step*, not by pixel: `--space-sm`
is 16px and `--space-xs` is 12px — so 12px horizontal padding is `px-xs`, not `px-sm`.) The primitives
below bake this in, so most screens never spell control padding out at all.

---

## Bridged vs. non-bridged tokens

Most tokens are bridged into Tailwind utilities and are consumed **as utilities on the component**:

- **Colours** → `bg-bg`, `bg-surface`, `bg-overlay`, `text-text`, `text-text-read`, `text-text-soft`,
  `text-text-faint`, `bg-menu-hover`, `bg-token-active`, `bg-highlight`, `text-accent-dot`…
- **Type sizes** → the fixed-px ramp in [`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §1, from
  `text-eyebrow` (11) up through the four view-title steps (`text-title-panel` … `text-title-doc`)
- **Families** → `font-sans`, `font-serif`, `font-mono`
- **Weights** → `font-medium`, `font-semibold`
- **Tracking** → two ramps: the eyebrow steps (`tracking-eyebrow` / `-eyebrow-menu` / `tracking-rail`,
  plus `tracking-caps` for uppercase micro-text) step on *depth*; the title steps
  (`tracking-title-panel` … `tracking-title-doc`, and `tracking-row`) step on *size*. See
  [`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §1
- **Line-heights** → `leading-body`, `leading-read`, and one `leading-title-*` per title step, which
  must travel with the `text-title-*` of the same name
- **Spacing steps** → `p-*`, `px-*`, `py-*`, `gap-*`, `m-*` with the named suffixes above
- **Radii** → `rounded-sm`, `rounded-md` (tokens.css overrides Tailwind's defaults at `:root`)

> **There are no bridged width utilities.** `max-w-measure`, `max-w-content` and `max-w-wide` were
> real while `--container-measure` / `--container-content` / `--container-wide` were in the
> `@theme inline` block; the redesign split that family into the un-bridged `--measure-*` set and
> the bridge entries went with it. Tailwind emits nothing for an unknown utility and raises no
> error, so those three class names now silently do nothing — a column cap is read from a
> co-located `Component.css` instead (`ViewFrame.css` is the reference; it is where every variant's
> `--measure-*` is applied).

> **Never Tailwind's own `tracking-wide`.** It is 0.025em; the eyebrow token is 0.16em. The Sidebar used
> the token through a CSS class while twelve other call sites used the Tailwind default, so the same role
> rendered 6.4× apart. Bridging `tracking-eyebrow` / `tracking-caps` removed the choice.

Some tokens are **not** bridged. These must live in a **co-located `Component.css`** imported by the
component (the pattern established by `Sidebar.css`, `SpiritMark.css`, and each primitive's own
`*.css`). The un-bridged set:

| Token family | Examples | Why it's in CSS |
| --- | --- | --- |
| Edges / hairlines | the `--edge-*` ladder (`--edge-faint` … `--edge-dot`) | rendered as **inset shadows** |
| Elevation | `--lift`, plus one recipe per plane role (`--lift-card`, `--lift-row`, `--lift-menu`, …) | rendered as `box-shadow` |
| Motion | `--dur-*`, `--ease-*`, plus the reduced-motion switch `--move` and the amplitudes gated on it (`--press-scale`) | `transition` / `animation` shorthands, `@starting-style`, and `transform` values |
| Focus | `--focus-width`, `--focus-offset`, `--radius-focus` | rendered as `outline` |
| Derived recipes | `--wash-active`, `--selection`, `--scrim`, `--scrollbar-*` | composed values |
| Sheen | `--sheen` | specialised |
| Layer-4 geometry | `--gutter-view-*`, `--measure-*`, `--sidebar-*`, `--lead-*`, `--row-*`, `--palette-*`, `--capture-*` | per-view stance, off the step scale |

**Enforced, not aspirational.** [`src/designTokens.test.ts`](../src/designTokens.test.ts) fails any literal
colour, font-family, duration **or spacing value** (padding / margin / gap, in px, rem or em) in a
`src/**/*.css` file, and `eslint.config.js` fails numeric spacing utilities and arbitrary values inside
`className`. Between them, spacing has no unguarded side: eslint owns the class strings it can parse, the
test owns the stylesheets it cannot. Both run in gates CI already runs. The escape hatch is
a `token-guard-allow` comment, which is deliberately greppable — and it must sit on the offending
declaration or in the comment block directly above it, since an intervening line of code (a selector,
say) ends its reach.

Each `Component.css` opens with a banner comment stating that **only non-bridged tokens live there** —
everything bridged stays a utility on the element. Keep that split.

### Two recipes to copy exactly

**Hairlines are inset shadows, never borders.** A hairline is a translucent edge, not a 1px `border`
(docs/DESIGN.md: *space instead of borders and boxes*):

```css
box-shadow: inset 0 -1px 0 var(--edge-faint);   /* a single bottom edge */
box-shadow: inset 0 0 0 1px var(--edge);        /* a full ring */
```

**Focus is one shared class, never retyped.** It lived in eight places before the design-system pass
(four primitive CSS files, one more in `CaptureOverlayPill.css`, and three as inline Tailwind in views).
Add [`ui-focus-ring`](../src/components/ui/ui.css) to the element:

```tsx
<button className="ui-focus-ring …">
```

```css
/* src/components/ui/ui.css — the only definition */
.ui-focus-ring:focus-visible {
  outline: var(--focus-width) solid var(--text);
  outline-offset: var(--focus-offset);
}
```

The ring is **ink, not a hue**. The old interactive-green accent is retired along with the `--accent`
token: this palette has exactly one green and it means audio is being recorded, so a focused control
wearing it would claim something false.

There is exactly one sanctioned exception (the palette input, and nothing else), stated in
[`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §2. Where the focusable element is not the box the user
sees — a chrome-less `<input>` inside a field that already is the chrome — add
`ui-focus-ring-within` to the **wrapper** and leave `outline: none` on the input. It is the same
declaration at a `:has(:focus-visible)` trigger, in the same rule, so the two cannot drift.

**Two more shared recipes live in `ui.css`, and both are one line at the call site:**

```tsx
<span className="ui-tnum">{count} notes</span>   {/* a number that CHANGES */}
<h2 className="ui-balance …">{title}</h2>        {/* a display-step heading */}
```

- **`ui-tnum`** sets `font-variant-numeric: tabular-nums`. Only for a number that changes under the
  user — a count, a workload, a progress caption. The interface sans is proportionally spaced, so
  digits have different widths and a ticking number shuffles everything after it sideways. A date or
  an id sits still and reads better in the figures the face was drawn with, and the mono voice is
  already tabular by construction, so neither restates this.
- **`ui-balance`** sets `text-wrap: balance`, for the display steps only (`ViewFrame`'s three serif
  titles, the note title). Body prose uses `text-wrap: pretty` in its own CSS — `balance` is
  specified to give up after a few lines, so it is a heading rule, not a paragraph one.

**The reserved green (`--accent-dot`) is untouchable.** It belongs to the listening state alone. Precisely:
it means **audio is actually being recorded**. A degraded capture that still has a live source keeps the
green (it *is* recording, and dropping the green would falsely imply privacy), while a capture whose
sources have all dropped out shows no green at all — the label carries the reconnecting state instead.
Capture failure states get no red or amber: there is no such token, and they read through **value**
(`text-text-faint` captions), like every other status line. Selected and highlighted rows likewise read
through value, not hue — an ink wash of the text colour, identical in both themes:

Selected and highlighted rows use the shared `ui-wash` class (or `var(--wash-active)` in CSS), which is
that ink-mix defined once in `tokens.css` — **never `var(--accent-dot)`**.

The reserved green is spent on the SpiritMark and nothing else, including the status label beside it.
As *text* it measures **4.06–4.56:1** against the light theme's three planes, below the 4.5:1 floor
on two of them; as a *graphic* it clears the 3:1 one everywhere in both themes. The label carries
the same state through value instead
([`CaptureStatusLine`](../src/components/capture/CaptureStatusLine.tsx)). The measured figures live once, in
[`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §6 — this line used to quote 3.42–3.70, which were the
*pre-re-tune* green's numbers and had been wrong here since the pigment changed.

---

## Primitives

The shared controls live in [`src/components/ui/`](../src/components/ui/). They are the home for control
padding, the focus ring, and the hairline recipes, so screens compose them instead of restating utility
strings. Named function exports, relative imports, one co-located `*.css` per component.

### `Button` — `variant="primary" | "quiet" | "filled" | "destructive"`

Owns **structure only** (rounding and every interaction state: focus, hover, active, disabled) plus
each variant's emphasis. It deliberately sets **no text size**, so a caller's own `text-*` utilities
never collide with a baked-in one. `primary` is the raised control chip — `bg-surface`, `rounded-md`,
the app's control padding `px-xs py-2xs`, `--fw-medium`, and the ring-plus-shadow of `--lift-chip` in
one declaration; `filled` is an ink fill with a page-coloured label, the heaviest control in the app
and the only one that inverts, spent on the single action that *ends* a surface; `quiet` is a
transparent ghost that inherits its colour — the low-emphasis and navigation form. `destructive` is
not a fourth weight: it shares the quiet ghost's chrome (one CSS rule, two selectors) and exists so
call sites state intent — it may only appear inside a confirmation dialog, as the non-default
control beside a `primary` Cancel (docs/DESIGN_SYSTEM.md, "Destructive is a confirmation, not a
colour").

**Padding follows the variant, not the component.** `primary` and `filled` are real chips with a
fixed size, but a `quiet` button is whatever shape its context needs (a sidebar nav row, a text
action beside a title, a menu item), so it carries no padding of its own and each consumer sets it in
its co-located CSS.

**Hover belongs to the primitive.** Callers used to add their own, in two different destination colours;
there is one hover step and it is toward `--text`.

**So does the press, and it follows the padding rule.** `primary` and `filled` shrink 3% under the
pointer via `--press-scale` — the one state change allowed to move its own box without moving the
layout (docs/DESIGN_SYSTEM.md §2). `quiet` and `destructive` do not, for the same reason they carry no padding: a quiet button has
no box of its own to shrink, and scaling a full-width sidebar nav row moves its edges far more than
it moves a chip's. Never write the scale at a call site; it is one token with one amplitude, and
`src/designTokens.test.ts` fails an `:active` transform that does not use it.

```tsx
import { Button } from "./ui/Button";

<Button onClick={save}>Save note</Button>                        {/* primary */}
<Button variant="quiet" className="text-text-soft">Cancel</Button>
```

**`loading` keeps a pending control mounted *and focusable*.** Swapping a focused button for a
`<span>Saving…</span>` drops focus to `<body>` mid-task — and so does the native `disabled` attribute,
so a busy button takes `aria-disabled` + `aria-busy`, swallows its own activation, and swaps only the
label. `disabled` stays a genuine disable, for a control with nothing to do rather than something in
flight; passing both for the same condition puts the focus loss back:

```tsx
<Button loading={saving} loadingLabel="Saving…">Save</Button>
```

Spreads all native `<button>` props (`type` defaults to `"button"`), forwards `ref`, merges `className`.

### `TextField` — labelled text input

Label stacked a `gap-2xs` above the input; bound by `id` (generated via `useId` when omitted). The input
carries the control padding, a bottom hairline (inset shadow), and the focus ring. Optional `hint` renders
below and is wired through `aria-describedby`.

**No fill — the line is the whole affordance.** `.ui-field` sets `background: transparent`, and the input's
className carries no `bg-*`. It used to carry `--surface` *as well as* the writing line, which made it a box
wearing a line rather than a line; three of them stacked in a form out-weighed everything they labelled. The
field now sits on whatever plane the form sits on, page or panel. Hover still firms the line
(`--edge` → `--edge-strong`) rather than filling the field, `aria-invalid` carries a heavier line, and
disabled drops it to `--edge-faint`.

```tsx
import { TextField } from "./ui/TextField";

<TextField
  label="Title"
  value={title}
  onChange={(e) => setTitle(e.target.value)}
  placeholder="Untitled note"
  hint="Shown in the note list."
/>
```

Spreads native `<input>` props (minus `id`, which it manages).

Pass `error` rather than rendering a message beside the field: the copy and `aria-invalid` have to
travel together, and every hand-rolled version in the app set one without the other.

```tsx
<TextField label="Days to keep" value={days} error={saveError} onChange={…} />
```

### There is no `Textarea` primitive

It existed and nothing ever composed it. Both writing surfaces in the app — the note editor's body
and the quick-capture box — are a raw `<textarea>` carrying an `aria-label`, the `ui-writing` caret
class, `ui-focus-ring`, and their own class from a co-located `*.css` (`.note-edit__body`,
`.capture__input`). That is not an oversight to be corrected: the two want very different room and
very different type (one is the reading ramp at `--fs-read`, the other is `--fs-capture`), and a
primitive whose only job would be to be overridden twice is a primitive that documents a shape
nobody wears. **Copy the raw pattern those two use.**

### `Select` — token-styled dropdown, `variant="boxed" | "token"`

A hand-rolled collapsible listbox (WAI-ARIA active-descendant), **not** a headless dependency — the same
combobox know-how the command palette proves, minus the dep, keeping the app's zero-UI-dependency posture.
That choice was re-tested against base-ui in 2026-07 and held; see
[`docs/decisions/popover-primitive.md`](decisions/popover-primitive.md) for the evidence and the
conditions that would reopen it.
Focus stays on the trigger; ↑/↓ move a virtual highlight via `aria-activedescendant`, Enter/Space selects,
Escape closes and returns focus to the trigger, click-outside closes, and typing jumps (typeahead). The
open list sits on the **overlay** plane (`--overlay` + `--lift-menu`, at `--layer-dropdown`); the active
row is the value wash (never the reserved green).

The list is **anchored to its trigger** with CSS anchor positioning: it hangs off the trigger's right
edge, flips above when there is no room below, escapes a clipping ancestor with no portal
(`position: fixed`), and scales up out of that edge as it opens (`--dur-plane`). All of it sits behind an
`@supports` test for `anchor-scope`, whose base branch is the plain `position: absolute` stance the list
used to carry as utilities — so a WebView2 below 131 renders exactly what shipped before, entrance
included. There is no exit transition: closing unmounts the list. See
[`docs/decisions/popover-primitive.md`](decisions/popover-primitive.md) §5–§6 for the measurements and
the two caveats a reader will otherwise re-discover.

The second of those caveats has a **live consequence worth knowing before you place a `Select`**: the
menu anchors to the *trigger's* border box, not to the wrapper. Where the wrapper shrink-wraps its
trigger — both Settings rows (`justify-self: end` on an `auto` track, `hideLabel`) and the Inbox token
(a flex row) — the two boxes coincide and nothing moved but the token's 10px/5px pill bleed. Where the
wrapper *stretches* and the trigger is narrow, they do not: `ConsentNudge` passes a visible label into
a `flex flex-col` dialog panel, so its 253px menu right-aligns to a 176px trigger pinned at the panel's
left edge and overhangs the panel by 53px onto the scrim. **A `Select` whose wrapper is wider than its
trigger will hang its menu off the trigger, wherever that lands.** This is unresolved, not a documented
intent: [`popover-primitive.md`](decisions/popover-primitive.md) §8.2 has the measurements and the
three candidate remedies.

```tsx
import { Select } from "./ui/Select";

<Select
  label="Project"
  value={project}
  onChange={setProject}
  options={[
    { value: "inbox", label: "Inbox" },
    { value: "research", label: "Research" },
  ]}
  placeholder="Choose a project…"
/>
```

Pass `hideLabel` when the control's purpose is clear from context (a per-row picker in a list): the `label`
stays as the accessible name (`sr-only`) but takes no visual row. The Inbox re-route picker uses this.

Pass **`busy`** while a write is in flight — never `disabled`. They are different states and the
difference is focus: `busy` is `aria-disabled` + `aria-busy` on a control that stays focusable and
declines its own activation, exactly like `Button`'s `loading`, while `disabled` is the native
attribute and blurs a focused trigger straight to `<body>`. `disabled` means "there is nothing here
to choose"; passing both puts the focus loss back, because `disabled` wins. See
[`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §6.

```tsx
<Select label="Retention" value={kind} onChange={setKind} options={…} busy={saving} />
```

Pass `emptyLabel` for what the list says when there are no options — it opens and explains rather
than refusing to open, which used to leave a trigger that swallowed every click.

**`variant` fixes how much the *resting* trigger weighs; nothing else changes.** The dropdown list, the
focus ring, and every keyboard behaviour are identical across both. **The box exists only while
choosing** — that is the idea the two triggers split on.

- **`boxed`** (the default) is the form field and reads like one: `--surface` fill, `--radius-md`, the
  ring-and-shadow of `--lift-chip`, its value at `text-label text-text`, and a chevron. Elevation carries
  all three states and the fill never moves: `--lift-chip-hover` under the pointer, `--lift-chip-open`
  while its menu is down, a bare `--edge-faint` ring when disabled.
- **`token`** rests as quiet mono text with **no box at all** — transparent, `font-mono text-cap
  tracking-token text-text-faint`, and no chevron. It takes a soft `--token-active` pill (at
  `--radius-item`) under the pointer and holds it while open, stepping its colour to `--text`; the pill
  bleeds past the text on a negative margin so nothing in the row shifts when it appears — and its menu
  anchors to that bled box, so the menu hangs off the pill's edge rather than the text's. Its arrow *is*
  the state — `→` at rest, `↓` the moment the menu is under it — and its menu rows render mono, because
  what they list are paths.

```tsx
<Select hideLabel variant="token" label={`File "${note.title}" to project`} … />
```

**`token` is for an in-content affordance sitting beside content it must not out-weigh — never in a form.**
A field that doesn't look like a field is a field people don't fill in. Its only consumer is the Inbox's
per-row *File to…* picker, where boxed made the control the heaviest object on the screen while the note
title was the subject of the row (against DESIGN.md's *space instead of borders and boxes* and *typography
carries the hierarchy*). Settings' two pickers and `ConsentNudge`'s stay `boxed`.

**Never a raw `<select>`.** The native control ignores the token theme entirely (system chrome, no focus
ring, no value wash), so this primitive is the only dropdown.

### `Checkbox` — labelled boolean *(no component consumer, but it owns a live skin)*

> **No component composes this today, and it is kept anyway.** Every boolean the app *ships* is a
> setting that takes effect the moment it moves, so `SettingsView` carries its own local `Toggle`:
> a `<button role="switch">` with `aria-checked` and a knob, styled by `.settings__toggle` /
> `.settings__knob` in `SettingsView.css`. The platform semantics for "this applies immediately"
> and "this is one of several things you are about to submit" differ, and the switch is the first
> of those. Reach for this component only for the second kind, and expect to be its first caller.
>
> What keeps it from being dead code is that **a GFM task list draws the identical skin**
> (`.md-reading input[type="checkbox"]`, `markdownReading.css` — the shared markdown surface the
> note body and the chat's answers both render on). Those two used to be the same
> five literals typed out in two files, invisible to the token guard — which scopes its spacing
> check to padding/margin/gap, so a width or a ring thickness was structurally unseeable. Both now
> read the Layer-4 `--check-*` family (`--check-box`, `--check-ring`, `--check-mark-w`,
> `--check-mark-h`, `--check-mark-stroke`), so the skin is defined once. Editing one without the
> other is the drift the tokens exist to prevent.
>
> The note-body copy renders **`disabled`** — `mdast-util-to-hast` hard-codes it on a task-list
> item — so it takes no hover, no active and no pointer cursor. This primitive, which is genuinely
> interactive, does.

A real `<input type="checkbox">` under a token skin (`appearance: none`), so keyboard behaviour, form
semantics, and screen-reader state come from the platform rather than re-implemented ARIA — the opposite
call from `Select`, where no native control could carry the theme at all. The label sits inline a `gap-2xs`
beside the box and is bound by `id` (generated via `useId` when omitted); an optional `hint` renders below,
indented to the label column and wired through `aria-describedby`. `onChange` receives the new boolean, not
the event.

```tsx
import { Checkbox } from "./ui/Checkbox";

// Illustrative, and deliberately not a shipped control: every boolean the app
// ships today is a `Toggle`. This is the shape the second kind would take — one
// of several things you are about to submit.
<Checkbox
  label="Include the raw transcript"
  hint="Attaches the verbatim transcript alongside the distilled note."
  checked={includeTranscript}
  onChange={setIncludeTranscript}
/>
```

The checked state is ink on surface — **value, not hue**. The reserved green is never used here: it means
audio is actually being recorded, and a settings control wearing it would be claiming something false.

### `ViewFrame` — the page scaffold, `variant="queue" | "library" | "panel" | "health" | "doc" | "search" | "terminal" | "chat"`

The view gutter, the content column, and the eyebrow/title header. Every full view sits in one. The
gutter (`--gutter-view-y` / `--gutter-view-x`) is the same on every variant and on all four sides; see
*Layer 4* below for why that is not negotiable per view.

**`variant` answers "what am I looking at" before the heading is read.** It is not decoration — it is
the one place a view declares its kind, so two views of the same kind can't drift apart. **It is a
required prop.** There is no default and no bare scaffold: a new view has to say what kind of place it
is. The eight, with what each fixes:

| `variant` | The view is | Column cap | Title step | `summary` renders as |
| --- | --- | --- | --- | --- |
| `queue` | work to get through | none | none — a one-line masthead | `· 4 to file`, inside the masthead at `text-text-faint` |
| `library` | a place to browse | none | `text-title-library` (34) | `text-label text-text-faint` — a quiet count ("12 notes") |
| `panel` | configuration | none (its rows cap themselves) | `text-title-panel` (26) | not rendered |
| `health` | system state to recover from | none | `text-title-health` (28) | `text-label text-text-faint` |
| `doc` | a note | `--measure-doc` (660) | header supplied by the view | not rendered |
| `search` | results under a pinned query | `--measure-search` (640) | header supplied by the view | not rendered |
| `terminal` | the embedded Claude Code terminal | none — a full-height pane that scrolls inside itself | `text-title-panel` (26) | not rendered |
| `chat` | the designed chat over the knowledge base | `--chat-measure` (660) — the terminal's full-height stance on a doc's measure | `text-title-panel` (26) | not rendered |

The "Title step" column names the size, but a step is a **triple**: `ViewFrame` emits
`text-title-x` with the `leading-title-x` and `tracking-title-x` that tighten alongside it
([`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §1). A 26px serif title spelled anywhere else — a
dialog heading, say — takes all three, or it renders at body leading with no tracking.

```tsx
<ViewFrame variant="library" eyebrow="Project" title={formatSlug(slug)}
           summary={`${notes.length} notes`} action={<Button …>New note</Button>}>
  {…}
</ViewFrame>
```

**A queue's masthead is one line, not a stack.** `queue` is the one variant that does not render a
serif title: the view's name and the amount of work in it belong to the same sentence
("Inbox · 4 to file"), and splitting them across a title and a subtitle made a short list of chores
look like a chapter opening.

**`doc` and `search` render no header of their own.** Their headers are a genuinely different shape (a
back link and its own actions; a query field), so they arrive as children. Passing `eyebrow`/`title` to
either is not an error and not a look anyone has designed — pass the header as a child instead.

**`action` is one node, and it is one action.** A single header-level control, right-aligned in the
header opposite the title block: the one thing the view is for. It is not a container — a caller with
two passes a flex `<div>` and the type says nothing, which is how `ProjectView` came to carry two. Which
slot an action belongs in, and how many a surface may hold, is *Composition* below.

**And it is only a prop on the six variants that draw a header.** On `doc` and `search` it is a **type
error**, for the same reason `summary` is on the three variants below. It used to sit in `BaseProps`, so
those two accepted it — and since neither passes an `eyebrow` or a `title`, `renderHeader` returned
before it reached the action and dropped it on the floor. (The guard is `!eyebrow && !title`, not a
variant check, so strictly it drops the action on *any* caller that passes no header content; every
shipped call site on the other six passes a title, which is why `doc` and `search` are the two the type
now excludes.) A view that draws its own header puts its own actions in it.

**Note the asymmetry that remains, on purpose.** Passing `eyebrow`/`title` to `doc`/`search` still
compiles, because those two render rather than vanish — a `doc` caller that passes a title gets an `<h2>`
with no type step on it at all (`TITLE_CLASS.doc` is deliberately `""`) in a header nobody designed,
which is a visible mistake rather than a silent one. `action` and `summary` are the props with nowhere to land at all, and
those are the two the compiler now catches. Separately and not fixed here: because neither variant
passes either, `NoteEditorView`'s and `SearchView`'s `<section>` carry no accessible name and are not
exposed as regions — a real gap, flagged in the component, wanting its own decision rather than a side
effect of this one.

**`summary` is not a free styling slot.** The variant fixes its typographic role, so a workload sentence
can never render at a count's weight in one view and a heading's in another. Call sites pass the content,
never a class — there is no `className` on it to reach for.

**And it is only a prop on the three variants that draw one.** `queue`, `library` and `health` accept
it; on `panel`, `doc` and `search` it is a **type error**, not a silent no-op — there is no typographic
role for one on those three at all, so the props are discriminated on `variant` and the compiler says so
at the call site. It used to be accepted, dropped at render, and reported nowhere. `action` above is the
same fix for the same reason, on the narrower set of variants that draw no header; the asymmetry with
`eyebrow`/`title` is spelled out there.

**Omit `summary` at zero.** The empty `StatusMessage` in the body already says there's nothing here, and
two "nothing here" voices in one header is one too many. Every call site does this with a conditional
(`notes.length > 0 ? … : undefined`).

**Only `doc` and `search` cap their column at all.** `queue`, `library`, `panel` and `health` cap
nothing: their content is rows, not prose, so it takes the width the gutter leaves it — the same width
on every one of those views. Where a line length genuinely hurts, the cap goes on the thing that needs
it rather than on the frame: a queue row's serif snippet stops at `--measure-snippet`, and Settings caps
the block holding its rows at `--measure-setting` (`.settings__rows`, 520) so the tab rail above it can
still run the pane's full width. A panel view therefore does **not** wrap its own blocks in a width
utility — the cap sits on the one block that needs it, in the view's own CSS.

### `StatusMessage` — `variant="empty" | "error" | "status"`

The one way a view says "nothing here", "that failed", or "working on it". **The variant fixes the ARIA
role**: `error` → `role="alert"`, `status` → `role="status"`, `empty` → none. That binding is the point —
before it, `role="alert"` was on some async failures and not others. `compact` steps the type down to
`text-cap` for a row-level message.

```tsx
<StatusMessage variant="error">Couldn&apos;t load notes: {error}</StatusMessage>
```

### There is no `ListRow` primitive

It existed and nothing ever composed it. **A row is the view's stance, and the whole redesign turns
on that**: `.inbox__row` lifts onto the raised plane when you touch it (these are items you clear),
`.project__row` only tints (a reading room, nothing is waiting on you), and `.attention__card`
arrives already lifted with no hover at all (it flagged itself). One affordance, three meanings —
which is exactly the difference a single shared row was flattening. Each list-bearing view draws its
own from a co-located `*.css`.

It also carried the bug it claimed to have fixed: its hover recoloured the inner title span while
the ring sat on the outer element, with no `:focus-visible` counterpart — the very failure its own
doc comment cited. **Hover and the focus ring live on the same element** is the rule that outlived
it; it is stated in [`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §2, where it applies to everything
rather than to one unused component.

Build a row's meta string with [`noteMeta`](../src/noteMeta.ts), which takes the surface's own
middle segments: `noteMeta(note, noteKind(note.type))` (ProjectView),
`noteMeta(note, noteKind(note.type), matchScore(note.confidence))` (InboxView), and
`noteMeta(note, note.type)` (NoteEditorView). Falsy middles are dropped, so a helper may return
`null` rather than making every caller branch.

A **list** row names its kind through `noteKind`, which returns `null` for a plain `note` — every
note is one until proven otherwise, so the word is noise on most rows and information on a
`meeting` or a `chat`. The **single-note** surface passes `note.type` straight through instead:
one note fills the view, so its kind is worth stating unconditionally.

### `Overlay` — the modal shell

Scrim, stacking layer, backdrop dismissal, and the raised panel. Dismissal fires on click (not
pointerdown) and only when the gesture both started and ended on the backdrop — two guards that existed
twice, byte for byte, before this primitive.

Focus trapping is deliberately **not** here: the palette holds focus on its lone input by swallowing Tab,
the consent nudge wraps Tab across several controls. Each passes its own `onKeyDown`.

```tsx
<Overlay onDismiss={onClose} label="Command palette" className="overflow-hidden">{…}</Overlay>
```

### `DestructiveConfirmDialog` — the destructive-confirmation shell

The shared shape of a destructive confirmation (`docs/DESIGN_SYSTEM.md` §2): the action is marked by
**confirmation, not colour**. It composes `Overlay` + `useDialogFocus` + `wrapDialogTab`, wires Escape and
the Tab-trap, and renders the title, a body slot, an error slot (`StatusMessage`), and the footer — the
`destructive` confirm beside a `primary` Cancel that holds initial focus. This is the boilerplate
`DeleteProjectDialog` and the Needs Attention capture delete shared byte for byte, factored into one place.

It is deliberately **presentational**: each caller keeps its own async handler, `busy`/`error` state,
success behaviour, and error copy, and passes the results down. It never closes itself on confirm — the
caller decides what success means (navigate away, refetch, unmount).

```tsx
<DestructiveConfirmDialog
  title={`Delete ${slug}?`}
  confirmLabel="Delete project"
  busyLabel="Deleting…"
  busy={deleting}
  error={error}
  onConfirm={confirm}
  onClose={onClose}
>
  <p>Its notes will move back to the Inbox.</p>
</DestructiveConfirmDialog>
```

---

## Interaction conventions

Beyond spacing and primitives, a few consistency rules for any screen:

- **Type floor.** The smallest named steps (`text-eyebrow`, `text-cap`) are for labels, captions, and
  eyebrows only; readable body copy uses `text-body` or larger. Sizes always come from the named scale —
  never a hard-coded `text-[13px]` (same reasoning as the named-spacing rule).
- **No hover-only affordances.** Every action is reachable by keyboard and discoverable without hovering;
  the primitives' `:focus-visible` ring is the baseline. Hover may *enhance* an always-visible control, but
  it must never be the only way to reveal or trigger one.
- **`data-testid` discipline.** Interactive elements a screen adds — and any element whose text an
  assertion reads — carry a stable kebab-case `data-testid`, so the end-to-end harness in
  [`e2e/`](../e2e/README.md) can select them without depending on copy or DOM shape. Only `Button`,
  `TextField` and `Checkbox` spread native props, so only those pass a `data-testid` down to the
  underlying element. `Select`, `ViewFrame`, `Overlay`, `StatusMessage` and `DestructiveConfirmDialog`
  take closed prop sets and reject it as a type error — put the id on a surrounding element instead.
- **No component-level `useEffect`.** External-system glue (focus hand-off, scroll-into-view,
  outside-press dismissal, timers, Tauri events) comes from the blessed bridge hooks in `src/` — the
  primitives here compose those hooks, derive during render, and act in event handlers. See
  [`.claude/rules/no-use-effect.md`](../.claude/rules/no-use-effect.md); eslint enforces the list.
  An entrance animation is not the exception it looks like: `@starting-style` in the co-located CSS
  animates a first paint with no mount flag and no effect, so it needs no hook at all
  ([`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §4).

---

## Composition — where a control goes

`design/tokens.css` is exact about how far a line of text may run: a note's column stops at
`--measure-doc` (660), a queue row's serif snippet at `--measure-snippet` (500), a settings row at
`--measure-setting` (520). Nothing anywhere says how many controls may sit beside that column.
Reading density is governed to the pixel; control density was governed per view, one view at a time,
which is how a single screen came to carry four separate places to look for something to press.
This section is the other half: **which slot an action goes in, and how many a surface may hold.**

### The shell has two regions, and a view fills one

`AppShell` renders the `Sidebar` beside a single `<main>`, with the capture toast, the command palette
and the consent nudge overlaid on top rather than docked beside. `MainContent` is a flat switch and all
eight destinations render into that one main slot. **There is no inspector, no split, and no third
rail.** A view that needs more room takes depth, not width.

That is a decision, not an accident of what got built first. *Layer 4* below records that per-view
gutters were tried and removed because a left edge that moves between destinations reads as the layout
being unstable rather than as two places being different kinds of place. A region that some destinations
have and others don't is the same failure at the right edge: it makes the main column's width depend on
where you navigated. The two candidates people reach for are already answered by stacking — a note's
metadata is one `noteMeta` line under the title, leading the body at `--lead-doc`, and
`SessionArtifactsSection` sits under the body behind a single hairline, which its own stylesheet calls
*chrome below the document, not part of it*.

**What would overturn it:** a majority of destinations wanting the *same* persistent secondary content,
which has to stay visible while the main column is being used. One line of metadata is not that, and a
recording you play once is not either. Until then, "more than fits" is answered by a summoned surface
(`Overlay`, the command palette) or a disclosure. If that day comes it is a `ViewFrame` variant change
and it is named here — not discovered by whichever view runs out of room first.

### The six slots

**A view's actions sit in one of six places**, and the slot is chosen by what the action *acts on* —
never by where there happened to be room.

| Slot | Acts on | Ceiling |
| --- | --- | --- |
| **Frame header** — `ViewFrame`'s `action` | the view: the one thing you came here to do | **one** |
| **View-owned header** — `doc` and `search` only | the open document, or the query | **one cluster** |
| **Contextual chrome** | whatever summoned it — a selection, a pending prompt | no count; *one job* |
| **Row affordance** | one item in a list | **two** |
| **Footer / composer** | the surface as a whole: commit it, or abandon it | **two** |
| **Disclosure** — the toggle that summons a subordinate section | content stacked below the view's subject | **one per section, and it does not nest** |

**Four kinds of control are deliberately outside that list.** Getting around is not acting — a back
link, and Settings' tab rail, which *filters* the pane rather than navigating (`role="tablist"`, so a
screen reader announces it as a filter and not as a second set of destinations competing with the
sidebar). Affordances *inside* the content belong to the content: a note's tag chips, a recording's
`<audio>` player. A control a **view state** raised belongs to that state and not to the frame,
whether it recovers or announces — `TerminalView`'s Restart, `ChatView`'s Start a new chat,
`AppErrorBoundary`'s Try this screen again and the Inbox's filed toast (a success, not a recovery)
all sit inside a state block, and the vocabulary for those is
[`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §3. And the `Sidebar`'s New project button is global, so it
lives in the other region entirely.

**A frame header takes one action, because that is what the prop is.** `action` is a single node,
right-aligned in the header opposite the title block. A caller with two reaches for a flex `<div>` and
the type never notices — which is exactly how a ceiling erodes: not by a decision, but by there being
nowhere to record one.

**`doc` and `search` draw their own header, and it is still one cluster.** They render no frame header
at all (see `ViewFrame` above), so the discipline is kept by hand: whatever the surface can do goes in
the title row, in one group, rather than spreading down the column. The back link sits on its own line
*above* that row, and Layer 4 gives the gap a token of its own (`--lead-doc-title`, 18), because the way
*out* of a document reads differently from the things you can do *to* it.

**Contextual chrome is summoned by state and never parked.** The note editor's format toolbar is the
pattern: it exists only while text is selected, anchored just above the selection rather than parked in
a bar at the top of the screen, which is why it can carry five tools without reading as clutter. You
summoned it, it does one job, and it leaves. The chat's inline permission card is the same slot in a
different shape — a card in the log flow rather than a floating bar, raised where the exchange that
asked for it sits, and gone once you answer. What disqualifies a control from this slot is being there
when nothing summoned it: a bar that is simply always present is a frame header that grew.

**Two is a row's ceiling, because a row is scanned rather than read.** No list in the app exceeds it and
the two that reach it stop there: an Inbox row overlays Delete and its File picker, and a Needs
Attention card carries Retry and Dismiss (its dismissed shelf, Restore and Delete). Most carry fewer — a
project or search row carries none at all, because the whole row is the button. A third affordance would
take its width from the title, which is the row's actual subject.

**A footer holds one control that commits the surface and one that abandons it.** `CreateProjectDialog`
pairs a `quiet` Cancel with the `filled` Create that the `Button` entry above reserves for the action
ending a surface. `DestructiveConfirmDialog` inverts the emphasis instead — the `destructive` confirm
first, a `primary` Cancel beside it — because there the *non-default* control is the one that acts
([`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §2). A composer is the same slot on a surface you do not
leave: the chat's Send commits the draft and comes back for the next one, and Stop replaces it while a
turn is running rather than sitting beside it. Two controls is what this section fixes; which variant
wears which weight, and in what order, is the `Button` entry's business and §2's.

**A disclosure is one toggle, and the section behind it is where its controls go.** This is the slot
that answers "more than fits" above, and it is the one a view reaches for when a subordinate section
has to sit under the subject rather than beside it. **The toggle is the section's whole control footprint
at rest**: the section's own actions live *inside* what it opens, next to the thing they act on, and a
second toggle nested in there would put the reader back to two places. `NeedsAttentionView`'s dismissed
shelf is the shape — one collapsed line, `Dismissed · N`, with Restore and Delete on the rows it
reveals — and a note's source pairing is the same slot below a document: one `Source · recording · 3
segments` line, with the recording (and its Reveal), the player, and the transcript all behind it. The
toggle states in its own label what opening it will show, because a control whose only signal is a
chevron makes the reader click to find out whether there was anything there. And the disclosure *is*
the cap on what it holds: a section that only exists once asked for cannot out-measure the subject the
way an always-open block below it would (§1 of [`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md)), so what it
reveals does not additionally need a scroller.

### A dependent setting is grouped, not just listed

The six slots above govern where an *action* goes. This is the same question one level down, for a
surface made of settings rather than actions: **what expresses that one option is subordinate to
another.** `SettingsView` renders every setting through one `Row` on one grid, which is exactly what
makes its column scannable — and it is also what flattens rank. Two clusters were lying about their
structure: a day count that means nothing without the retention policy above it, and two toggles that
are two halves of one concept. At one indent on one grid, a dependent option sits at exactly the rank
of an independent one, and the reader is left to work out which is which.

**A cluster becomes a `role="group"` carrying an `aria-label`, and the rows inside it indent one `sm`
step.** The group's name is what a screen reader announces on entry, and that is what lets a nested
row's own label be short: `Days`, not `Days to keep`.

**Two shapes, chosen by one question: does the concept already own a control?**

- **It does** — the row carrying that control heads the group, and the group's label is only its
  accessible name. It takes `hideLabel`, the same prop `Select` takes for the same reason. Retention
  is this shape: `Retention` with its `Select`, and `Days` indented under it.
- **It does not** — the group draws its label as a visible heading row (`--fw-medium` + `text-text`,
  per [`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §1) and the rows under it are *peers* at one indent.
  `Capture pill` is this shape: it is a concept, not a setting.

**An indent is a mapping claim, so it has to be true.** That is why the headless shape has to exist.
The two pill toggles are independent booleans — `manual_captures` and `auto_captures`, and a test
pins that they move independently — so nesting the auto one under the manual one would tell the
reader that switching the pill off silences the auto pill too. It doesn't. A shape that reads well
and says something false is worse than the flat list it replaced.

**The indent comes out of the label column, never the control column.** A nested row still spans
`--measure-setting`, so the `1fr` label track absorbs the `padding-left` and the `auto` control track
stays flush to the same right edge. Settings' one scannable column of controls is the view's
identity; hierarchy is not allowed to cost it.

**A group adds no control.** A disclosure is the obvious alternative and it is refused here:
`SettingsView` sits *exactly* on the four-control ceiling in the table below, and an expander is a
fifth. Rank comes from indent, proximity and the group semantics — never a left rail, a border or a
zebra ([`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §1, *a list is not a table*), and never a fourth
eyebrow tracking step (§1's eyebrow ladder is three deep and stays three deep). Proximity is bought
with air *around* the group; the rhythm inside it stays the panel's own `--row-panel-y`, because a
nested row is a whole setting and a shorter box would claim it is not.

**Grouping does not move the count.** The two pill toggles now sit in one named group and
`SettingsView` is still 1 cluster, 4 controls. A *cluster* in that table is a **place you look** —
which is what makes `NoteEditorView`'s four the problem it is — and a group is rank *inside* one
place, not a second place. Counting it would inflate every view that ever groups two rows, and would
penalise the change that made the screen easier to sweep. That is the point of preferring a group in
the first place: a screen already on the ceiling can still afford the kind of hierarchy that adds
nothing to press.

**A control's accessible name is its visible label, verbatim.** Once the group carries the context,
the toggle's `label` (which becomes its `aria-label`) says exactly what the row says —
`Captures you start`, not `Show the capture pill during captures you start`. Those two used to
diverge on both pill rows, which leaves a voice-control user guessing between two names for one
switch. If a control still needs a long accessible name after grouping, the group is named wrong.

**A label written for a group is not a label written flat.** `ConsentNudge` keeps `Days to keep` on
purpose: its dialog is a flat column of fields with no group to lean on, so the field has to carry
the whole meaning itself. Copying the short form there would copy the shape without the thing that
makes the shape work.

### How much one surface may hold

**At rest, a view shows at most three clusters and four controls** — counting one representative row
affordance, and not counting contextual chrome. *At rest* means what you meet on arrival: nothing
selected, no dialog open, no shelf expanded.

**The count is every control that acts, wherever it sits — not every control in a slot.** Being held
outside the slot list does not exempt a control from the count: a back link is one place a reader has
to look, and it counts. A **collapsed disclosure counts as its one toggle and nothing behind it**, which
is the whole point of the slot. Three things are excluded, each for a reason that is not "it did not fit
the table": contextual chrome, because it is not there until summoned; affordances that belong to the
content rather than acting on it (a recording's `<audio>` transport, a tag chip's `×`); and Settings'
tab rail, which indexes the pane rather than acting on it — the same reason it is not in the slot list
and the Layer-4 table says it "filters, it doesn't act". Without that stated the table below is
unreadable: a reader who exempts the back link scores the note editor a compliant 3/4 in exactly the
state that prompted this section, and one who counts the tab rail scores Settings over it.

The number is read off what already ships rather than picked:

| Surface | Clusters | Controls at rest |
| --- | --- | --- |
| `InboxView` | 1 | 2 — a row's Delete and its File picker |
| `ProjectView` | 1 | 2 — New note, Delete project |
| `NeedsAttentionView` | 2 | 3 — a card's Retry and Dismiss, plus the shelf toggle |
| `SettingsView` (its heaviest tab, Capture) | 1 | 4 — two pill toggles in one group, Run test, Rebuild |
| `SearchView` | 0 | 0 — the query field is the surface |
| `TerminalView` | 0 | 0 — Restart appears only once the session has exited |
| `ChatView` | 1 | 1 — the composer's Send, which Stop replaces mid-turn |
| `NoteEditorView` (reading a session note) | 3 | 4 — the back link, Edit and Delete note, plus the source disclosure |

Seven of the eight sat at or under it without ever having been told to, which is the evidence that the
number is this app's own rather than an import. Settings sits exactly on it. The eighth is why this
section exists, and it now sits on it too — see *Going over* below for what moved.

Counting *at rest* is what makes that table honest, and it is why a file-wide count of `<Button` is a
different measurement — that one scores `ChatView` and `NeedsAttentionView` at five each. Most of those
never share a screen: Send and Stop are a ternary, the composer and the exited view's Start a new chat
are exclusive, and the dismissed shelf stays collapsed until you open it. The rest is contextual chrome,
which this ceiling excludes by design — the chat's permission card really does sit in the log above a
live composer, and it is meant to.

**The ceiling is a smell test, not a lint rule — and it is stated as a number anyway.** Nothing can
enforce it: [`src/designTokens.test.ts`](../src/designTokens.test.ts) reads stylesheets and
`eslint.config.js` reads class strings, and neither can count controls. But a number you can hold a
screen up against still beats "keep it simple", which is what governed this until now — and which is how
one screen reached four places to press without anyone ever deciding it should.

### Going over, and the one that already has

Going over is allowed and it is not free: **record it here, name the constraint that forces it, and name
what expires it.** That is the documented-departures habit below, plus the expiry — those three record
what forces them and stop there, and a ceiling is the kind of rule that needs the second half, because
an exception justified by "there is only one other control here" stops being true the moment a third
arrives ([`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §2 states that doctrine for focus rings). An
exception with no recorded reason is indistinguishable from an oversight six months later, and that is
the whole cost this section exists to avoid paying twice.

**One sanctioned exception: `ProjectView`'s header carries two.** New note and Delete project sit in a
frame header typed for one. Both are quiet text rather than control chips, so neither out-weighs the
title beside them; creation leads, and the destructive one needs no weight of its own because it sits
behind `DeleteProjectDialog` and the confirmation is what marks it
([`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §2). **It expires if a third verb appears, or if either one
stops being quiet text** — at that point the header is a toolbar and should say so.

**`NoteEditorView` was over, and that case is closed.** Reading a distilled session note whose audio and
transcript both survived retention used to put five controls in four places: the back link, Edit and
Delete note in the title row, then Reveal in Explorer and the transcript disclosure as separate rows
down in `SessionArtifactsSection`. The count was never really the problem; four *places* was. What
landed is the *Disclosure* slot above: the whole source pairing now sits behind one
`Source · recording · 3 segments` toggle, so the note reads at three places and four controls.

Be precise about how that arithmetic works, because the wrong lesson is easy to take from it. **Nothing
merged.** Reveal in Explorer still exists, unchanged; the transcript's toggle became the Source toggle,
widened to stand for the whole section. What changed is that **Reveal stopped being a resting control**
and became a summoned one, which the counting rule above licenses exactly ("a collapsed disclosure
counts as its one toggle and nothing behind it"). So the drop from four *places* to three is the real
win here, and the control count followed it.

**And moving the `<audio>` player bought nothing at all.** A player is an affordance that belongs to the
content rather than acting on it, excluded from the count above, so it never scored in the first place.
It moved because the recording and the transcript are one artifact used together — a turn's `m:ss`
offset exists so the two can be read against each other — and grouping is its own justification. The
distinction matters for the next change: hiding a **control** is what moved this number, under a rule
that says so out loud; hiding **content** moves nothing, and no future change should cite this one as
though it did.

**Reveal in Explorer survived the cut, and that was close.** Deleting it reaches the same 3/4 with a
smaller change, and the ticket invited removal. It stayed because a session recording lives at
`sessions/<ISO>-<hash>-<slug>.wav`, which is not a path anyone navigates to by hand, and it is the only
bridge from the app to a file `docs/FOUNDING_DOC.md`'s plain-folder posture says is the user's. The
honest counter-evidence, recorded so the next reader does not have to rediscover it: **a note's own
`.md` has no reveal control**, so the app does not offer reveal systematically. **This expires if reveal
gets a home that covers notes too** — at which point the recording's copy belongs there instead.

**One accepted regression, named so it can be revisited.** Collapsing the disclosure unmounts the
`<audio>`, so closing the source stops the source, and playing a recording while reading the note below
it no longer works. Closing a thing stopping that thing is the honest reading, and the alternative
(keeping the panel mounted behind `hidden`) trades it for a live but invisible media element. **It
expires if that turns out to be how people actually listen.**

The same change made the frame's half of this mechanical rather than advisory: **`action` is now a type
error on `doc` and `search`**, the two variants that draw no header, so a view cannot quietly hand the
frame an action and get nothing (see the `ViewFrame` entry above). `ProjectView`'s two-button header is
therefore the only surviving exception in this section.

---

## What consumes these today

Every shipped surface composes the primitives for its controls and its frame, and several draw their own
row or writing surface on top. The table is the live inventory: the middle column is what a surface
imports from `src/components/ui/`, the right-hand one is what it draws itself in a co-located `*.css`. A
new screen should find its shape here rather than inventing one.

| Surface | Composes | Draws itself |
| --- | --- | --- |
| `Sidebar` | `Button variant="quiet"` rows (including the needs-attention row), `StatusMessage` | the nav rows' geometry (`Sidebar.css`) |
| `InboxView` | `ViewFrame variant="queue"`, `Select variant="token"`, `StatusMessage` | `.inbox__rowShell` + `.inbox__row` (one full-surface button with the File picker overlaid in `.inbox__rowActions`), the progress instrument, the pipeline placeholder row (which wears the same shell and row shape), and the filed toast (`.inbox__toast`) |
| `NeedsAttentionView` | `ViewFrame variant="health"`, `Button` (with `loading`), `DestructiveConfirmDialog`, `StatusMessage` | `.attention__card`, pre-lifted |
| `ProjectView` | `ViewFrame variant="library"`, `Button`, `StatusMessage` | `.project__row`, a hand-rolled index row |
| `CreateProjectDialog` | `Overlay`, `TextField`, `Button` (`quiet` + `filled`) | — |
| `DestructiveConfirmDialog` | `Overlay`, `Button` (`destructive` beside a `primary` Cancel), `StatusMessage` | — |
| `DeleteProjectDialog` | `DestructiveConfirmDialog` | — |
| `NoteEditorView` | `ViewFrame variant="doc"`, `Button`, `StatusMessage` | its own header, and raw `<textarea>` / `<input>` elements with `aria-label` + `ui-writing` |
| `SessionArtifactsSection` | `Button variant="quiet"` (one disclosure toggle, one Reveal), `StatusMessage` | the disclosure below a document (`.session-artifacts`, one hairline and one summoned panel) and the transcript's three-column turns; the recording is a native `<audio controls>` |
| `SettingsView` | `ViewFrame variant="panel"`, `Select`, `Button`, `StatusMessage` | a local `role="switch"` `Toggle`, a local `Group` (`role="group"` + `aria-label`, `.settings__group`) in both its shapes — headless over Retention, headed over the two pill toggles — and a raw number `<input>` (`.settings__chip`) |
| `SearchView` | `ViewFrame variant="search"`, `StatusMessage` | its own query field (`.ui-focus-ring-within`) |
| `TerminalView` | `ViewFrame variant="terminal"`, `Button` | the xterm mount and the session-ended notice (`TerminalView.css`) |
| `ChatView` | `ViewFrame variant="chat"`, `Button`, `StatusMessage` | the conversation log (`.chat-view__log`, entries on the shared `.md-reading` surface), the inline permission card (`.chat-view__card`, `ui-raised`), and a raw `<textarea>` composer (`.chat-view__input ui-writing`) |
| `AppErrorBoundary` | `ViewFrame variant="health"`, `Button`, `StatusMessage` | — |
| `CommandPalette` | `Overlay` | its own combobox input and rows |
| `ConsentNudge` | `Overlay`, `Select`, `TextField`, `Button`, `StatusMessage` | — |
| `QuickCapture` | `Button`, `StatusMessage` | a raw `<textarea>` (`.capture__input ui-writing`) |
| `ListeningIndicator` | `CaptureStatusLine` | its own dot and waveform |
| `CaptureOverlayPill` | `CaptureStatusLine`, `SpiritMark` | the pill window |
| `CaptureToast` | — | the overlay plane directly (it is not a modal, so not `Overlay`); failures only — progress and the one success case (filed elsewhere) live in the Inbox's own placeholder and toast |

The right-hand column is not an accusation. A view drawing its own row is the redesign working as
intended: the row is where a queue, a library and a health view differ most, so each one states that
difference in its own CSS.

**Every primitive in `src/components/ui/` now has at least one call site.** `ListRow` and `Textarea`
did not, and are gone (see the two notes above for what replaced them, and why nothing should
resurrect them); `PlaceholderView` was never routed and is gone with them. `Checkbox` is the one
remaining primitive with no *component* consumer, and it is kept deliberately: a note's GFM task
list draws the identical skin, and both now read it from the shared `--check-*` tokens, so the
component is where that skin is defined rather than a second copy of it. Check this table before
assuming a primitive is the shape the app actually wears.

Three surfaces keep a documented departure, each forced by what the window is rather than by preference:

- **`CommandPalette`** — its input keeps a bespoke treatment (no focus ring, its own inset hairline,
  `role="combobox"`). It is a search-combobox, not a generic form field. The shell around it is `Overlay`
  like any other modal. Its own rhythm is the modal's, not a control's, and it is Layer-4 geometry in
  `CommandPalette.css` rather than utilities: the query row is `--palette-query-*` (20/22/18) and the rows
  are `--palette-row-*` (11/13) — a panel people read a list in, indented off its own edge rather than
  padded like a button.
- **`QuickCapture`** — a hotkey-first window that now also carries a visible **File it** button, because a
  pointer-only user cannot press Enter into a window they never focused. Adding it is why the textarea
  regained its focus ring: Tab finally has somewhere to go.
- **`CaptureOverlayPill`** — `rounded-full` rather than a radius step, because the pill's rounded edge *is*
  the window's apparent shape (the window is `transparent: true`) — the genuine off-scale one-off the
  named-steps rule allows. Its window drag stays mouse-only: the window is `focusable: false` so
  appearing over a full-screen app never steals focus, which also puts it out of the tab order. The
  keyboard paths that matter remain — the capture hotkey stops the capture and the pill with it, and the
  Settings toggle turns it off for good.


---

## Layer 4 — the per-view geometry

The redesign gives each **view type** its own stance: how dense it is, what its
header looks like, and where the weight falls, so a queue and a library are
told apart before any heading is read. Those measurements do not land on the
4px step scale, and they must not.

So they are a **fourth token layer** in `design/tokens.css`: `--sidebar-*`,
`--gutter-view-y` / `--gutter-view-x`, `--measure-*`, `--lead-*`,
`--palette-top`, and the radius ladder — plus the per-view row stances
(`--row-*`, `--card-pad-*`, `--gap-row-columns`), the control insets
(`--chip-pad-y`, `--btn-filled-x`, `--tag-*`, `--toolbar-*`, `--token-pill-*`,
`--field-search-y`, `--mark-pad-*`), the Settings panel's own furniture
(`--gap-tab`, `--tab-pad-*`, `--gap-setting`, `--sublabel-tuck`,
`--gap-setting-group`), the two
boolean controls' geometry (`--check-*`, `--toggle-*`), the writing surfaces'
floors (`--measure-body-min`, `--measure-capture-min`) and the summoned
surfaces (`--palette-*`, `--menu-*`, `--capture-*`).

**The guards cannot see a width, a height or a border thickness.** The token
test scopes its spacing check to `padding` / `margin` / `gap`, and eslint only
reads class strings — so a `width: 17px` in a stylesheet passes both. That is
how the checkbox skin came to be written out twice and the toggle's five
numbers once, unnamed. `--check-*` and `--toggle-*` exist because the rule is
the same whether or not a guard can enforce it, and `--toggle-travel` is
written as a `calc` off the other three so a knob provably cannot drift out of
its track — the same discipline as the Inbox row's `calc(-1 * var(--row-queue-x))`.

**Off the step scale is not a licence to be a literal.** These are named
precisely *because* they are off it: a number that no ladder explains is one
that only its own name can. Equal values with different jobs stay apart —
a menu option and a palette row are both 13px across, but one is chosen and
the other is read, and their vertical insets were tuned apart.

**The converse is the sharper rule: a value the ladder explains does not belong
here at all.** A nested settings row is indented 16px, and 16px is `--space-sm`,
so it is spent as the step rather than named a second time — Layer 4 is for the
numbers no ladder accounts for, and a Layer-4 alias for `sm` would be a step
wearing a disguise. `--gap-setting-group` (14) earns its name because nothing on
the ladder is 14; that it *composes* to the 40px section gap out of a row's own
13px padding is exactly why the increment, not the total, is what gets named.

**The gutter is not part of the stance.** Each view type used to set its own
padding and its own column alignment — the Inbox pinned left at 44/60, the
Project view centring a 640px measure at 52/60 — on the theory that where the
content sat was itself a signal. It reads that way in a static comp and not at
all in the running app, where clicking between two sidebar rows moved the left
edge of the page and registered as the layout being unstable, not as the two
places being different kinds of place. There is now **one gutter, on all four
sides, on every view**, and identity is carried by the header, the density and
the weight instead. `--measure-*` still varies, but only to stop a long line of
prose running too far right; nothing centres, so every view starts at the same
left edge.

The `--lead-*` family is the gap between a view's header and the thing it
heads, and it *is* per view — that is a density decision, which is stance:

| Token | px | Where |
| --- | --- | --- |
| `--lead-queue` | 22 | Inbox: the progress instrument to the first row |
| `--lead-panel` | 22 | Settings: the title to the tab rail |
| `--lead-library` | 30 | Project: the header to the index |
| `--lead-health` | 30 | Needs attention: the header to the cards |
| `--lead-doc` | 30 | Note: the meta line to the body |
| `--lead-search` | 2 | Search: the scope line to the results |

A queue leads tightest — you are meant to get straight into the list — and
anything you read rather than work through leads at 30. Rounding these onto the
nearest named step is what let a lifted Inbox row rise into the progress caption
above it, which is the exact failure this layer exists to prevent.

**They are consumed from a co-located `Component.css`, never from `className`.**
That is what keeps all three guards armed: eslint fails every arbitrary value
and numeric spacing utility in a class string, the token test fails a raw px in
the stylesheet, and the values themselves live in one file rather than scattered
through TSX.

```css
/* src/components/views/InboxView.css */
.inbox__rowShell {
  margin: 0 calc(-1 * var(--row-queue-x)); /* the lift reaches into the gutter */
}

.inbox__row {
  padding: var(--row-queue-y) var(--row-queue-trail) var(--row-queue-y)
    var(--row-queue-x);
  border-radius: var(--radius-row);
}
```

The bleed is written as a `calc` off the pad rather than as its own number, so
the two cannot drift: a row whose hover plane reaches into the gutter has to
reach by exactly its own inset, and this is the form that proves it. It sits on
a shell rather than the row because the row is one full-surface `<button>` (a
block button's `width: 100%` against two negative margins silently drops the
right-hand bleed) with the File picker overlaid on it as a sibling — a button
cannot nest a button. `--row-queue-trail` is the same discipline again: the
room the picker needs, derived in the token file from the pieces it is made of.

The named `--space-*` steps are unchanged and still govern everything *inside* a
component — gaps between elements, control padding, list rhythm. Layer 4 governs
the frame around them.

### `ViewFrame` variants are the stance

The gutter is not part of it, and neither is the alignment: every variant takes
`--gutter-view-y` / `--gutter-view-x` (44 / 60) on all four sides, and every
column is pinned to the same left edge. What still varies is the title step, the
header's shape, whether the column is capped at all, and where the view's
actions live.

| `variant` | Column | Title step | Where its actions live |
| --- | --- | --- | --- |
| `queue` | uncapped | none — a one-line masthead | two per row: the work is the list |
| `library` | uncapped | `text-title-library` (34) | the frame header |
| `panel` | uncapped (rows cap themselves at `--measure-setting`, 520) | `text-title-panel` (26) | one per row (the tab rail filters, it doesn't act) |
| `health` | uncapped | `text-title-health` (28) | two per card, plus the shelf toggle |
| `doc` | `--measure-doc` (660) | supplied by the view | its own title row; a format toolbar while editing; the source pairing behind one disclosure |
| `search` | `--measure-search` (640) | supplied by the view | none — the query field is the surface |
| `terminal` | uncapped, full-height (the pane scrolls inside itself) | `text-title-panel` (26) | none until the session exits |
| `chat` | `--chat-measure` (660), full-height (the log scrolls inside itself) | `text-title-panel` (26) | the composer footer, plus contextual chrome in the log |

The last column is a stance decision like the other three, and it is the one
this table gained last: see *Composition* above for the slots it names, what
each may hold, and how to record going over.

A measure is set only where line length actually hurts reading, and left unset
where the content is a list of rows that should use the width it has. That is
why five of the eight cap nothing: their rows (or the terminal's grid) are not
prose. Where a single
element inside one of those views *is* prose, the cap goes on that element (a
queue row's serif snippet at `--measure-snippet`) rather than on the frame.

There is no default and no bare scaffold: `variant` is required, so a new view
has to say what kind of place it is.

### The primitives grew variants to match

- **`Button`** — `primary` (the raised control chip), `filled` (ink fill,
  page-coloured label; the one action that *ends* a surface), `quiet` (a ghost
  that carries no padding of its own, because a nav row and a text action are
  not the same shape).
- **`Select`** — `boxed` (a control chip with a chevron) and `token` (quiet mono
  text that takes a pill only while choosing, for a picker beside content it
  must not out-weigh). Both open the same overlay-plane menu.
- **`Checkbox`** — unchecked is a ring and nothing else; checked is an ink
  square with a page-coloured check. Nothing composes it today: every boolean
  the app ships is an applies-immediately setting, and `SettingsView` draws its
  own `role="switch"` toggle for those.
