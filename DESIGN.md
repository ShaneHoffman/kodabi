# Kodama — Aesthetic Direction

**Status:** Locked (Phase 0, ticket P0-3). This is the design system every later screen is
measured against — the "locked system" referenced by Phase 4, and the source the design tokens
(P0-4) and the listening indicator (P0-5) descend from.

The visual companion to this document is [`design/moodboard.html`](design/moodboard.html) — a
self-contained page that *demonstrates* everything below in real material. Read this for the
intent; open that to feel it.

---

## North star

> **Calm you notice before you notice why.**

Kodama records your meetings and quietly organizes them. The interface should feel *unusually
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

> Exact hex values, the full ramp, and light/dark tokens are defined in **P0-4**. This document
> fixes the *roles and relationships*; the moodboard shows *indicative* values to feel the range.

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

**Locked in P0-4:** the design tokens now live in [`design/tokens.css`](design/tokens.css)
(demonstrated by [`design/tokens.html`](design/tokens.html)). Typefaces are **Source Sans 3**
(interface) + **Source Serif 4** (reading views) + **Source Code Pro** (mono). The palette adopts the
moodboard's a11y-tuned values as final; both greens are kept — a quiet interactive accent and the
brighter listening green. Web-font binaries are bundled later in `feat/scaffold-tauri-app`.

---

*Indicative, not final: the exact tokens are locked in P0-4 and the mark is designed in P0-5.*
*This document and `design/moodboard.html` together are the aesthetic that Phase 0 locks before
any screen is built.*
