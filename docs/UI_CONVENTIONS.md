# Kodabi — UI conventions (spacing & primitives)

*Status: Living (Phase 2, UI foundation). Extends [`docs/DESIGN.md`](DESIGN.md) — where DESIGN.md fixes
the aesthetic and [`design/tokens.css`](../design/tokens.css) is the material, this document fixes how we
**use** that material: the spacing conventions and the shared primitives that keep screens from
hand-assembling utility strings and drifting.*

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
| View gutter | lg | `px-lg py-lg` | 40 |
| Field stack gap (vertical) | sm | `gap-sm` | 16 |
| Section gap | lg | `gap-lg` | 40 |
| Inline label ↔ control gap | 2xs | `gap-2xs` | 8 |
| Panel / container padding | md | `p-md` | 24 |
| Tight list gap (nav rows) | 3xs | `gap-3xs` | 4 |
| Reading / writing column width | measure | `max-w-measure` | 33rem |

Control padding is **`px-xs py-2xs` (12 / 8)**. (The tokens are named by *step*, not by pixel: `--space-sm`
is 16px and `--space-xs` is 12px — so 12px horizontal padding is `px-xs`, not `px-sm`.) The primitives
below bake this in, so most screens never spell control padding out at all.

---

## Bridged vs. non-bridged tokens

Most tokens are bridged into Tailwind utilities and are consumed **as utilities on the component**:

- **Colours** → `bg-bg`, `bg-bg-sink`, `bg-surface`, `text-text`, `text-text-soft`, `text-text-faint`, `text-accent`…
- **Type sizes** → `text-eyebrow`, `text-cap`, `text-body`, `text-read`, `text-h3`, `text-h2`, `text-display`
- **Families** → `font-sans`, `font-serif`, `font-mono`
- **Spacing steps** → `p-*`, `px-*`, `py-*`, `gap-*`, `m-*` with the named suffixes above
- **Widths** → `max-w-measure`, `max-w-content`, `max-w-wide`
- **Radii** → `rounded-sm`, `rounded-md` (tokens.css overrides Tailwind's defaults at `:root`)

Some tokens are **not** bridged. These must live in a **co-located `Component.css`** imported by the
component (the pattern established by `Sidebar.css`, `CommandPalette.css`, `SpiritMark.css`, and each
primitive's own `*.css`). The un-bridged set:

| Token family | Examples | Why it's in CSS |
| --- | --- | --- |
| Weights | `--fw-medium`, `--fw-semibold` | no `font-*` bridge |
| Letter-spacing | `--ls-eyebrow` | no bridge |
| Line-heights | `--lh-read`, `--lh-body` | no bridge |
| Edges / hairlines | `--edge`, `--edge-faint`, `--hairline` | rendered as **inset shadows** |
| Elevation | `--lift`, `--lift-soft` | rendered as `box-shadow` |
| Focus radius | `--radius-focus` (2px) | no bridge |
| Sheen / flow | `--sheen`, `--flow-*` | specialised |

Each `Component.css` opens with a banner comment stating that **only non-bridged tokens live there** —
everything bridged stays a utility on the element. Keep that split.

### Two recipes to copy exactly

**Hairlines are inset shadows, never borders.** A hairline is a translucent edge, not a 1px `border`
(docs/DESIGN.md: *space instead of borders and boxes*):

```css
box-shadow: inset 0 -1px 0 var(--edge-faint);   /* a single bottom edge */
box-shadow: var(--hairline);                    /* the pre-composed inset ring */
```

**Focus is a 2px accent outline** on `:focus-visible` (2px matches `--radius-focus`) — the Sidebar
convention (`Sidebar.css`), carried by every primitive:

```css
.thing:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}
```

**The reserved green (`--accent-dot`) is untouchable.** It belongs to the listening state alone. Selected
and highlighted rows read through **value**, not hue — an ink wash of the text colour, identical in both
themes:

```css
background: color-mix(in srgb, var(--text) 8%, transparent);   /* never var(--accent-dot) */
```

---

## Primitives

The shared controls live in [`src/components/ui/`](../src/components/ui/). They are the home for control
padding, the focus ring, and the hairline recipes, so screens compose them instead of restating utility
strings. Named function exports, relative imports, one co-located `*.css` per component.

### `Button` — `variant="primary" | "quiet"`

Owns **structure only** (padding `px-xs py-2xs`, `rounded-md`, focus ring, disabled) plus each variant's
emphasis. It deliberately sets **no text size or colour**, so a caller's own `text-*` utilities never
collide with a baked-in one. `primary` is a raised value plane (surface fill, `--hairline`, `--fw-medium`);
`quiet` is a transparent ghost that inherits its colour — the low-emphasis and navigation form.

```tsx
import { Button } from "./ui/Button";

<Button onClick={save}>Save note</Button>                        {/* primary */}
<Button variant="quiet" className="text-text-soft hover:text-text">
  Cancel
</Button>
```

Spreads all native `<button>` props (`type` defaults to `"button"`), merges `className`.

### `TextField` — labelled text input

Label stacked a `gap-2xs` above the input; bound by `id` (generated via `useId` when omitted). The input
carries the control padding, a bottom hairline (inset shadow), and the focus ring. Optional `hint` renders
below and is wired through `aria-describedby`.

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

### `Select` — token-styled dropdown

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

**Never a raw `<select>`.** The native control ignores the token theme entirely (system chrome, no focus
ring, no value wash), so this primitive is the only dropdown.

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

---

## What consumes these today

- **`Sidebar`** — its project rows and the Commands affordance are `Button variant="quiet"`. This is where
  the `py-2 pr-3` numeric drift lived; it now comes from the primitive. The sidebar keeps only its own
  non-bridged bits (eyebrow tracking, the selected-row weight) and applies the nested-project indent
  inline so it overrides the primitive's symmetric padding.
- **`TextField` / `Select`** — built here as foundation; their first screen consumers arrive with the note
  create/edit work (board #46). Until then the usage examples above are the reference.
- **`NeedsAttentionSection`** (in the Inbox) — the per-row Retry for a session whose distill failed is a
  `Button variant="quiet"`, matching the weight of the Inbox's own right-column controls; a row-level
  action shouldn't shout. Its emphasis is value and type only, never the reserved green.
- **`CommandPalette`** is intentionally left as-is: a bespoke search-combobox (no focus ring by design,
  its own inset hairline, `role="combobox"` on the input) — not a generic form field.
