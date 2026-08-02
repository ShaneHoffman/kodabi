# Kodabi — Spirit-mark (Listening Indicator)

**Status:** Concept (Phase 0, ticket P0-5). Designs the mark that
[`DESIGN.md`](DESIGN.md) reserves the green and the feeling for; the exact tokens are locked later
in P0-4 and the runtime is built in P1-5. This document is the *intent*; its visual companion,
[`design/spirit-mark.html`](../design/spirit-mark.html), *demonstrates* it in real material — open that
to feel the breath.

The listening indicator **is** the kodama. It is one mark doing three jobs at once: the app's
**logo**, its **consent / trust signal** ("Kodabi is recording"), and its **screenshot moment** —
the single element that carries the product in a still frame. It animates while a capture is under
way and is still when idle.

---

## As built

The concept below is intact and still the intent. This section says where the mark actually lives
and which of the indicative numbers above were superseded when Grove locked them; read it first if
you are changing the mark rather than reasoning about it.

- **Two files, one contract.** `src/components/capture/SpiritMark.tsx` emits the DOM and the `is-*`
  mode classes; the *spirit-mark* block in `src/index.css` §3 is its material. The CSS is a
  sanctioned exception — the aura's two lobes are pseudo-elements and the core's sheen is a gradient
  over a themed fill, neither of which a utility class can reach.
  `src/components/capture/SpiritMark.test.tsx` pins the class contract, because the two halves live
  in different files and a renamed class fails silently: the mark still renders, just inert and ink,
  which is the one failure a listening indicator must never have.
- **Colour is a token.** `--color-kodama` (`#96ce7c` night, `#4f7b3f` day) and `--color-ink`, so
  `.day` and `.hc` carry the mark with no rule of its own. This supersedes the indicative
  `#5F7E5A` / `#86A67E` pair named under *Accessibility & consent*.
- **The clocks are the shared Grove animations,** not values local to the mark: breath and halo
  3200 ms (the concept's indicative ~4.2 s), the counter-rotating lobes 7000 ms and 9500 ms, and
  the starting / reconnecting pulse is `animate-pending` at 1600 ms. The wake and settle transition
  is **300 ms** (was ~450 ms), which is the app's one morph length — the same one the listen pill's
  fill takes, so the pill and the mark inside it arrive together. It is a canonical duration, so it
  is in [`DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §4's table with the other four, not only here.
- **The ring variant.** `variant="ring"` trades the soft field for a single crisp pulse leaving the
  core (2200 ms ease-out, `animate-ring`), for chrome where the mark reads as an *instrument*
  metering the capture rather than a creature sitting in it. It follows the same reservation as the
  aura: listening only. Degraded drops it exactly as it drops the aura.
- **Reduced motion is an animation swap,** not the amplitude gate the pre-Grove mark used: the
  moving animation is replaced by its opacity-only partner (`animate-halo` → `animate-halo-still`),
  and the lobes settle round. See [`DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §4 for the table.
  **An opacity partner also needs somewhere visible to breathe.** The lobes settle to a resting
  shape; the ring settles to a resting *radius* (`motion-reduce:scale-[1.7]`, midway through its
  1 → 2.4 travel), because the ring span's box is the core's box — held at rest with no scale it
  would pulse underneath an opaque disc, and the reduced ring variant would show nothing moving at
  all. That is the failure the swap exists to prevent, so check the resting frame, not just that a
  `motion-reduce:` partner is present.
- **The pairing is a component.** `src/components/shell/ListenPill.tsx` is the mark plus the text
  label the concept requires, plus the elapsed clock. Look at both on `/gallery.html` under
  `pnpm dev` — the Kodama section renders every mode across all four grounds.

---

## North star

> **Calm you notice before you notice why.**

Everywhere else in Kodabi, hue stays almost silent and hierarchy comes from value and space. The
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

The rule that makes the mark load-bearing for consent: **the green means audio is actually being
recorded — nothing else.** Every state below follows from that one test, and nothing ambiguous
sits in between.

- **Idle (not recording) — the dormant neutral mark.** The core rendered in a quiet ink value
  (`--text` / near-neutral), **still**, with **no aura and no green**. This *is* the resting logo:
  present, plainly not listening. "Idle is absent" is honored literally — the *green and the motion*
  are absent; only a hue-silent presence remains.
- **Listening (recording) — the one green, breathing.** The core warms to the reserved green and a
  soft aura blooms and breathes around it. Because that green is spent *nowhere else in the app*,
  its presence is an unambiguous "recording now," and its absence is an unambiguous "not recording."
- **Starting (a capture is being set up) — ink, waking.** Device negotiation can take about a
  second, and a mark that showed nothing for that window would be indistinguishable from a press
  that never registered. The core stays **ink** (nothing is recorded yet, so no green) and pulses
  gently in opacity: anticipation, not on-air.
- **Degraded (recording, but not everything) — green, breath without the field.** One source is
  recording and the other has failed or dropped out. The green **stays**, because audio genuinely
  is being captured and withdrawing it would falsely suggest privacy; but the aura collapses, so
  the mark visibly is not full listening. The paired label names the source that is down.
- **Reconnecting (engaged, nothing recorded) — ink, waking.** Every source has dropped and the
  capture threads are rebuilding. Nothing reaches disk, so the mark wears **no green**. But it is
  *not* the idle mark either: idle means "no capture is running", while this session is still
  engaged and will resume recording with no further press, so it borrows **starting**'s ink pulse.
  Same ink, same motion — both mean "engaged, nothing recorded yet" — with the label distinguishing
  them. A capture that claims to be on air while recording nothing is the one failure the mark must
  never show; a dormant-looking mark over a session that is about to resume is the second.
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
  illustrated character; under reduced motion the rotation stops and the two layers round out to a
  still, symmetric glow.
- **Accessibility floor — reduced motion.** Reduced motion means *fewer and gentler*, not none, so
  under `prefers-reduced-motion: reduce` the mark **stops moving but stays alive**: the core no longer
  swells, the aura no longer scales, and the counter-rotation stops and rounds out — while the halo
  keeps its slow opacity breath, and starting / reconnecting keep their opacity pulse. What is gone is
  every scale and rotation; what is kept is a signal that is still visibly *on*, because movement is
  what causes vestibular trouble and a fade is not. Listening is unmistakably on-air by the reserved
  color regardless; degraded likewise; starting and reconnecting stay ink, their state also carried by
  the text label the mark is always paired with. The mechanism, and why a duration floor was the wrong
  instrument here, is [`docs/DESIGN_SYSTEM.md`](DESIGN_SYSTEM.md) §4.

---

## One mark, three jobs

- **Logo.** The still neutral mark at rest. Because the core is a clean disc carried by *value*, not
  hue, it holds up as a brand mark at any size and in a single ink. Lockups: **mark-only** and
  **mark + `kodabi` wordmark** (in the interface humanist sans). Sizes down to a **16 px tray icon /
  favicon**, plus title-bar and About. **Clear space** is measured in *ma*: an empty margin of at
  least the core's own diameter on every side; the aura's reach is generous — never crop it.
- **Trust / consent signal.** Green = recording; no green = not recording. (Motion alone doesn't
  carry it: a *starting* mark animates in ink because nothing is recorded yet, and a *degraded* one
  is green without its aura.) It is unambiguous *because* the green is reserved — green anywhere in
  Kodabi means "live." This is the consent story P1-5 must satisfy, seeded here.
- **Screenshot moment.** In an otherwise near-neutral interface, the one breathing green is the
  element that sells the app in a still marketing frame.

**Monochrome / inverse:** the idle mark is already a single value, so it inverts cleanly on both
washi (light) and sumi (dark) grounds without redrawing.

---

## Accessibility & consent

- **Not color alone.** The on-air read is backed by *motion* (breathing) and *presence* (the aura)
  in addition to the reserved green, and in the running app it is expected to pair with a text label
  and reflect real capture state (P1-5). It never depends solely on distinguishing one hue.
- **Reduced motion** takes away the *movement*, not the mark: the recording states keep the reserved
  green and the aura keeps a slow opacity breath, so the read is never a blank or an ambiguous state.
  Starting and reconnecting stay ink and keep their opacity pulse, since neither is recording; their
  state is also carried by the text label the mark is always paired with. Nothing scales or rotates.
  Note this weakens the *motion* leg of "not color alone" above without removing it, which is why the
  text-label pairing is load-bearing rather than a nicety.
- **Contrast & theme.** The green shifts by theme (`#5F7E5A` light / `#86A67E` dark — indicative,
  P0-4 locks the final value) so it stays legible on washi and on night grounds.
- **Unambiguous by construction.** One reserved color, spent on exactly one meaning — audio is
  being recorded — so every state resolves to green-or-not with no in-between. That is what makes
  "am I being recorded?" answerable at a glance, however many states the capture engine has.

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
- **→ P1-5 (runtime listening indicator):** the full behavior spec above — the state set (idle,
  starting, listening, degraded, reconnecting), wake/settle transition, breathing baseline, the
  capped voice-amplitude "whisper" mapping, and the
  reduced-motion floor — plus the requirement that the mark reflect **real capture state** and be
  unambiguous enough to serve as the **consent signal**. `design/spirit-mark.html` is the reference
  implementation to build against (its voice envelope is *simulated*; P1-5 substitutes real audio).

---

*Concept, not final: the exact tokens are locked in P0-4 and the runtime is built in P1-5.*
*This document and [`design/spirit-mark.html`](../design/spirit-mark.html) together specify the mark
that `DESIGN.md` reserved the green and the feeling for.*
