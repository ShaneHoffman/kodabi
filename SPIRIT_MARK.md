# Kodama — Spirit-mark (Listening Indicator)

**Status:** Concept (Phase 0, ticket P0-5). Designs the mark that
[`DESIGN.md`](DESIGN.md) reserves the green and the feeling for; the exact tokens are locked later
in P0-4 and the runtime is built in P1-5. This document is the *intent*; its visual companion,
[`design/spirit-mark.html`](design/spirit-mark.html), *demonstrates* it in real material — open that
to feel the breath.

The listening indicator **is** the kodama. It is one mark doing three jobs at once: the app's
**logo**, its **consent / trust signal** ("Kodama is recording"), and its **screenshot moment** —
the single element that carries the product in a still frame. It animates while listening and is
still when idle.

---

## North star

> **Calm you notice before you notice why.**

Everywhere else in Kodama, hue stays almost silent and hierarchy comes from value and space. The
spirit-mark is the one exception the aesthetic sets aside: the sole place a living green is ever
spent. It has to earn that — a small, quiet presence that reads as *attending to you*, never as a
status LED, a mascot, or a nature motif.

---

## The mark

**A single luminous point held in a breath of space.** One clean circular **core** — the silhouette
— sitting inside a soft, graded **aura** (a field of *ma*). The core is essentially a disc, so it
survives at 16&nbsp;px and in a single ink; the aura is atmospheric, and appears only while
listening. A faint caught-light sheen and a deliberately off-true aura keep it from reading as a
mechanical dot — a little wabi-sabi asymmetry that makes it feel alive and hand-made.

It is entirely **form + rhythm + one green**. There is no face, no eyes, no body, no leaf, no tree,
no character. That is the point: per `DESIGN.md` §4, the kodama is *suggested* through calm, space,
warmth, and a single green — **never drawn**. This is "evoke the archetype, never trace it," and it
is not derivative of Ghibli's character. It descends directly from the moodboard's sanctioned
"one breathing dot," elevated into a specified mark with logo and state semantics.

---

## States

The mark has exactly two states and nothing ambiguous in between — that clarity is what lets it
carry consent.

- **Idle (not recording) — the dormant neutral mark.** The core rendered in a quiet ink value
  (`--text` / near-neutral), **still**, with **no aura and no green**. This *is* the resting logo:
  present, plainly not listening. "Idle is absent" is honored literally — the *green and the motion*
  are absent; only a hue-silent presence remains.
- **Listening (recording) — the one green, breathing.** The core warms to the reserved green and a
  soft aura blooms and breathes around it. Because that green is spent *nowhere else in the app*,
  its presence is an unambiguous "recording now," and its absence is an unambiguous "not recording."
- **Wake / settle transitions.** Idle→listening *warms* the ink to green and draws the first breath
  over ~450 ms — the quiet "it woke up" beat. Listening→idle reverses it: green recedes, the breath
  settles to still, the aura fades. One deliberate motion, never a flourish.

---

## Animation intent

Three layered motions over one firm accessibility floor. Values are indicative starting points for
P1-5 to refine against real audio; the moodboard's `breathe` / `halo` keyframes are the seed.

- **Baseline — breathing.** A slow **~4.2 s** ease-in-out swell of the core and aura (core `scale
  1 → 1.12`, aura `scale .86 → 1.14` with a gentle opacity rise). This is the resting rhythm of a
  presence that is simply listening; it runs the whole time capture is on, even in silence.
- **Overlay — a whisper of a waveform.** Voice amplitude gently modulates the **aura's bloom**
  (radius + opacity) on top of the breath — heavily smoothed and **capped**, so speech makes the
  breath *swell* rather than draw a scope. Explicitly **not** an EQ meter, bars, or an oscilloscope.
  Intended mapping: a low-passed amplitude envelope drives aura scale within a small bounded range
  (roughly +0–18%); quiet rooms read as pure breath.
- **Living aura — a gentle undulation.** The aura is not a static glow: two soft, blurred layers
  slowly counter-rotate and morph their shape, so its edge quietly undulates — a living presence
  rather than a status light. Kept whisper-subtle so it *evokes* life without tipping into an
  illustrated character; it freezes to a still, symmetric glow under reduced motion.
- **Accessibility floor — reduced motion.** Under `prefers-reduced-motion: reduce`, all animation
  stops and listening becomes a **still green mark** — still unmistakably on-air by the reserved
  color and presence alone. This mirrors the locked behavior already in `design/moodboard.html`.

---

## One mark, three jobs

- **Logo.** The still neutral mark at rest. Because the core is a clean disc carried by *value*, not
  hue, it holds up as a brand mark at any size and in a single ink. Lockups: **mark-only** and
  **mark + `kodama` wordmark** (in the interface humanist sans). Sizes down to a **16 px tray icon /
  favicon**, plus title-bar and About. **Clear space** is measured in *ma*: an empty margin of at
  least the core's own diameter on every side; the aura's reach is generous — never crop it.
- **Trust / consent signal.** Green + breath = recording; still neutral = not recording. It is
  unambiguous *because* the green is reserved — green anywhere in Kodama means "live." This is the
  consent story P1-5 must satisfy, seeded here.
- **Screenshot moment.** In an otherwise near-neutral interface, the one breathing green is the
  element that sells the app in a still marketing frame.

**Monochrome / inverse:** the idle mark is already a single value, so it inverts cleanly on both
washi (light) and sumi (dark) grounds without redrawing.

---

## Accessibility & consent

- **Not color alone.** The on-air read is backed by *motion* (breathing) and *presence* (the aura)
  in addition to the reserved green, and in the running app it is expected to pair with a text label
  and reflect real capture state (P1-5). It never depends solely on distinguishing one hue.
- **Reduced motion** degrades to a still green mark (above) — never a blank or an ambiguous state.
- **Contrast & theme.** The green shifts by theme (`#5F7E5A` light / `#86A67E` dark — indicative,
  P0-4 locks the final value) so it stays legible on washi and on night grounds.
- **Unambiguous by construction.** Two states, one reserved color, no in-between — the design makes
  "am I being recorded?" answerable at a glance.

---

## Optional "kodama rattle" sound

A faint wooden click/rattle on state-change (wake) is a natural later addition — **off by default**,
a Phase-1+ concern. State-change is the trigger hook. No audio is produced by this concept; it is
noted here and deferred.

---

## What this hands downstream

- **→ P0-4 (design tokens):** confirms the *role* — one reserved green, listening only — for the
  token set. Open questions for P0-4 to lock: the **final green hex** (light/dark), the **core
  diameter and aura geometry** as real tokens, and the **humanist-sans face** used in the wordmark
  lockup.
- **→ P1-5 (runtime listening indicator):** the full behavior spec above — two states, wake/settle
  transition, breathing baseline, the capped voice-amplitude "whisper" mapping, and the
  reduced-motion floor — plus the requirement that the mark reflect **real capture state** and be
  unambiguous enough to serve as the **consent signal**. `design/spirit-mark.html` is the reference
  implementation to build against (its voice envelope is *simulated*; P1-5 substitutes real audio).

---

*Concept, not final: the exact tokens are locked in P0-4 and the runtime is built in P1-5.*
*This document and [`design/spirit-mark.html`](design/spirit-mark.html) together specify the mark
that `DESIGN.md` reserved the green and the feeling for.*
