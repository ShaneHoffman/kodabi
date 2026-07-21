# Kodabi — UI conventions (spacing & primitives)

*Status: Living (Phase 2, UI foundation; extended by the Phase 3 design-system pass).*

Three documents describe the look, and they divide cleanly:

| Document | Fixes |
| --- | --- |
| [`docs/DESIGN.md`](DESIGN.md) | The **aesthetic** — the four principles, the reference class, what we refuse |
| [`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) | The **system** — interaction states, the view state vocabulary, motion, elevation, the accessibility floor |
| **This document** | The **mechanics** — the spacing steps and the primitive catalogue |

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
steps and *also* exposes the named aliases (`index.css:44-55`). So `py-2` and `py-2xs` compile to the
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
with a matching negative inline margin — written `calc(-1 * var(--row-queue-x))`, so the bleed
provably tracks the pad — that lets its hover plane reach into the gutter, a `.project__row` is
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
- **Tracking** → `tracking-eyebrow` (section eyebrows), `tracking-caps` (uppercase micro-text)
- **Line-heights** → `leading-body`, `leading-read`
- **Spacing steps** → `p-*`, `px-*`, `py-*`, `gap-*`, `m-*` with the named suffixes above
- **Radii** → `rounded-sm`, `rounded-md` (tokens.css overrides Tailwind's defaults at `:root`)

> **There are no bridged width utilities.** `max-w-measure`, `max-w-content` and `max-w-wide` were
> real while `--container-measure` / `--container-content` / `--container-wide` were in the
> `@theme inline` block; the redesign split that family into the un-bridged `--measure-*` set and
> the bridge entries went with it. Tailwind emits nothing for an unknown utility and raises no
> error, so those three class names now silently do nothing — a column cap is read from a
> co-located `Component.css` instead (`PlaceholderView` carries the note about it).

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
| Motion | `--dur-*`, `--ease-*` | `transition` / `animation` shorthands |
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
[`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §2.

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
As *text* it measures 3.42–3.70:1 against the light theme's surfaces, below the 4.5:1 floor; as a
*graphic* it clears the 3:1 one. The label carries the same state through value instead
([`CaptureStatusLine`](../src/components/CaptureStatusLine.tsx)).

---

## Primitives

The shared controls live in [`src/components/ui/`](../src/components/ui/). They are the home for control
padding, the focus ring, and the hairline recipes, so screens compose them instead of restating utility
strings. Named function exports, relative imports, one co-located `*.css` per component.

### `Button` — `variant="primary" | "quiet" | "filled"`

Owns **structure only** (rounding and every interaction state: focus, hover, active, disabled) plus
each variant's emphasis. It deliberately sets **no text size**, so a caller's own `text-*` utilities
never collide with a baked-in one. `primary` is the raised control chip — `bg-surface`, `rounded-md`,
the app's control padding `px-xs py-2xs`, `--fw-medium`, and the ring-plus-shadow of `--lift-chip` in
one declaration; `filled` is an ink fill with a page-coloured label, the heaviest control in the app
and the only one that inverts, spent on the single action that *ends* a surface; `quiet` is a
transparent ghost that inherits its colour — the low-emphasis and navigation form.

**Padding follows the variant, not the component.** `primary` and `filled` are real chips with a
fixed size, but a `quiet` button is whatever shape its context needs (a sidebar nav row, a text
action beside a title, a menu item), so it carries no padding of its own and each consumer sets it in
its co-located CSS.

**Hover belongs to the primitive.** Callers used to add their own, in two different destination colours;
there is one hover step and it is toward `--text`.

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

### `Textarea` — labelled multi-line input *(currently unused)*

> **Nothing composes this today.** Both writing surfaces in the app — the note editor's body and the
> quick-capture box — are a raw `<textarea>` carrying an `aria-label`, the `ui-writing` caret class,
> and their own class from a co-located `*.css` (`.note-edit__body`, `.capture__input`). Don't reach
> for this primitive expecting the app's writing surfaces to match it; either match the raw pattern
> those two use, or bring them onto this component in the same change. The description below records
> what the component does, not what the app currently looks like.

`TextField`'s shape for prose: the same control padding, writing-line hairline and focus ring, plus the
body line-height (`--lh-body`). Height and resize behaviour are the caller's, since a capture box and a
note body want very different room. `hideLabel` keeps the label as the accessible name only.

It **carries no fill**, the same as `TextField`. It used to sit on a sunk `--bg-sink` plane; that
token is gone, because a recessed plane is a fourth plane and the system has three. A writing area
sits on whatever plane its surface sits on.

```tsx
<Textarea label="Body" value={body} onChange={…} className="note-editor__body font-mono" />
```

### `Select` — token-styled dropdown, `variant="boxed" | "token"`

A hand-rolled collapsible listbox (WAI-ARIA active-descendant), **not** a headless dependency — the same
combobox know-how the command palette proves, minus the dep, keeping the app's zero-UI-dependency posture.
Focus stays on the trigger; ↑/↓ move a virtual highlight via `aria-activedescendant`, Enter/Space selects,
Escape closes and returns focus to the trigger, click-outside closes, and typing jumps (typeahead). The
open list sits on the **overlay** plane (`--overlay` + `--lift-menu`, at `--layer-dropdown`); the active
row is the value wash (never the reserved green).

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

Pass `disabled` while a write is in flight, and `emptyLabel` for what the list says when there are no
options — it opens and explains rather than refusing to open, which used to leave a trigger that
swallowed every click.

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
  bleeds past the text on a negative margin so nothing in the row shifts when it appears. Its arrow *is*
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

### `Checkbox` — labelled boolean *(currently unused)*

> **Nothing composes this today.** Every boolean in the app is a setting that takes effect the moment
> it moves, so `SettingsView` carries its own local `Toggle`: a `<button role="switch">` with
> `aria-checked` and a knob, styled by `.settings__toggle` / `.settings__knob` in `SettingsView.css`.
> The platform semantics for "this applies immediately" and "this is one of several things you are
> about to submit" differ, and the switch is the first of those. Reach for this component only for the
> second kind, and expect to be its first caller. The description below records what it does, not what
> the app currently looks like.

A real `<input type="checkbox">` under a token skin (`appearance: none`), so keyboard behaviour, form
semantics, and screen-reader state come from the platform rather than re-implemented ARIA — the opposite
call from `Select`, where no native control could carry the theme at all. The label sits inline a `gap-2xs`
beside the box and is bound by `id` (generated via `useId` when omitted); an optional `hint` renders below,
indented to the label column and wired through `aria-describedby`. `onChange` receives the new boolean, not
the event.

```tsx
import { Checkbox } from "./ui/Checkbox";

<Checkbox
  label="Show the capture pill during captures you start"
  hint="A small pill stays on top of full screen apps while a capture is running."
  checked={enabled}
  onChange={setEnabled}
/>
```

The checked state is ink on surface — **value, not hue**. The reserved green is never used here: it means
audio is actually being recorded, and a settings control wearing it would be claiming something false.

### `ViewFrame` — the page scaffold, `variant="queue" | "library" | "panel" | "health" | "doc" | "search"`

The view gutter, the content column, and the eyebrow/title header. Every full view sits in one. The
gutter (`--gutter-view-y` / `--gutter-view-x`) is the same on every variant and on all four sides; see
*Layer 4* below for why that is not negotiable per view.

**`variant` answers "what am I looking at" before the heading is read.** It is not decoration — it is
the one place a view declares its kind, so two views of the same kind can't drift apart. **It is a
required prop.** There is no default and no bare scaffold: a new view has to say what kind of place it
is. The six, with what each fixes:

| `variant` | The view is | Column cap | Title step | `summary` renders as |
| --- | --- | --- | --- | --- |
| `queue` | work to get through | none | none — a one-line masthead | `· 4 to file`, inside the masthead at `text-text-faint` |
| `library` | a place to browse | none | `text-title-library` (34) | `text-label text-text-faint` — a quiet count ("12 notes") |
| `panel` | configuration | none (its rows cap themselves) | `text-title-panel` (26) | not rendered |
| `health` | system state to recover from | none | `text-title-health` (28) | `text-label text-text-faint` |
| `doc` | a note | `--measure-doc` (660) | header supplied by the view | not rendered |
| `search` | results under a pinned query | `--measure-search` (640) | header supplied by the view | not rendered |

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

**`summary` is not a free styling slot.** The variant fixes its typographic role, so a workload sentence
can never render at a count's weight in one view and a heading's in another. Call sites pass the content,
never a class — there is no `className` on it to reach for.

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

### `ListRow` — one row in a list *(currently unused)*

> **Nothing composes this today.** Each list-bearing view now draws its own row from a co-located
> `*.css`, because the row *is* the view's stance: `.inbox__row` lifts onto the raised plane when you
> touch it (these are items you clear), `.project__row` only tints (a reading room, nothing is waiting
> on you), and `.attention__card` arrives already lifted with no hover at all (it flagged itself). One
> affordance, three meanings — which is exactly the difference a single shared row was flattening. Its
> one still-live rule is the one below about hover and focus. The description below records what the
> component does, not what the app currently looks like.

Title, meta, optional two-line snippet, optional trailing control in a fixed column. `layout="inline"`
keeps title and meta on one baseline; `"stacked"` (the default) puts meta underneath. **Hover and the
focus ring live on the same element** — the Inbox previously put hover on the inner title span while the
ring sat on the outer button, so keyboard focus produced a ring with no colour change.

Build the meta string with [`noteMeta`](../src/noteMeta.ts), which takes the surface's own middle
segment: `noteMeta(note)`, `noteMeta(note, note.type)`, `noteMeta(note, matchScore(note.confidence))`.

### `Overlay` — the modal shell

Scrim, stacking layer, backdrop dismissal, and the raised panel. Dismissal fires on click (not
pointerdown) and only when the gesture both started and ended on the backdrop — two guards that existed
twice, byte for byte, before this primitive.

Focus trapping is deliberately **not** here: the palette holds focus on its lone input by swallowing Tab,
the consent nudge wraps Tab across several controls. Each passes its own `onKeyDown`.

```tsx
<Overlay onDismiss={onClose} label="Command palette" className="overflow-hidden">{…}</Overlay>
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
- **`data-testid` discipline.** Interactive elements a screen adds carry a stable kebab-case `data-testid`
  so a future end-to-end harness can select them without depending on copy or DOM shape. The primitives
  spread native props, so a passed `data-testid` reaches the underlying element.
- **No component-level `useEffect`.** External-system glue (focus hand-off, scroll-into-view,
  outside-press dismissal, timers, Tauri events) comes from the blessed bridge hooks in `src/` — the
  primitives here compose those hooks, derive during render, and act in event handlers. See
  [`.claude/rules/no-use-effect.md`](../.claude/rules/no-use-effect.md); eslint enforces the list.

---

## What consumes these today

Every shipped surface composes the primitives for its controls and its frame, and several draw their own
row or writing surface on top. The table is the live inventory: the middle column is what a surface
imports from `src/components/ui/`, the right-hand one is what it draws itself in a co-located `*.css`. A
new screen should find its shape here rather than inventing one.

| Surface | Composes | Draws itself |
| --- | --- | --- |
| `Sidebar` | `Button variant="quiet"` rows (including the needs-attention row), `StatusMessage` | the nav rows' geometry (`Sidebar.css`) |
| `InboxView` | `ViewFrame variant="queue"`, `Select variant="token"`, `StatusMessage` | `.inbox__row` and the progress instrument |
| `NeedsAttentionView` | `ViewFrame variant="health"`, `Button` (with `loading`), `StatusMessage` | `.attention__card`, pre-lifted |
| `ProjectView` | `ViewFrame variant="library"`, `Button`, `StatusMessage` | `.project__row`, a hand-rolled index row |
| `NoteEditorView` | `ViewFrame variant="doc"`, `Button`, `StatusMessage` | its own header, and raw `<textarea>` / `<input>` elements with `aria-label` + `ui-writing` |
| `SettingsView` | `ViewFrame variant="panel"`, `Select`, `Button`, `StatusMessage` | a local `role="switch"` `Toggle`, and a raw number `<input>` (`.settings__chip`) |
| `SearchView` | `ViewFrame variant="search"`, `StatusMessage` | its own query field |
| `PlaceholderView` | `ViewFrame variant="panel"`, `StatusMessage` | — |
| `AppErrorBoundary` | `ViewFrame variant="health"`, `StatusMessage` | — |
| `CommandPalette` | `Overlay` | its own combobox input and rows |
| `ConsentNudge` | `Overlay`, `Select`, `TextField`, `Button`, `StatusMessage` | — |
| `QuickCapture` | `Button`, `StatusMessage` | a raw `<textarea>` (`.capture__input ui-writing`) |
| `ListeningIndicator` | `CaptureStatusLine` | its own dot and waveform |
| `CaptureOverlayPill` | `CaptureStatusLine`, `SpiritMark` | the pill window |
| `CaptureToast` | — | the overlay plane directly (it is not a modal, so not `Overlay`) |

The right-hand column is not an accusation. A view drawing its own row is the redesign working as
intended (see `ListRow` above): the row is where a queue, a library and a health view differ most, so
each one states that difference in its own CSS. What it does mean is that **`ListRow`, `Checkbox` and
`Textarea` have no call sites at all** — check this table before assuming a primitive is the shape the
app actually wears.

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
(`--gap-tab`, `--tab-pad-*`, `--gap-setting`, `--sublabel-tuck`) and the
summoned surfaces (`--palette-*`, `--menu-*`, `--capture-*`).

**Off the step scale is not a licence to be a literal.** These are named
precisely *because* they are off it: a number that no ladder explains is one
that only its own name can. Equal values with different jobs stay apart —
a menu option and a palette row are both 13px across, but one is chosen and
the other is read, and their vertical insets were tuned apart.

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
.inbox__row {
  padding: var(--row-queue-y) var(--row-queue-x);
  margin: 0 calc(-1 * var(--row-queue-x)); /* the lift reaches into the gutter */
  border-radius: var(--radius-row);
}
```

The bleed is written as a `calc` off the pad rather than as its own number, so
the two cannot drift: a row whose hover plane reaches into the gutter has to
reach by exactly its own inset, and this is the form that proves it.

The named `--space-*` steps are unchanged and still govern everything *inside* a
component — gaps between elements, control padding, list rhythm. Layer 4 governs
the frame around them.

### `ViewFrame` variants are the stance

The gutter is not part of it, and neither is the alignment: every variant takes
`--gutter-view-y` / `--gutter-view-x` (44 / 60) on all four sides, and every
column is pinned to the same left edge. What still varies is the title step, the
header's shape, and whether the column is capped at all.

| `variant` | Column | Title step |
| --- | --- | --- |
| `queue` | uncapped | none — a one-line masthead |
| `library` | uncapped | `text-title-library` (34) |
| `panel` | uncapped (rows cap themselves at `--measure-setting`, 520) | `text-title-panel` (26) |
| `health` | uncapped | `text-title-health` (28) |
| `doc` | `--measure-doc` (660) | supplied by the view |
| `search` | `--measure-search` (640) | supplied by the view |

A measure is set only where line length actually hurts reading, and left unset
where the content is a list of rows that should use the width it has. That is
why four of the six cap nothing: their rows are rows, not prose. Where a single
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
