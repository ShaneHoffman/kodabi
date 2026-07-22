# Kodabi — Aesthetic Direction

**Status:** v2 — re-tuned and re-locked on `feat/screen-overhaul` (originally Phase 0, ticket P0-3).
This is the design system every later screen is measured against — the "locked system" referenced by
Phase 4, and the source the design tokens (P0-4) and the listening indicator (P0-5) descend from.
**It is binding going forward**, exactly as the Phase 0 lock was.

What v2 changed and what it did not: the *principles* below are untouched — the north star, the four
principles, the Linear / Things 3 bar, the hold/refuse table, and the "do not use" list all stand as
written. What was deliberately re-opened was the **material**: the exact values the palette is built
from, and the type ramp. The day ground moved off beige to a warm near-white, the raised plane
flipped from a darker fill to a lighter one (a card that *lifts* rather than a box that *fills*), both
greens gained life, and the interface type ramp tightened away from the reading ramp. Same roles,
same relationships, better paper. The current values live in
[`design/tokens.css`](../design/tokens.css); the measured contrast matrix lives in
[`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §6.

The visual companion to this document is [`design/moodboard.html`](../design/moodboard.html) — a
self-contained page that *demonstrates* everything below in real material. Read this for the
intent; open that to feel it. The moodboard still shows the Phase 0 indicative values; for the
material as it ships, [`design/tokens.css`](../design/tokens.css) is the source of truth.

---

## North star

> **Calm you notice before you notice why.**

Kodabi records your meetings and quietly organizes them. The interface should feel *unusually
calm and beautiful* — and a person should feel that calm **without ever consciously registering
"forest."** The theme is an undertone, never a costume. If a first-time user thinks *"this is
peaceful and well-made"* and not *"oh, a nature app,"* we succeeded.

That sentence is the acceptance test for every screen we ship.

---

## The bar: Linear / Things 3 — not an admin panel

The reference class is deliberate. We are aiming for the quality of **Linear** and **Things 3**:
software that feels considered in the hand — typography-first, chrome-last, fast, and quiet. We
are explicitly **not** building an admin panel, a dashboard, or a productivity app with a nature
skin over dense tables.

| We hold to | We refuse |
| --- | --- |
| Typography carries the hierarchy | Dense data tables and grids |
| Space instead of borders and boxes | Boxes nested inside boxes |
| One deliberate motion, used sparingly | Motion and color used to shout |
| Precision in the small things | Chrome competing with content |
| Restraint as the default | Decorative theming |

If a screen would look at home in a settings panel, it has failed the bar — no matter how
correct it is.

---

## The four principles

### 1. Ma (間) — negative space is a material, not a leftover

Ma is the charged interval *between* things. We compose with emptiness deliberately: hierarchy
comes from how much room something is given, not from rules, fills, badges, or borders.

**How to apply:** Default to more space than feels necessary. Establish rank through scale and
whitespace before reaching for any other tool. When a layout feels busy, the fix is almost always
*remove something and add space*, not rearrange.

### 2. Forest palette (intent) — a forest you feel, not one you see

A small family of quiet neutrals drawn from the forest floor — **moss, fern, mist, stone, washi
cream, sumi charcoal** — desaturated nearly to grey so the mood reads as *calm* rather than
*green*. **Value carries the hierarchy; hue stays almost silent.** There is exactly **one** living
green, and it is spent only on the **listening state**.

**How to apply:** Build screens as if they were greyscale — establish all hierarchy through value
and type, then let the near-neutral forest tones sit underneath as warmth. Never use color to rank
or categorize. The single accent green is reserved; do not spend it on ordinary UI.

**The values live in [`design/tokens.css`](../design/tokens.css), and only there.** This paragraph
used to restate them, and by the time anyone read it again it was quoting a palette that no longer
existed — a day ground of `#F7F5EF`, an ink of `#1F1E18`, a "sinking" fourth plane that had since
been deleted. The relationships are what this document fixes, and they are unchanged: a warm
near-white day ground and a deep warm black night one (never a blue-grey charcoal), three planes
each *lighter* than the last, ink receding through two muted steps, and hue almost silent
throughout. For the hexes, read the file. The measured contrast matrix is
[`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §6.

**A raised plane lifts; it does not fill.** This is the one structural change v2 made, and it follows
from Ma and from "space instead of boxes". The light theme's raised surface used to be mist — a fill
*darker* than the page — so every button, dropdown, selected row, and modal read as a grey box
stamped down onto the paper. It is now a hair *lighter* than the ground, because a lifted
sheet catches more light than the surface under it. Separation is the value shift plus a hairline
plus a shadow, never a border and never a darker fill. (Night was already lighter-than-ground and is
unchanged in principle.) Three planes only: page, raised, overlay.

**There is exactly ONE green, and the interactive accent is retired.** v2 briefly kept two — a
quiet interactive green for links, focus and selected text, alongside the reserved listening one —
and that second hue is gone as a token. Links are ink with an underline, the focus ring is ink,
selection is an ink wash. The single living green (`--accent-dot`) is **spent only on the listening
state and the writing caret**, both of which mean the same thing: this is happening right now. That
reservation is absolute — it never marks selection, never ranks, and is never used as text, where
it does not clear the contrast floor anyway.

> The exact tokens, the full ramp, and the light/dark mapping live in
> [`design/tokens.css`](../design/tokens.css) — the single source of truth. **This document quotes
> no hexes at all**, deliberately: it fixes the *roles and relationships*, and every time it also
> restated a value, the value moved and the prose did not. It said the raised plane was `#FDFCF8`
> for two re-tunes after it became `#FBFAF6`. Read the file for values; read this for what they
> are for.

### 3. Wabi-sabi restraint — quiet, warm, and unfinished on purpose

Wabi-sabi prefers the plain and weathered to the glossy and complete. We favor warmth over polish,
let a little asymmetry and imperfection stand, and treat "done" as the point where removing one
more thing would break it.

**How to apply:** Nothing is decorative — every element earns its place or is cut. Prefer warm,
matte, paper-like surfaces to crisp glass and hard shadow. Resist the urge to fill, align, and
perfect everything; a little quiet imperfection reads as human.

### 4. Theme-as-restraint guardrail — evoke the archetype, never trace it

The theme is expressed by what we **leave out**. The kodama (a quiet forest spirit) is *suggested*
through calm, space, warmth, and a single green — it is never *drawn*.

**Do not use:**
- Leaf or tree icons
- Wood-grain textures
- Spirit / mascot / character illustrations (never trace Ghibli)
- Literal forest scenery or landscape imagery
- Color used to code or categorize the UI
- Admin-panel density (tight tables, boxes, hairline grids)

**Instead:** space and type, washi and mist, atmosphere built from value, and one breathing dot.
If a choice makes the forest *obvious*, it is wrong.

---

## Reference translation — five sources, one instruction each

The moodboard is assembled from these named categories. We take a single discipline from each; we
never copy their imagery.

| Reference | What we take |
| --- | --- |
| **Japanese stationery** | The discipline of a beautifully made *blank* page. |
| **Ghibli backgrounds** | The light and air of the scene — never the characters. |
| **Muji** | Everything unbranded, down to its function; anonymous quality. |
| **Washi paper** | Warmth and tactile tooth without any ornament. |
| **Misty forest photography** | Depth built from *value*, not from detail. |

---

## What this hands downstream

- **→ P0-4 (design tokens):** the palette *roles* (moss / fern / mist / stone / washi / sumi as
  near-neutrals; one reserved green) and the *type roles* (one humanist sans for the interface, one
  serif for reading views; hierarchy through type and spacing, almost never color). P0-4 chooses the
  exact typefaces, the type scale, the spacing scale, and the final hex tokens for light and dark.
- **→ P0-5 (listening indicator / kodama spirit-mark):** the reserved green and the "evoke, don't
  trace" restraint. The single accent belongs to the listening state; idle is absent, listening
  breathes. The mark itself is designed in P0-5 — this document only reserves the color and the
  feeling.

**Locked in P0-4, re-tuned in v2:** the design tokens live in
[`design/tokens.css`](../design/tokens.css). **Typefaces are unchanged** — **Source Sans 3**
(interface) + **Source Serif 4** (reading views) + **Source Code Pro** (mono), self-hosted so they
render offline, at the weights the `--fw-*` scale names (400/500/600/700, plus a sans italic) and
no others.

What v2 replaced is the values P0-4 adopted from the moodboard. The palette was rebuilt to product
grade (above), and the **type ramp split into two voices**: the interface ramp tightened while the
reading ramp was left as it was, so chrome is compact and a note still opens like a page. The ramp
is now **fixed px, not rem** — the window is 960×640 and does not reflow, so the earlier relative
steps only made the same role render at two sizes for no reader's benefit. Both ramps are
enumerated in [`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §1, which is where they are maintained;
letter-spacings, spacing, radii and focus are untouched. Motion has changed once since: the shared
easing is a stated curve rather than the CSS `ease` keyword (§4). The measured contrast matrix is
§6.

> **`design/tokens.html` and `design/moodboard.html` are Phase-0 artefacts and have drifted.** Both
> still demonstrate `--accent` and a recessed `bg-sink` plane, neither of which exists any more.
> They are kept as a record of where the aesthetic started, not as a reference for what it is. The
> file above is the source of truth; when the two disagree, the file wins.

---

*This document and `design/moodboard.html` together are the aesthetic Phase 0 locked before any
screen was built. The values were re-tuned once, on `feat/screen-overhaul`; the principles were not.*
