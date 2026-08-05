# Kodabi — Aesthetic Direction

**Status:** v3 — the **principles** below are live and binding; the **material** they describe was
replaced by Grove on 2026-08-01.

> **Read this document for intent, never for material.** The four principles, the north star, the
> Linear / Things 3 bar, the hold/refuse table and the "do not use" list all survived the Grove
> redesign intact — they are why Grove looks the way it does. What did not survive is every concrete
> claim about planes, palettes and surfaces, which twice now has outlived the values it described.
> Each such paragraph below carries an inline **v3** note saying what Grove does instead.
>
> The material is the `@theme` block in [`src/index.css`](../src/index.css), the doctrine is
> [`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md), and the mechanics are
> [`docs/UI_CONVENTIONS.md`](UI_CONVENTIONS.md). Where this document and those disagree, they win.

What v2 changed and what it did not: the *principles* below are untouched — the north star, the four
principles, the Linear / Things 3 bar, the hold/refuse table, and the "do not use" list all stand as
written. What was deliberately re-opened was the **material**: the exact values the palette is built
from, and the type ramp. The day ground moved off beige to a warm near-white, the raised plane
flipped from a darker fill to a lighter one (a card that *lifts* rather than a box that *fills*), both
greens gained life, and the interface type ramp tightened away from the reading ramp. Same roles,
same relationships, better paper.

**v3:** Grove re-opened the material a second time and went further than v2 did — one ground plane
instead of three, glass instead of paper, and a folder palette where v2 had none. The *principles*
came through unchanged, which is the interesting part: Grove is quieter, more spacious and more
reserved about colour than v2 was, by holding to the same four rules against a different material.

The visual companion to this document is [`design/moodboard.html`](../design/moodboard.html) — a
self-contained page that *demonstrates* everything below in real material. Read this for the
intent; open that to feel it. The moodboard still shows the Phase 0 indicative values; for the
material as it ships, the `@theme` block in [`src/index.css`](../src/index.css) is the source of
truth, and [`DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) is its doctrine.

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

**The values live in the token file, and only there.** This paragraph used to restate them, and by
the time anyone read it again it was quoting a palette that no longer existed — a day ground of
`#F7F5EF`, an ink of `#1F1E18`, a "sinking" fourth plane that had since been deleted. The
relationships are what this document fixes: a warm near-white day ground and a deep warm black night
one (never a blue-grey charcoal), ink receding through muted steps, and hue almost silent throughout.

> **v3:** the file is now the `@theme` block in [`src/index.css`](../src/index.css), and Grove keeps
> every relationship in that sentence. What it drops is "three planes each *lighter* than the last":
> Grove has **one** ground and builds every surface as glass over it (§5 of
> [`DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md)). The measured contrast matrix is §6 of that document, and
> it describes the Grove palette — not the one this section was written against.

**A raised plane lifts; it does not fill.** This follows from Ma and from "space instead of boxes".
The light theme's raised surface used to be mist — a fill *darker* than the page — so every button,
dropdown, selected row, and modal read as a grey box stamped down onto the paper. It became a hair
*lighter* than the ground, because a lifted sheet catches more light than the surface under it.
Separation is the value shift plus a hairline plus a shadow, never a border and never a darker fill.

> **v3:** the rule holds and Grove sharpens it — a raised surface is translucent, so it literally
> *is* the ground with more light on it rather than a fill of its own. The "three planes only: page,
> raised, overlay" that used to close this paragraph is gone with the three-plane palette.

**Green is reserved, absolutely.** The interactive accent is retired: links are ink with an
underline, the focus ring is ink, selection is an ink wash. Green is spent only on things that mean
*this is happening right now*.

> **v3:** Grove widens the reservation from two sites to four — the kodama, the caret, a search match
> and a routing suggestion — under one rule: green is the system's voice, never progress, a count, or
> decoration (`DESIGN_SYSTEM.md` §2). It is a **two-step** token, not one: `--color-kodama` is the
> mark, `--color-kodama-ink` is the step green takes when it must carry text, which is what makes a
> highlighted match legible. So the old "never used as text, where it does not clear the contrast
> floor anyway" is no longer true — Grove solved it with a second step rather than a prohibition.
> Grove also adds a **folder palette** (coral, cobalt, teal, plum), which is not a retreat from
> "never use colour to rank or categorize": a folder hue is *identity*, never rank, never status, and
> the doctrine says so in as many words.

> The exact tokens, the full ramp, and the night/day mapping live in the token file — the single
> source of truth. **This document quotes no hexes at all**, deliberately: it fixes the *roles and
> relationships*, and every time it also restated a value, the value moved and the prose did not. It
> said the raised plane was `#FDFCF8` for two re-tunes after it became `#FBFAF6`. Read the file for
> values; read this for what they are for.

### 3. Wabi-sabi restraint — quiet, warm, and unfinished on purpose

Wabi-sabi prefers the plain and weathered to the glossy and complete. We favor warmth over polish,
let a little asymmetry and imperfection stand, and treat "done" as the point where removing one
more thing would break it.

**How to apply:** Nothing is decorative — every element earns its place or is cut. Resist the urge to
fill, align, and perfect everything; a little quiet imperfection reads as human.

> **v3:** this paragraph used to end "prefer warm, matte, paper-like surfaces to crisp glass and hard
> shadow", and Grove is made of glass. The preference was never really about the material, though —
> it was about *hardness*. Grove's glass is warm-tinted, heavily blurred, and edged with a soft inset
> highlight rather than a crisp line, and its shadows are wide and low-contrast. Read the rule as
> "warm and soft over hard and glossy", which is what it was doing all along.

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

**Locked in P0-4, re-tuned in v2:** the design tokens lived in `design/tokens.css`. **Typefaces are
unchanged** — **Source Sans 3** (interface) + **Source Serif 4** (reading views) + **Source Code
Pro** (mono), self-hosted so they render offline, at the weights the `--fw-*` scale names
(400/500/600/700, plus a sans italic) and no others.

What v2 replaced is the values P0-4 adopted from the moodboard. The palette was rebuilt to product
grade (above), and the **type ramp split into two voices**: the interface ramp tightened while the
reading ramp was left as it was, so chrome is compact and a note still opens like a page. The ramp
is now **fixed px, not rem** — the window is 960×640 and does not reflow, so the earlier relative
steps only made the same role render at two sizes for no reader's benefit.

> **v3:** the two-voice split survived and Grove made it three, by giving data its own face:
> `font-ui` for the interface, `font-data` for anything that lines up in a column, `font-note` for
> reading ([`DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §1). Fixed px survived too. What is gone is the
> enumerated `--fs-*` ramp this sentence pointed at: Grove sets sizes with utilities and names only
> the one size that must never drift, the reading step. §1 of that document describes Grove's type,
> not the v2 ramp, and §6 its contrast — do not read either as a continuation of this paragraph.
>
> **The Source trio is gone too.** Grove's three faces (Bahnschrift, Cascadia Mono, Georgia) all
> ship with Windows, so the app self-hosts nothing and fetches no font; `design/tokens.css` and the
> `@fontsource` dependencies were deleted with the rest of the pre-Grove layer.

> **`design/moodboard.html` is a Phase-0 artefact and has drifted.** It still demonstrates
> `--accent` and a recessed `bg-sink` plane, neither of which exists any more. It is kept as a
> record of where the aesthetic started, not as a reference for what it is; `src/index.css` is the
> source of truth, and when the two disagree the stylesheet wins.

---

*This document and `design/moodboard.html` together are the aesthetic Phase 0 locked before any
screen was built. The values were re-tuned once, on `feat/screen-overhaul`; the principles were not.*
