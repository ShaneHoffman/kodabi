# Kodabi — the Grove design system

*Status: Living (Phase 4, the Grove redesign). This document was rewritten from scratch when Grove
replaced the pre-Grove system; nothing below describes the old one.*

Three documents describe the look, and they divide cleanly:

| Document | Fixes | Changes when |
| --- | --- | --- |
| [`docs/DESIGN.md`](DESIGN.md) | The **aesthetic** — the four principles, the reference class, what we refuse | Almost never |
| **This document** | The **doctrine** — what colour means, what shape means, what may move, what must be legible | A new interaction or state appears |
| [`docs/UI_CONVENTIONS.md`](UI_CONVENTIONS.md) | The **mechanics** — how to spell it: utilities, variants, the primitive catalogue | A primitive is added or changed |

The material all three describe is the `@theme` block in [`src/index.css`](../src/index.css).

**The point of this document is that a contributor styling a new component makes no judgment calls
about meaning.** Grove is a small vocabulary used strictly: a colour tells you what kind of thing
something is, a shape tells you what it does, and motion tells you where it came from. Most of the
rules below are prohibitions, because the failure mode of a small vocabulary is that it quietly grows.

---

## 0. What Grove is

The app is a grove at night. One ground plane, lit from the top left by a green wash and from the
bottom right by a warm one, with panes of frosted glass floating on it. Every surface is the same
ground seen through more or less glass — there is no second background colour, and no card that is
simply "lighter grey".

Two variants remap the tokens and nothing else:

- **`.day`** — the grove in daylight. Not "light mode with the same numbers": ink inverts, and every
  hue darkens far enough to be read on a pale ground.
- **`.hc`** — more contrast. A promotion, not a palette (§6).

They combine. `.hc.day` is the high-contrast day grove. Night is the default and carries no class.

---

## 1. Type

### Three faces, three jobs, no overlap

| Token | Face | Carries |
| --- | --- | --- |
| `font-ui` | Bahnschrift, "Segoe UI" | The interface's own voice: labels, buttons, titles, menu rows |
| `font-data` | Cascadia Mono, Consolas | Anything that lines up in a column: clocks, counts, ids, paths, eyebrows |
| `font-note` | Georgia, Cambria | Reading prose. The only face that carries a paragraph |

All three ship with Windows, so **Grove fetches no webfont**. If a fourth face is ever proposed, the
question to answer first is which of these three it is taking work away from.

`font-data` is doing more than looking technical. A clock that re-renders every second, a count that
changes when a note is filed, and an id you compare by eye all need figures that do not shift
horizontally — so anything numeric that updates in place also takes `tabular-nums`.

### Eyebrows are data, not headings

The small uppercase label above a group (`FOLDERS`, `SESSION`, `KODABI`) is set in `font-data` at
10px with wide tracking. It is a signpost, not a title: it never takes ink strength, and there is
never more than one level of it on a surface.

### Every view opens at one step

A view's title is 26px semibold in `font-ui`, with its summary in `font-data` on the same baseline.
One step for the Inbox, a folder, Settings and the terminal alike.

The pre-Grove rule was the opposite: each kind of view had its own serif title step, on the theory
that a config panel and a note must not open at the same size. What told them apart in the running
app was never the heading — it was the density and the shape below it, which is what the stance
still fixes. Sizing the head per view only made the window's volume change when you clicked
something. The note editor came last and came down: a document's title used to open at 36px against
every other screen's 26px, and it now opens at the one step like everything else. What tells a note
apart is the serif measure under it, not a bigger heading.

### Structure stretches, prose keeps its measure

The single most important typographic rule in Grove, and the one a full-width layout breaks first.

- **Structure fills the space it is given.** Lists, rows, cards, tables, and the dock all stretch to
  the panel width. A row that stops short of the edge looks broken.
- **Prose stops at its measure.** Note bodies at `66ch`, a chat answer at `78ch`, a settings hint at
  `46ch`. When the panel is wider than the measure, the leftover width becomes a details rail — not
  longer lines.

`text-note` carries the reading size and its leading together (15px / 1.75) so a note can never open
at two different sizes on two different screens.

---

## 2. Colour and shape

### Green is the kodama's voice

**Green means the system is doing something with your words.** That is its entire meaning, and the
list of things allowed to be green is closed:

- the **kodama** itself, while audio is flowing
- the **caret** in any input
- a **search match** the system found
- a **routing suggestion** the system is offering
- **selected text**, which is the same act of marking a match wears, from the user's side rather
  than the system's — a 25% kodama wash, spelled as a `color-mix` of the token so `.day` re-themes
  it (`::selection`, `src/index.css` §3)

It is **never** progress, a count, a chart, a success tick, a hover state, or decoration. Progress in
particular is information rather than voice, so a lit meter segment is ink at half strength.

`--color-kodama` is the mark; `--color-kodama-ink` is the step green takes when it has to carry text
(a highlighted match), because the mark value is not a reading value in day.

**On the resident capture overlay, the mark is the only green thing.** The always-on-top capture
pill (`CaptureOverlayPill`) keeps its status label at dim ink in every state, so a capture running
over somebody else's full-screen application announces itself with exactly one green element. A
pill that is half green reads as an alert; this overlay's job is to be calm and unmissable at once,
for an hour at a time, on a screen it does not own.

The other two capture indicators deliberately differ, and both step their live label up to
`kodama-ink`: the in-app listen pill (`ListenPill`), because inside Kodabi's own window it is a
piece of the app's chrome rather than a mark laid over another program, and `RecordingStatus`
inside the quick-capture window, which floats over the desktop too but is summoned by the user,
holds focus, and dismisses on blur — a surface being looked at for seconds, not one sitting
unattended over someone else's work. The rule is about the *resident* indicator, not about every
window that floats.

### Marigold is failure, and nothing else

`--color-warn` is reserved for failure surfaces: a capture that could not be transcribed, a card in
Needs attention. **It is deliberately absent from the folder palette**, so "a project" and "something
went wrong" can never be confused at a glance. `--color-danger` is its destructive-confirmation
sibling.

### Folder hues are identity, never status

Coral, cobalt, teal, plum. A hue answers *which project*, and nothing else — never priority, never
age, never health. They appear as markers first (a dot, a card's left border) and as text only for
the one-word routing guess, which is why each carries a day value dark enough to read.

### Rectangles act, pills represent

The shape rule, and it is exact:

| Shape | Radius | Padding | Is |
| --- | --- | --- | --- |
| **Rectangle** | `rounded-button` (10px) | 8×16 | An **action**. File, Retry, Send, Delete, dialog buttons |
| **Pill** | `rounded-pill` (999px) | 8×14 | A **token**: a thing that opens or represents something. The listen pill, artifact chips, citations |

A pill that performs a verb is a bug, and so is a rectangle that stands for a note. When in doubt:
if the label is a verb, it is a rectangle.

### One press spec

Every pressable thing in Grove presses the same way: **`scale-97` over 140ms on `ease-out-strong`.**
Not 0.95 on one control and 0.98 on another; not a colour flash instead. The press is the one state
allowed to move its own box, and it moves nothing else on the page.

### Hover enhances, never reveals

A control is fully legible before the pointer arrives. Hover may lift a card, brighten a fill, or
raise ink from dim to full — it may not be how you discover that something is clickable, and it may
not be the only way to reach an action. Everything hover offers is reachable by keyboard.

### Focus is one recipe

`outline: 2px` in ink, offset 2px outward — or inset 2px where the focusable thing fills its
container (a menu row, a list row), because an outward ring on a full-bleed row is clipped. It is
never a colour change alone, and it is never removed without a replacement.

### A state change never changes the layout box

Hover, focus, selection, and active states change colour, fill, shadow, and outline. They never
change padding, border width, font size, or weight — anything that would nudge a neighbour. A row
that reflows as the pointer crosses it is the specific defect this rule exists to prevent. Where a
state needs a border that was not there, the resting state carries a transparent one of the same
width.

### Selected is value, never hue

An active dock item, a selected row, an open control: all of them are a brighter wash of white (or
of ink, in day) plus an edge. Never a folder hue, and never green — those two already mean something
else.

---

## 3. View states

Every view answers four questions, and a view that answers only the first is unfinished.

- **Loading** renders nothing rather than a spinner. Kodabi's reads are local and fast; a flash of
  skeleton is more disruptive than an extra 40ms of the previous frame. The exception is work with a
  real duration (transcription, distillation), which gets the **pipeline placeholder**: a card in
  ink, with a pulsing dot and the current phase named in words.
- **Empty** is first-run copy, not an apology. An idle kodama, one line saying what belongs here, and
  one line saying how it gets here. "Nothing waiting." over "No items found."
- **Error** names what failed and what happens next, in the user's words. It never leaks an exception
  string, and it always leaves the underlying data reachable: "The audio is safe; retrying will pick
  up from the recording on disk."
- **Success** is the absence of noise plus, where the thing left the screen, a toast that says where
  it went and offers a way back to it.

---

## 4. Motion

### The vocabulary

| Move | Duration | Curve | Used by |
| --- | --- | --- | --- |
| **Press** | 140ms | `ease-out-strong` | Every pressable thing (§2) |
| **Exit** | 110–130ms | `ease` | Anything leaving: menus, dialogs, toasts |
| **Vanish** | 220ms card, inside a 280ms collapse | `ease-in-out-strong` | A row leaving a working list under its own power: the Inbox's routed capture |
| **Materialize** | 220ms | `ease-out-strong` | A surface arriving: menu, dialog, toast |
| **Rise-in** | 280ms, 45ms stagger | `ease-out-strong` | A list of rows appearing |
| **Morph** | 300ms | `ease-out-strong` | A surface that stays put and changes what it means: the listen pill going on air, the kodama's core taking the green |

**Exits are faster than entrances, always.** The user has already decided; waiting on the way out is
what makes an interface feel slow.

**Vanish is the sanctioned exception to that, and only because it is not a dismissal.** A menu closing
is the user getting out of the way; a routed capture leaving the Inbox is the app showing its work,
and it is the one departure the user is watching rather than waiting through. So it runs long enough
to be read: the card slides left out of a 2px blur over 220ms while the slot beneath it closes over
280, the two overlapping so the space is handed back behind a card that has already gone and the rows
below glide up instead of snapping. A row that leaves because the user disposed of it gets the same
movement for the same reason — they aimed at it, so they are looking at it. Under reduced motion it is
a fade and nothing else: no travel, no collapse.

**Morph is the one that is slower than an entrance, and deliberately.** Nothing arrives and nothing
leaves — a thing already on screen changes state — so the length is what makes the change legible as
a change rather than a repaint. Everything morphing together must share it: the listen pill's fill,
edge and label and the mark's core all take 300ms, which is what makes going on air read as one move
rather than a pill and a dot agreeing by luck (docs/SPIRIT_MARK.md).

**The command palette materializes in 200ms and leaves in 110ms**, the short end of both bands, spelled
at the call site rather than taken from `animate-materialize`. Ctrl K is a hundred-times-a-day action:
a surface summoned that often has to be *there*, and 20ms of extra arrival is 20ms the user spends
waiting to type. It is the same keyframe at a different length, not a different move.

### Surfaces materialize

`animate-materialize` scales from 0.96 and resolves out of a 4px blur. The blur is the load-bearing
part: it is what makes a menu read as *coming into focus* rather than being pasted on, and it is why
a plain fade looks cheap next to it. Each surface materializes from its own origin — a menu from the
corner it hangs off, a dialog from its centre, a toast from the corner it sits in.

### Lists rise in

`animate-rise-in` lifts each row 8px with a 45ms stagger. The stagger is capped: past about five rows
it stops reading as a cascade and starts reading as lag, so longer lists animate only the first
screenful.

### What does not animate

Text does not animate in. Numbers that update in place do not count up. Nothing loops except the
kodama and the caret, and both are the app telling you it is awake.

### Reduced motion keeps opacity life

The rule, and the reasoning behind it, in one line: **movement is the accessibility problem; life is
not.**

So under `prefers-reduced-motion` Grove does not floor every duration to zero. It swaps the moving
animation for its opacity-only partner, at the call site, with the `motion-reduce:` variant:

| Normal | Reduced |
| --- | --- |
| `animate-materialize` | `animate-fade-in` |
| `animate-rise-in` | `animate-fade-in` |
| `animate-dissolve` | `animate-fade-out` |
| `animate-halo` | `animate-halo-still` |
| `animate-ring` | `animate-halo-still` |
| `animate-breathe`, `animate-drift`, `animate-drift-back` | none |
| `active:scale-97` | no press transform |
| a switch knob's `translate` | `duration-0` — the same move, arriving at once |

`animate-caret` and `animate-pending` are opacity-only already and are left alone.

**The last row is the exception the rule needs, and it is not a floor by the back door.** Every swap
above removes a move because the move was *decoration on top of* the information. A switch knob's
position IS the information — it is what tells you the switch is on, since no track fill quiet enough
to sit on a card clears 3:1 (§6) — so cancelling it would cancel the state, not the animation. It
therefore keeps the property and drops only the time. **Gate a duration this way exactly when the end
state, not the travel, is what the user is reading.** Anywhere else, swap the animation.

`animate-halo-still` is the shape of this whole rule. A duration floor could not have produced it: it
would have frozen the aura mid-rotation at whatever shape it happened to hold, which is exactly the
bug the pre-Grove app shipped. The still halo keeps the same breath, held at one size.

**Writing the swap at the call site is deliberate.** It is visible in the className, which means it
is visible in review — where a media query buried in a stylesheet was not, and got forgotten.

---

## 5. Elevation and glass

### One recipe

A raised surface in Grove is four things at once, and dropping any one of them makes it read as a
hole rather than a pane:

1. a **translucent fill** (white over the night ground; white at higher alpha in day)
2. a **backdrop blur** with a little saturation, so the ground's glow bends through it
3. an **inset lit top edge** (`--color-edge-lit`) — the highlight that says light is falling on it
4. a **deep, soft shadow** beneath

In day, weight shifts between (3) and (4): the highlight does the separating, because a shadow on a
pale ground reads as dirt.

### The material comes in nine thicknesses

Each is a named recipe in `src/index.css` §3, carrying all four parts plus its rung of the ladder
below — bar the two deliberate drops argued for underneath the table (a card's blur, the pill's
shadow). Reach for the one that matches the job; do not assemble a tenth by hand.

| Recipe | Is | Blur |
| --- | --- | --- |
| `glass-top` | The window's top bar, flush to the edge | 24 / 160% |
| `glass-dock` | The navigation rail. Darker than the panel: a rail is a held thing | 26 / 160% |
| `glass-panel` | The main pane. The thinnest fill, so the ground's glow bends through it | 28 / 150% |
| `glass-card` | A card, and anything near-solid **inside** a panel | **none** |
| `glass-overlay` | Menus and toasts | 32 / 160% |
| `glass-dialog` | Dialogs: the same material, the longest shadow | 32 / 160% |
| `glass-palette` | The command palette. The dialog's shadow at the card's radius | 36 / 160% |
| `glass-pill` | The capture overlay pill: a whole window, over the desktop | 28 / 160% |
| `glass-sheet` | The quick-capture window: panel-round, dialog-deep, over the desktop | 32 / 160% |

**The last two float over the desktop, not over the app, and that changes two of the four parts.**
Every other rung is tinted against the ground and edged with `--color-edge`, both measured against
that ground. A transparent capture window has no ground behind it — it has whatever the user happens
to be doing — so its fill is its own dark tint and its night border is a literal 0.16 rather than the
token's 0.11. They still hand the border back to `--color-edge` under `.hc`, on **both** grounds
rather than only `.hc.day`, since a literal night border would otherwise swallow the night half of
the contrast promotion.

**For `glass-pill` it changes a third: it is the one rung with no shadow at all.** The pill fills
its window edge to edge — deliberately, because a transparent webview window is not click-through,
so a margin the user cannot see would still take the mouse. Flush to those bounds a shadow has
nowhere to fade: it clips flat and composites over the application behind it as a dark rounded
wall, which is the opposite of depth. The border and the inset lit edge separate it alone.
`glass-sheet` keeps its shadow because its window does give it room (`p-10` of it).

**The palette is the thickest rung, and that is the rule rather than an exception to it.** A bigger
pane has more of the app behind it to push back, so it has to read as a thicker material or it reads
as a hole. It keeps the card's 14px because what it holds is a list of rows, and a dialog's 16px
would bow out past the topmost of them.

**A card carries no backdrop blur, deliberately.** It floats on a surface that is already blurring
the ground, and blurring a blurred image again is mud, not depth — so a card's fill goes near-opaque
and its depth comes from the lit edge and the shadow alone.

**The card rung has a hover companion, `glass-card-lift`, and it is not a tenth thickness.** It
changes no fill and no blur: it deepens the shadow and brightens the lit edge, which is what the same
material looks like a little closer to the light. It exists because those two values are ground-
specific literals, and a literal colour may not be written in a `className` (§7) — so the pointer
state of a card is a recipe for the same reason the card itself is. Written `hover:glass-card-lift`,
and only where a lift is the right signal: an Inbox row, where every card is a thing to be handled.

**It repaints `border-color`, all four sides, from a `:hover` rule — so a card that colours one edge
for CONTENT must restate that colour at the hover layer.** `.hover\:glass-card-lift:hover` weighs
(0,2,0) against a bare `border-l-coral`'s (0,1,0), so it wins on specificity whatever order Tailwind
emits them in, and the content colour drops off the edge exactly while the pointer is on the card.
The Inbox row's guess hue is written `border-l-coral hover:border-l-coral` for that reason. Colouring
an edge is the only conflict here: nothing else the recipe sets is a property a card is likely to
have opinions about.

### The one surface that goes down

`glass-term` is the material inverted: a **well** sunk into a panel rather than a pane raised off the
ground. It is the terminal's, and it is spelled by inverting the two parts that do the raising.

| Part | A pane | The well |
| --- | --- | --- |
| Fill | White (or ink in day) **over** the ground, so it lightens | Black at 35% — the only fill in Grove that darkens what it sits on. Day inverts to ink at 6%, far shallower, because ink over a pale ground carries much further than black over a dark one |
| Backdrop blur | Its rung's | **None.** What it holds is a live terminal, and blurring the ground behind the densest text on screen buys nothing and costs a compositor pass on every frame the PTY paints |
| Lit top edge | `--color-edge-lit` | The same. A recess lit from above catches the light on its top lip exactly as a pane does, and without it the well stops reading as a depth in the panel and starts reading as a black rectangle laid on one |
| Shadow | Deep and soft, beneath | **None.** A recess casts none |
| Radius | Its rung of the ladder | 12px, off the ladder — the search field's shape, because it is the same kind of thing: a container for what the user types into |

It is **not a tenth thickness**, for the same reason `glass-card-lift` is not: the nine are one
material at nine depths above the page, and this is the other side of the page. Reach for it when
something is genuinely *recessed into* a surface. Everything else that sits inside a panel is a card.

It still carries the `prefers-reduced-transparency` branch, because the rule for that branch is a
translucent fill **or** a `backdrop-filter`, and it has the first. Each solid there is this fill
composited over `glass-panel`'s own solid, so the well keeps its depth on a machine that does not do
glass.

### The ladder

| Layer | Radius | Is |
| --- | --- | --- |
| Panel, dock | `rounded-panel` (18px) | The furniture. Sits on the ground, never moves |
| Card, menu, toast, palette | `rounded-card` (14px) | Content and transient surfaces |
| Dialog | `rounded-dialog` (16px) | Card-sized but window-like, so between the two |
| Button | `rounded-button` (10px) | An action (§2) |
| Pill | `rounded-pill` (999px) | A token (§2) |

The ladder reads as containment: a panel holds a card holds a button. A card with a panel's radius
looks like furniture that came loose.

### Scrims are a veil, not a blackout

A modal's scrim is light (~28% ink; `glass-scrim`). The palette's glass only reads as glass if the app
stays visible enough beneath it to blur — a heavy scrim turns an expensive frosted surface into a
grey box.

### Reduced transparency means frosted solids

Under `prefers-reduced-transparency`, every glass surface takes a **solid** colour sampled from what
it looked like composited, and drops its `backdrop-filter`. The layout, the radii, the edges, and the
shadows are unchanged — the app should look like the same design rendered on a machine that does not
do glass, not like a different app.

The scrim is the one exception, and not an omission: it has no `backdrop-filter` to drop, and a scrim
made opaque would blank the app rather than reveal it.

Because the swap removes a *property* rather than remapping a value, it cannot live in a token or a
variant — which is why the nine thickness recipes and `glass-term` are `@utility` blocks with the day
branch and the reduced-transparency branch nested inside each one, emitted together or not at all.

`glass-card-lift` carries only the day branch, and that is correct rather than an omission: it sets a
shadow and a border colour, neither of which the reduced-transparency swap touches (it changes fills
and blurs). A recipe needs the third branch exactly when it has a `backdrop-filter` or a translucent
fill to give up.

---

## 6. The accessibility floor

### Contrast, measured

Every ink step clears **4.5:1 on the ground, on the glass panel above it, and on a card inside that
panel**. Night's card is the tightest surface in the system, and that is the number that matters:
faint metadata is almost never rendered directly on the ground.

Measured with a WCAG 2.1 relative-luminance check, alpha composited first (night panel resolves to
`#1c211b` and its card to `#1c2318`; day panel to `#f6f7f1` and its card to `#fdfdfc`):

| Token | Night | on ground | on panel | on card | Day | on ground | on panel | on card |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `ink` | `#eef2e7` | 16.02 | 14.41 | 14.14 | `#1e2418` | 13.67 | 14.76 | 15.64 |
| `ink-read` | `#d7ddcd` | 13.10 | 11.78 | 11.56 | `#333d29` | 9.81 | 10.59 | 11.23 |
| `ink-dim` | `#a6b09b` | 8.06 | 7.25 | 7.12 | `#55604a` | 5.72 | 6.17 | 6.54 |
| `ink-faint` | `#838d78` | 5.24 | 4.71 | **4.62** | `#5f6a54` | 4.91 | 5.30 | 5.62 |

`ink-faint` is the step that sets the floor, and it is the reason the night value is `#838d78` rather
than the `#7e8873` first drawn: that value cleared 4.90 on the ground but only **4.41 on the panel**,
which is where faint text actually renders. It was lightened until the tightest of the three numbers
cleared 4.5 — and that number is the card's 4.62, with only 0.12 of headroom. **A surface darker
than `glass-card` would put faint metadata under the floor**, which is the constraint any new
material has to clear.

**`ink-faint` is a metadata register and is spent on no other text.** Timestamps, counts, ids,
eyebrows, keyboard hints. The moment it carries a sentence the user has to read, it is the wrong
token.

The one thing it colours that is not text is the spirit-mark's dormant core (`src/index.css` §3),
and that is the same claim in another material: the mark at idle is metadata about the capture, not
a lit indicator. It stays legal because a graphic answers to the 3:1 non-text floor, which the step
clears on every surface in the table above with room to spare.

### Hues as text

Folder hues and green are **markers first**. Where one has to carry text, day uses the darker value
in the `.day` block, which is why day's hues are not simply the night values dimmed:

| Token | Night on ground | Day on ground |
| --- | --- | --- |
| `kodama` | 9.89 | 4.26 |
| `kodama-ink` | 13.61 | 6.24 |
| `warn` | 9.49 | 3.67 |
| `coral` | 6.43 | 4.00 |
| `cobalt` | 6.31 | 5.24 |
| `teal` | 7.61 | 4.67 |
| `plum` | 6.83 | 5.37 |

Three day values sit between 3:1 and 4.5:1 — `kodama` (4.26), `coral` (4.00), and `warn` (3.67).
**That is the boundary of what a hue is allowed to do in day.** Each clears the 3:1 non-text floor,
so each is fine as a dot, a border, an icon, or large text; none of them may be the colour of a
sentence. When a hue must carry running text in day, the accompanying ink token carries it and the
hue moves to the marker beside it.

**One label sits deliberately at that boundary: the quick-capture routing guess.** `→ briarwood-golf`
wears its project's hue in both variants, dot and slug together, because the guess *is* the identity
of a folder and splitting it into a coloured dot beside grey text says two things where the user
reads one. It is a three-word `font-data` label standing next to its own hue dot with the filing
verb ("saves and routes it") a few characters to its left, not a sentence anyone reads for meaning —
so day coral at 4.00 is the accepted case, not a precedent. A hue still never colours running text.

### Edges are decorative

`--color-edge` sits well under 3:1 in both variants by design (1.40 night, 1.30 day; 2.36 and 1.99
under `.hc`). **No boundary in Grove is the only thing carrying a meaning.** A card's grouping is
also its spacing and its fill; a field's focus is also its ring. If a border is the sole signal, that
is the bug, not the contrast number.

### More contrast is a token remap

`.hc` moves exactly three things:

- **`ink-faint` is promoted** — metadata stops whispering. At night that is exactly the `ink-dim`
  value (`#a6b09b`). In day it goes one step *past* dim, to `#4a543f` (6.86 on the ground) rather
  than dim's `#55604a` (5.72): a pale ground gives faint ink less room to recover, so matching dim
  would have left day's promotion visibly weaker than night's. The two variants promote to the same
  *perceived* step, not to the same token.
- **`edge` takes a stronger alpha** (.26 night, .32 day) — structure stops being implied.
- **`switch-on` takes a stronger alpha** (.30 night, .22 day, from .22 and .14) — a switch's track is
  the one fill in the system whose job is to report a **state** rather than to be a surface, and a
  state readout is exactly what more contrast is asked for. It is the addition the rule below
  licenses: no ratio in the table above justifies it, because a resting track never clears 3:1 at any
  alpha quiet enough to belong on a card. What clears is the knob — ink on that track, and 18px of
  travel — which is why the fill is allowed to be a hint and why promoting it is a strengthening of
  the hint rather than a repair of a signal.

Nothing else moves, because §6's table shows nothing else needs to. That is the test for any future
addition here: if a token is proposed for the `.hc` block, the ratio that justifies it should be
missing from the table above.

**The hosted terminal's 16 ANSI colours are the one palette outside this, and deliberately.** They
are not Grove's voice — they are the colours a hosted program asked for, and remapping them would
change what that program is saying, not how sharply it says it. xterm's own `minimumContrastRatio`
is the right lever there, since it lifts a foreground against whatever background it actually landed
on rather than second-guessing the palette. The pane's own chrome is Grove's and takes the promotion
normally: its edge is `--color-edge`, its foreground is `ink-read`, and its caret is `kodama`.

It reaches the DOM two ways, and it is **additive**: the in-app toggle and the OS
`prefers-contrast: more` are OR-ed in [`src/contrast.ts`](../src/contrast.ts), so turning the app
toggle off cannot sharpen the app away from someone whose OS asked for it. Only the app preference is
persisted. `src/contrast.test.ts` pins that property.

### Focus order, live regions, keyboard paths

- **Focus order follows reading order.** A dialog traps focus, returns it to the trigger on close, and
  takes it on open — to the first field, or to the safe action where the dialog is destructive.
- **Anything that changes without a click announces itself.** Status lines are `role="status"`;
  failures are `role="alert"`. A toast that reports where a note went is announced; a hover tooltip is
  not.
- **Every action has a keyboard path**, and destructive actions never sit on a keyboard path where a
  stray Enter reaches them first.

---

## 7. Enforcement

Grove is enforced far more lightly than the system it replaced, and that is deliberate. The old guard
scanned stylesheets for literal colours, durations, and spacing — a check that only made sense while
every screen had a stylesheet. Grove has one, and it is the app's only one.

What remains machine-checked, in `pnpm exec eslint . --max-warnings=0`:

- **No colour literals in `className`.** A hex in a class string is a value no theme block can
  re-map, so it survives `.day` and `.hc` unchanged and breaks both variants silently. This is the
  one literal that is not merely untidy but wrong.
- **No `.css` import outside `src/index.css`.** There is nothing left to except: the app's own code
  imports no other stylesheet, and the single surviving `eslint-disable` covers xterm's third-party
  one. A new import is possible, but it has to be argued in a comment. Keeping the count at one is
  also what keeps the token-shadowing trap closed — `@theme` emits into `@layer theme`, and an
  unlayered declaration of the same name would silently win (UI_CONVENTIONS §6).

And in `pnpm test`:

- **[`src/theme.test.ts`](../src/theme.test.ts)** pins that `.day` is *resolved* under "system":
  Grove's tokens carry no media query of their own, so `applyTheme` has to read
  `prefers-color-scheme` and decide, and a build where the class never moved would strand every
  window at night.
- **[`src/contrast.test.ts`](../src/contrast.test.ts)** pins that `.hc` is the OR of the two requests,
  which is what makes the switch additive.
- **[`PrimitiveGallery.test.tsx`](../src/components/dev/PrimitiveGallery.test.tsx)** renders every
  Grove **control** under all four grounds (night, day, `.hc`, `.hc.day`). Controls, not all ten of
  UI_CONVENTIONS §4's primitives: `ViewFrame` is a page scaffold with nothing to show in a catalogue
  row and has its own tests, and the three overlays are summoned rather than resident. It proves
  they *render*, not that they look right — the looking is what `/gallery.html` is for, served by
  `pnpm dev` and absent from the build.

**Everything else in this document is review's job, and this document is the checklist.** Nothing
checks that green stayed on the kodama, that a verb got a rectangle, that an exit came in under its
entrance, or that a hue is not carrying a sentence in day. Those are the rules most worth holding, and
they are held by reading the diff against the sections above.

The one thing worth stating plainly: **the absence of a guard is not permission.** The pre-Grove
system had a guard for spacing and still grew a parallel field system on one screen, because a guard
only catches what it was written to see.
