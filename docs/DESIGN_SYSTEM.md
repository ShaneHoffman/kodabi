# Kodabi — design system (states, motion, elevation, accessibility)

*Status: Living (Phase 3, design-system pass). Three documents describe the look, and they divide
cleanly:*

| Document | Fixes | Changes when |
| --- | --- | --- |
| [`docs/DESIGN.md`](DESIGN.md) | The **aesthetic** — the four principles, the reference class, what we refuse | Never (locked in Phase 0) |
| **This document** | The **system** — how a control behaves, what a view shows when it has nothing, what may move, what must be legible | A new interaction or state appears |
| [`docs/UI_CONVENTIONS.md`](UI_CONVENTIONS.md) | The **mechanics** — spacing steps and the primitive catalogue | A primitive is added or changed |

[`design/tokens.css`](../design/tokens.css) is the material all three describe.

**The point of this document is that a contributor styling a new component makes no judgment calls.**
Every question below is answered with a named token. If you find yourself choosing a value, the
answer is missing here and belongs here.

---

## 1. Typography and density

### The scale

| Step | Token | Role | Never |
| --- | --- | --- | --- |
| `text-eyebrow` | `--fs-eyebrow` .72rem | Section eyebrows only, always with `uppercase tracking-eyebrow` | Body copy |
| `text-cap` | `--fs-cap` .8rem | Captions, meta lines, hints, status text | A paragraph someone reads |
| `text-body` | `--fs-body` 1.06rem | Interface body, list rows, buttons | — |
| `text-read` | `--fs-read` 1.18rem | Note bodies, prose (with `font-serif`) | Interface chrome |
| `text-h3` | `--fs-h3` | Sub-headings inside a view | — |
| `text-h2` | `--fs-h2` | The view title (with `font-serif`) | More than once per view |
| `text-display` | `--fs-display` | Reserved. Nothing ships with it yet | Any current screen |

Sizes always come from this scale. Never `text-[13px]`, never a raw `font-size`. Enforced by the
eslint rule described in §7.

### The eyebrow is one thing

A section eyebrow is exactly `text-eyebrow uppercase tracking-eyebrow text-text-faint`.

`tracking-eyebrow` bridges `--ls-eyebrow` (0.22em). **Never Tailwind's `tracking-wide`** (0.025em).
Before this pass the Sidebar used the token and twelve other call sites used the Tailwind default,
so the same role rendered 2.53px and 0.29px apart at identical font-size. There is one eyebrow.

An eyebrow labels a *section*. It is not a field label (that is `text-cap text-text-soft`, owned by
`TextField`) and not a status line (that is `text-cap`).

### Weight and colour carry rank before size does

Per DESIGN.md, value carries the hierarchy. In practice:

- **Rank a row** with `--fw-medium` (`font-medium`) and `text-text`, not with a larger size.
- **Recede** with `text-text-soft`, then `text-text-faint`. Never with a smaller size than `text-cap`.
- **Never rank with hue.** `--accent` marks *interactive*, `--accent-dot` marks *recording*. Neither
  ranks anything.

### List density

| Property | Value | Applies to |
| --- | --- | --- |
| Row vertical padding | `py-2xs` (8px) | Every list row |
| Title ↔ action column gap | `gap-md` (24px) | Rows with a trailing control |
| Row group gap | `gap-3xs` (4px) | Tight nav lists (Sidebar) |
| Between list sections | `gap-lg` (40px) | Views composing several lists |

Rows align `items-start` when the row has a multi-line body, `items-baseline` when it is a single
line. Pick by content, not by view.

**A list is not a table.** No column rules, no zebra striping, no borders (DESIGN.md refuses
admin-panel density). Separation is space and value.

**A subordinate section may not out-measure its subject.** An exception block (the Inbox's "Needs
attention") that grows unbounded will bury the content the view is named for. Cap it and let it
expand, so the view's actual subject always leads.

---

## 2. Interaction states

Every control shows all five where they apply. The primitives in `src/components/ui/` bake these in,
so a screen composing primitives gets them for free and must not restate them.

| State | Treatment | Token |
| --- | --- | --- |
| **Rest** | Per variant | — |
| **Hover** | Value step toward `--text`, over `--dur-quick` | `--dur-quick`, `--ease-standard` |
| **Focus-visible** | 2px accent outline, offset by its own width | `.ui-focus-ring` |
| **Active** (pressed) | One value step past hover, no transition (a press must feel immediate) | — |
| **Disabled** | `text-text-faint` + `cursor-not-allowed`; controls with no text to fade use `--disabled-opacity` | `--disabled-opacity` |
| **Destructive** | See below — a confirmation, not a colour | — |

### Focus is one recipe, applied by class

```css
.ui-focus-ring:focus-visible {
  outline: var(--focus-width) solid var(--accent);
  outline-offset: var(--focus-offset);
}
```

Defined once in [`src/components/ui/ui.css`](../src/components/ui/ui.css). Add the class; never
retype the declarations, and never use plain `:focus` (a pointer press would draw it).

**One sanctioned exception:** the command palette input. Its dialog's only tabbable element is that
input, and it shows a caret, so a ring would be noise rather than information. A form field always keeps
its ring.

(The quick-capture textarea used to be a second exception on the same reasoning. It stopped being one
when the window gained a visible **File it** button: with two tab stops, the ring is the only thing that
says which one has focus. An exception justified by "there is only one control here" expires the moment
that stops being true.)

### Hover may enhance, never reveal

Every action is reachable and discoverable without a pointer. Hover adds emphasis to an
already-visible control. A control that only appears on hover does not exist for keyboard or touch.

**Hover and focus must target the same element.** Putting `hover:` on an inner span while the outer
button carries the ring means keyboard users get a ring with no colour change, and hovering the rest
of the row does nothing.

### Destructive is a confirmation, not a colour

There is no red token, and DESIGN.md refuses hue as a ranking device. So a destructive action is not
marked by painting its control: it is marked by **making the user confirm**, and by the confirming
control being the non-default one (`quiet` beside a `primary` cancel).

No destructive action ships today — nothing in the app deletes anything — so no `variant="destructive"`
exists either. Shipping an unused variant would be inventing a look with nothing to check it against.
The first delete lands the variant and this rule together.

### Selected is value, never hue

```css
background: var(--wash-active);   /* an ink wash of --text, both themes */
```

Available as `.ui-wash`. **Never `--accent-dot`** for selection: the reserved green means audio is
being recorded, and a selected row wearing it is a lie.

---

## 3. The view state vocabulary

Every view that reads from disk has four states. `useVaultQuery` returns `{data, loading, error}`,
so all three are always available; there is no excuse for an unhandled one.

| State | Treatment | Component |
| --- | --- | --- |
| **Loading** | Nothing. The view appears when ready | — |
| **Empty** | One sentence at `text-body text-text-soft`, left-aligned, saying what *would* be here and how it gets here | `StatusMessage variant="empty"` |
| **Error** | One sentence at `text-body text-text-soft` with `role="alert"`, prefixed with what failed | `StatusMessage variant="error"` |
| **Success** | Usually the result itself. A discrete confirmation only where the surface closes | `StatusMessage variant="status"` |

### Loading renders nothing, deliberately

No spinners, no skeletons. A local disk read completes in milliseconds, and a spinner that flashes
for 30ms is worse than nothing. **But `loading` must still be consumed** — a view that ignores the
flag renders its *empty* state during the fetch, so every cold start flashes "No projects yet."
before the data lands. That is the bug this rule exists to prevent.

Gate the empty state on `!loading && !error && items.length === 0`. All three conditions.

For an operation the user *initiated* (a save, a retry, a re-route), pending state is shown on the
control via `Button loading` — never as a page-level treatment.

### Empty states are first-run copy

Never a blank pane. The copy says what belongs here and how it arrives:

> "Nothing waiting. Notes the router can't place land here."

Not "No items." An empty Inbox on first launch is the user's first impression of the product.

### Errors name what failed and never leak an exception

> "Couldn't load projects: {message}"

Not the bare string. Before this pass three surfaces rendered a raw
`TypeError: Cannot read properties of undefined` straight into the UI. **Prefix every error with the
action that failed**, so the sentence is readable even when the message is not.

Errors are `text-text-soft`, never red: there is no red token, and DESIGN.md does not rank with hue.
Weight and the `role="alert"` announcement carry the urgency.

### Success

Most successes are self-evident (the note appears). Confirm explicitly only when the surface
disappears before the user can see the result — quick capture flashes its destination for
`FLASH_MS` before the window hides. A settings toggle that visibly moved needs no "Saved".

---

## 4. Motion

> **Motion as feedback, not decoration.** The distill-and-route moment gets one satisfying
> transition. Everything else is near-instant. (FOUNDING_DOC §4)

### The durations

| Token | Value | Spent on |
| --- | --- | --- |
| `--dur-quick` | 120ms | Hover and colour changes on a control |
| `--dur-settle` | 200ms | A row leaving or entering a list |
| `--dur-wake` | 450ms | The spirit-mark waking and settling |
| `--dur-pulse` | 1600ms | Starting / reconnecting pulse |
| `--dur-breath` | 4200ms | The listening breath cycle |
| `--dur-drift` / `--dur-drift-slow` | 15s / 21s | The aura's counter-rotating blobs |

Easings: `--ease-standard` (state changes), `--ease-breath` (the continuous listening motion),
`--ease-drift` (constant-rate rotation). Never a bare `0.2s` or `ease-in-out` in a component.

### What animates

**Animates:** a control's own state change (`--dur-quick`); a row leaving a list (`--dur-settle`);
the spirit-mark, which is the app's one continuous motion.

**Never animates:**

- **A data refresh.** `vault:changed` refetches constantly. Animating it would make the app twitch
  whenever a file changed on disk.
- **Overlay entrance.** The palette, the consent nudge, quick capture and the capture pill all appear
  instantly. Showing the surface *is* the transition. A hotkey surface that fades in feels slow.
- **Layout.** Nothing reflows under the user.
- **Anything on a list of unknown length.** Staggered row animations are decoration.

### Reduced motion

`src/index.css` applies an app-wide floor: under `prefers-reduced-motion: reduce`, animations and
transitions collapse to 1ms.

A component needing a *different* reduced-motion resting state overrides it in its own CSS. The
spirit-mark is the precedent and the reason the floor is 1ms rather than `none`: it must settle to a
**still green mark**, because the green carries the recording state and a blank mark would imply
privacy that does not exist. Never let reduced motion remove *information*.

---

## 5. Elevation and overlay layers

Three planes, and nothing invents a fourth.

| Plane | Background | Shadow | z |
| --- | --- | --- | --- |
| Page | `bg-bg` (sidebar: `bg-bg-sink`) | none | auto |
| Raised | `bg-surface` | `--hairline` | auto |
| Dropdown | `bg-surface` | `--lift` | `--layer-dropdown` (10) |
| Modal | `bg-surface` | `--lift` | `--layer-overlay` (50) |

Separation between adjacent planes is a **value shift plus a hairline**, never a border
(DESIGN.md: space instead of borders and boxes). Hairlines are inset shadows:

```css
box-shadow: inset 0 -1px 0 var(--edge-faint);   /* one edge   */
box-shadow: var(--hairline);                    /* a full ring */
```

Use `--edge-faint` for a divider inside a surface, `--edge` for the edge of a control. Two adjacent
elements must not use different edge tokens for the same visual line.

### Modals

Every modal goes through the `Overlay` primitive, which owns the scrim (`--scrim`), the layer, and
backdrop dismissal. Dismissal fires on **click, not pointerdown**, and only when the gesture both
started and ended on the backdrop — otherwise a drag that begins inside the panel dismisses it, and
an unmount at pointerdown lets the rest of the gesture fall through to whatever was underneath.

A modal traps focus (§6), takes `role="dialog"` + `aria-modal="true"`, and closes on Escape.

---

## 6. Accessibility floor

### Contrast

Measured, not estimated. Text pairs against WCAG AA (4.5:1); the spirit-mark is a graphic (3:1).

**Light (washi day)**

| | on `bg` | on `bg-sink` | on `surface` |
| --- | --- | --- | --- |
| `text` | 13.32 | 12.48 | 11.55 |
| `text-soft` | 5.36 | 5.03 | 4.65 |
| `text-faint` | 5.25 | 4.92 | 4.55 |
| `accent` | 5.26 | 4.93 | 4.56 |
| `accent-dot` *(graphic)* | 3.95 | 3.70 | 3.42 |

**Dark (night)**

| | on `bg` | on `bg-sink` | on `surface` |
| --- | --- | --- | --- |
| `text` | 14.10 | 14.93 | 12.82 |
| `text-soft` | 8.11 | 8.58 | 7.37 |
| `text-faint` | 4.98 | 5.27 | 4.53 |
| `accent` | 7.48 | 7.92 | 6.80 |
| `accent-dot` *(graphic)* | 6.42 | 6.80 | 5.84 |

Every text pair clears 4.5. `--k-stone-ink` and `--k-accent` were re-tuned by 2/255 per channel
during this pass to clear it on `--surface`, the lightest raised plane.

**`--accent-dot` is a graphic, never text.** At 3.42–3.95 in the light theme it does not meet the
text floor, and it never has to: it is spent on the spirit-mark, where 3:1 applies and it passes. The
LISTENING *label* beside it is `--text` when live and `--text-faint` when not, so the state reads
through value like every other status line, and the green stays the mark's alone.

Any new colour pair is measured before it ships.

### Focus order

- **Every interactive element is a real `<button>`, `<a>`, or form control.** Never a `div` with
  `onClick`.
- **A modal traps focus** and restores it on close (`useDialogFocus`).
- **Focus must survive an optimistic swap.** Replacing the focused control with a `<span>Filing…</span>`
  drops focus to `<body>` mid-task and silently strips the user's place in the page. Keep the control
  mounted and use `Button loading` instead.
- **`disabled` drops focus too, so a *busy* control is `aria-disabled`, not `disabled`.** An element
  that is focused when it becomes disabled is blurred and focus resets to `<body>` (the HTML focus
  fixup rule) — the same failure as unmounting it, and worse inside a modal, whose Escape and Tab
  handling lives on an ancestor the focus has just left. `Button loading` therefore sets
  `aria-disabled` + `aria-busy` and swallows its own activation; the native attribute stays for
  `disabled`, which means "there is nothing to do here", not "something is in flight". A caller must
  not pass both for the same condition — `disabled` wins, and the focus goes.
  `Select` and `Checkbox` do not do this yet: their `disabled` prop is a genuine disable, so a write
  in flight behind one still costs the user their place in the page.
- **`aria-current="page"`** marks the selected navigation row.
- Items in an `aria-activedescendant` listbox are deliberately not tabbable; focus stays on the
  controlling input.

### Live regions

| Role | For | Politeness |
| --- | --- | --- |
| `role="status"` | Progress and success — "Transcribing…", "Note saved" | polite |
| `role="alert"` | Failures the user did not cause and may not be looking at | assertive |

**Every async failure gets `role="alert"`.** A retry that fails silently while the user looks
elsewhere is a failure the app never reported.

**Debounce a live region fed by a flapping source.** `ListeningIndicator` is the pattern to copy: the
mark reacts instantly for visual feedback, but the announced label follows
`useDebouncedValue(state, 400)`, so a flapping voice-activity detector cannot spam a screen reader.
The visual and the announcement are allowed to run at different speeds.

**One region per concern.** Capture state, transcription, and distill are three concerns and get
three regions. Do not merge them into one node whose text rewrites itself.

### Keyboard paths

Every mouse flow has a keyboard flow, **and the reverse**: a hotkey-first surface still needs a
visible control. Quick capture is submitted with Enter and also with a real button, because a
pointer-only user cannot press Enter into a window they never focused.

A value that commits only on blur is invisible. Commit on Enter as well, and show that it committed.

**One sanctioned exception:** the capture overlay pill's drag. Its window is `focusable: false` so
appearing over a full-screen app cannot steal focus, which also removes it from the tab order. The
keyboard paths that matter remain: the capture hotkey stops the capture and the pill with it, and the
Settings toggle disables it permanently.

---

## 7. Enforcement

Both guards run in gates CI already runs. Neither adds a dependency.

- **[`src/designTokens.test.ts`](../src/designTokens.test.ts)** (`pnpm test`) reads `design/tokens.css`
  and every `src/**/*.css`, and fails on a literal colour, font-family, or duration outside
  `tokens.css` — plus asserts each `--k-*` pigment is declared exactly once.
- **[`eslint.config.js`](../eslint.config.js)** (`pnpm exec eslint .`) fails numeric spacing utilities
  (`p-3`, `gap-4`) and arbitrary values (`text-[13px]`) inside `className`.

What they cannot catch — an eyebrow using the wrong tracking utility, a hover on the wrong element,
a missing `role="alert"` — is review's job, and this document is the checklist.
