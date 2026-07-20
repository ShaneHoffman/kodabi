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
| View gutter | lg | `p-lg` | 40 |
| Field stack gap (vertical) | sm | `gap-sm` | 16 |
| Section gap | lg | `gap-lg` | 40 |
| Inline label ↔ control gap | 2xs | `gap-2xs` | 8 |
| Panel / container padding | md | `p-md` | 24 |
| Tight list gap (sidebar nav rows) | 3xs | `gap-3xs` | 4 |
| Content list gap (every `ListRow` list) | sm | `gap-sm` | 16 |
| List row vertical padding | 2xs | `py-2xs` | 8 |
| Reading / writing column width | measure | `max-w-measure` | 33rem |

The view gutter and the section gap are owned by [`ViewFrame`](../src/components/ui/ViewFrame.tsx), so
a screen never spells them out. (This table previously claimed the gutter was `px-lg py-lg`; every view
in the tree used `p-xl`. The component now settles it, and it settled on `p-lg`: 64px every side spent a tenth of a short window before the eyebrow started.)

**Every list of `ListRow`s separates at `gap-sm`, whatever its rows contain.** The gap has to beat the
`gap-3xs` a row stacks its own lines at, or the list stops reading as rows — the Inbox shipped
three-line rows at `gap-3xs` and became one undifferentiated block. It is tempting to then scale the
gap per view (tighter for inline rows, looser once a snippet is in play); don't. That was tried, and
the result was three list rhythms in one app, which reads as carelessness from across the room even
though each number is defensible on its own screen. One gap, set by the densest thing a row can hold.

Control padding is **`px-xs py-2xs` (12 / 8)**. (The tokens are named by *step*, not by pixel: `--space-sm`
is 16px and `--space-xs` is 12px — so 12px horizontal padding is `px-xs`, not `px-sm`.) The primitives
below bake this in, so most screens never spell control padding out at all.

---

## Bridged vs. non-bridged tokens

Most tokens are bridged into Tailwind utilities and are consumed **as utilities on the component**:

- **Colours** → `bg-bg`, `bg-bg-sink`, `bg-surface`, `text-text`, `text-text-soft`, `text-text-faint`, `text-accent`…
- **Type sizes** → `text-eyebrow`, `text-cap`, `text-body`, `text-read`, `text-h3`, `text-h2`, `text-display`
- **Families** → `font-sans`, `font-serif`, `font-mono`
- **Weights** → `font-medium`, `font-semibold`
- **Tracking** → `tracking-eyebrow` (section eyebrows), `tracking-caps` (uppercase micro-text)
- **Line-heights** → `leading-body`, `leading-read`
- **Spacing steps** → `p-*`, `px-*`, `py-*`, `gap-*`, `m-*` with the named suffixes above
- **Widths** → `max-w-measure`, `max-w-content`, `max-w-wide`
- **Radii** → `rounded-sm`, `rounded-md` (tokens.css overrides Tailwind's defaults at `:root`)

> **Never Tailwind's own `tracking-wide`.** It is 0.025em; the eyebrow token is 0.22em. The Sidebar used
> the token through a CSS class while twelve other call sites used the Tailwind default, so the same role
> rendered 8.8× apart. Bridging `tracking-eyebrow` / `tracking-caps` removed the choice.

Some tokens are **not** bridged. These must live in a **co-located `Component.css`** imported by the
component (the pattern established by `Sidebar.css`, `SpiritMark.css`, and each primitive's own
`*.css`). The un-bridged set:

| Token family | Examples | Why it's in CSS |
| --- | --- | --- |
| Edges / hairlines | `--edge`, `--edge-faint`, `--hairline` | rendered as **inset shadows** |
| Elevation | `--lift`, `--lift-soft` | rendered as `box-shadow` |
| Motion | `--dur-*`, `--ease-*` | `transition` / `animation` shorthands |
| Focus | `--focus-width`, `--focus-offset`, `--radius-focus` | rendered as `outline` |
| Derived recipes | `--wash-active`, `--scrim`, `--scrollbar-*` | composed values |
| Sheen / flow | `--sheen`, `--flow-*` | specialised |

**Enforced, not aspirational.** [`src/designTokens.test.ts`](../src/designTokens.test.ts) fails any literal
colour, font-family or duration in a `src/**/*.css` file, and `eslint.config.js` fails numeric spacing
utilities and arbitrary values inside `className`. Both run in gates CI already runs. The escape hatch is
a `token-guard-allow` comment, which is deliberately greppable.

Each `Component.css` opens with a banner comment stating that **only non-bridged tokens live there** —
everything bridged stays a utility on the element. Keep that split.

### Two recipes to copy exactly

**Hairlines are inset shadows, never borders.** A hairline is a translucent edge, not a 1px `border`
(docs/DESIGN.md: *space instead of borders and boxes*):

```css
box-shadow: inset 0 -1px 0 var(--edge-faint);   /* a single bottom edge */
box-shadow: var(--hairline);                    /* the pre-composed inset ring */
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
  outline: var(--focus-width) solid var(--accent);
  outline-offset: var(--focus-offset);
}
```

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

### `Button` — `variant="primary" | "quiet"`

Owns **structure only** (padding `px-xs py-2xs`, `rounded-md`, and every interaction state: focus,
hover, active, disabled) plus each variant's emphasis. It deliberately sets **no text size or colour**,
so a caller's own `text-*` utilities never collide with a baked-in one. `primary` is a raised value
plane (surface fill, `--hairline`, `--fw-medium`); `quiet` is a transparent ghost that inherits its
colour — the low-emphasis and navigation form.

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

### `Textarea` — labelled multi-line input

`TextField`'s shape for prose: the same control padding, writing-line hairline and focus ring, plus the
body line-height (`--lh-body`). Height and resize behaviour are the caller's, since a capture box and a
note body want very different room. `hideLabel` keeps the label as the accessible name only.

It **keeps its fill** (`bg-bg-sink`) where `TextField` gave one up. That is deliberate, not drift: a
multi-line box you write paragraphs into is a writing well, and the sunk plane is what says "this area is
yours". A single-line field has no area to speak of.

```tsx
<Textarea label="Body" value={body} onChange={…} className="note-editor__body font-mono" />
```

### `Select` — token-styled dropdown, `variant="boxed" | "quiet"`

A hand-rolled collapsible listbox (WAI-ARIA active-descendant), **not** a headless dependency — the same
combobox know-how the command palette proves, minus the dep, keeping the app's zero-UI-dependency posture.
Focus stays on the trigger; ↑/↓ move a virtual highlight via `aria-activedescendant`, Enter/Space selects,
Escape closes and returns focus to the trigger, click-outside closes, and typing jumps (typeahead). The
open list sits on `--lift` elevation; the active row is the value wash (never the reserved green).

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
focus ring, and every keyboard behaviour are identical across both.

- **`boxed`** (the default) is the form field and reads like one: `bg-surface`, the full `--hairline` ring,
  `text-text`.
- **`quiet`** rests as text — transparent, no hairline, `text-text-soft`. Hover takes `--wash-active` and
  steps the colour to `--text`; while open (`[aria-expanded="true"]`) it takes `--surface` + `--hairline` +
  `--text`, because open it *is* the anchored control the list hangs off. Same
  inherit-then-step-toward-ink logic as `Button variant="quiet"`.

```tsx
<Select hideLabel variant="quiet" label={`File "${note.title}" to project`} … />
```

**`quiet` is for an in-content affordance sitting beside content it must not out-weigh — never in a form.**
A field that doesn't look like a field is a field people don't fill in. Its only consumer is the Inbox's
per-row *File to…* picker, where boxed made the control the heaviest object on the screen while the note
title was the subject of the row (against DESIGN.md's *space instead of borders and boxes* and *typography
carries the hierarchy*). Settings' retention picker, the note editor's pickers, and `ConsentNudge` all stay
`boxed`.

**Never a raw `<select>`.** The native control ignores the token theme entirely (system chrome, no focus
ring, no value wash), so this primitive is the only dropdown.

### `Checkbox` — labelled boolean

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

### `ViewFrame` — the page scaffold, `variant="queue" | "library" | "panel"`

The view gutter (`p-lg`), the centred content column, the section rhythm (`gap-lg`), and the
eyebrow/title header. Every full view sits in one.

**`variant` answers "what am I looking at" before the heading is read.** It is not decoration — it is the
one place a view declares its kind, so two views of the same kind can't drift apart:

| `variant` | The view is | Column | `summary` renders as |
| --- | --- | --- | --- |
| `queue` | work to get through | `max-w-content` | `text-body text-text-soft` — a workload sentence ("5 notes to file") |
| `library` | a place to browse | `max-w-content` | `text-cap text-text-faint` — a quiet count ("12 notes") |
| `panel` | configuration | `max-w-measure` | not rendered |
| *omitted* | the bare scaffold | `max-w-content` | not rendered |

```tsx
<ViewFrame variant="library" eyebrow="Project" title={formatSlug(slug)}
           summary={`${notes.length} notes`} action={<Button …>New note</Button>}>
  {…}
</ViewFrame>
```

**`summary` is not a free styling slot.** The variant fixes its typographic role, so a workload sentence
can never render at a count's weight in one view and a heading's in another. Call sites pass the content,
never a class — there is no `className` on it to reach for.

**Omit `summary` at zero.** The empty `StatusMessage` in the body already says there's nothing here, and
two "nothing here" voices in one header is one too many. Every call site does this with a conditional
(`notes.length > 0 ? … : undefined`).

`panel` narrows the column to `max-w-measure` and owns that measure for the whole view: a config screen
whose labels and controls run the full content width drifts apart from what it labels. A panel view
therefore does **not** wrap its own blocks in `max-w-measure` — sections inside it are plain
`<section className="flex flex-col gap-sm">` with a `font-serif text-h3 text-text` heading (see
`SettingsView`'s local `SettingsSection`).

Omit `variant` *and* `eyebrow`/`title` for the bare scaffold, when a view's header is a genuinely
different shape (the note editor's carries a back link and its own actions and supplies its own header as
a child — five uses). `PlaceholderView` / `SearchView` deliberately take no variant either: real Search
arrives in Phase 3, and a silhouette invented with nothing to check it against is a guess the rest of the
system would then have to honour.

### `StatusMessage` — `variant="empty" | "error" | "status"`

The one way a view says "nothing here", "that failed", or "working on it". **The variant fixes the ARIA
role**: `error` → `role="alert"`, `status` → `role="status"`, `empty` → none. That binding is the point —
before it, `role="alert"` was on some async failures and not others. `compact` steps the type down to
`text-cap` for a row-level message.

```tsx
<StatusMessage variant="error">Couldn&apos;t load notes: {error}</StatusMessage>
```

### `ListRow` — one row in a list

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

Every shipped surface composes the primitives. The table is the inventory the design-system pass left
behind; a new screen should find its shape here rather than inventing one.

| Surface | Composes |
| --- | --- |
| `Sidebar` | `Button variant="quiet"` rows (including the needs-attention row), `StatusMessage` |
| `InboxView` | `ViewFrame variant="queue"`, `ListRow`, `Select variant="quiet"`, `StatusMessage` |
| `NeedsAttentionView` | `ViewFrame variant="queue"`, `ListRow`, `Button` (with `loading`), `StatusMessage` |
| `ProjectView` | `ViewFrame variant="library"`, `ListRow layout="inline"`, `Button`, `StatusMessage` |
| `NoteEditorView` | `ViewFrame` (bare), `TextField`, `Textarea`, `Select`, `Button`, `StatusMessage` |
| `SettingsView` | `ViewFrame variant="panel"`, `Select`, `TextField`, `Checkbox`, `Button`, `StatusMessage` |
| `PlaceholderView` / `SearchView` | `ViewFrame`, `StatusMessage` |
| `CommandPalette` | `Overlay` (+ its own combobox input) |
| `ConsentNudge` | `Overlay`, `Select`, `TextField`, `Button`, `StatusMessage` |
| `QuickCapture` | `Textarea`, `Button`, `StatusMessage` |
| `ListeningIndicator` / `CaptureOverlayPill` | `CaptureStatusLine`, `SpiritMark` |

Three surfaces keep a documented departure, each forced by what the window is rather than by preference:

- **`CommandPalette`** — its input keeps a bespoke treatment (no focus ring, its own inset hairline,
  `role="combobox"`). It is a search-combobox, not a generic form field. The shell around it is `Overlay`
  like any other modal. Its own rhythm is the modal's, not a control's: the input is `px-md py-sm` and the
  rows (and the no-commands row) are `px-md`, with `gap-md` between a row's title and its hint — a panel
  people read a list in, indented off its own edge rather than padded like a button. Row vertical padding
  stays `py-2xs`, the app's list-row step.
- **`QuickCapture`** — a hotkey-first window that now also carries a visible **File it** button, because a
  pointer-only user cannot press Enter into a window they never focused. Adding it is why the textarea
  regained its focus ring: Tab finally has somewhere to go.
- **`CaptureOverlayPill`** — `rounded-full` rather than a radius step, because the pill's rounded edge *is*
  the window's apparent shape (the window is `transparent: true`) — the genuine off-scale one-off the
  named-steps rule allows. Its window drag stays mouse-only: the window is `focusable: false` so
  appearing over a full-screen app never steals focus, which also puts it out of the tab order. The
  keyboard paths that matter remain — the capture hotkey stops the capture and the pill with it, and the
  Settings toggle turns it off for good.
