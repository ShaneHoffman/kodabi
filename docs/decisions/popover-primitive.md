# Popover primitive: base-ui vs. the hand-rolled `Select`

**Decision: no headless UI dependency. The hand-rolled `Select` stays. Closed 2026-07-28.**

**Ticket #109 (`feat/select-menu-origin`) is _not_ subsumed — do it by hand, and widen it to
cover positioning as well as the entrance.**

This is the evidence record for the zero-UI-dependency posture asserted in
[`.claude/rules/typescript-style.md`](../../.claude/rules/typescript-style.md) and
[`docs/UI_CONVENTIONS.md`](../UI_CONVENTIONS.md). That rule already named `Select` as its
precedent; until now it asserted the position without measuring it. This document measures it.

No production change landed with this decision. The counter-prototype in §5 was built, gated and
run in the real app, then reverted; only this document and its cross-links commit.

## 1. What was asked, and what the default was

Ticket #114 asked whether a headless popover library should replace
`src/components/ui/Select.tsx`, which is hand-rolled: origin-aware positioning, focus handling,
typeahead, collision flipping and ARIA wiring, all currently maintained by hand.

The bar was not neutral. `.claude/rules/typescript-style.md` holds:

> **No new UI runtime dependencies without discussion.** The app holds a zero-UI-dependency
> posture — the hand-rolled `Select` primitive (a full combobox with no headless library) is the
> precedent. Add a dependency only after agreeing it's worth the weight.

So the default answer was no, and the burden was on adoption. Two things could have overturned it:
a cheap dependency that took the design tokens without a fight, or a gap in the hand-rolled control
that could not be closed without one. Both were tested. Neither held.

## 2. The candidate as it actually is (measured 2026-07-28)

**The ticket was written against stale information, in the library's favour and against it.** The
package renamed from `@base-ui-components/react` to `@base-ui/react`, and 1.0 shipped in Feb 2026.
**"It is immature" is not an available argument and is not used here.**

| | |
|---|---|
| Package | `@base-ui/react@1.6.0`, MIT, published 2026-06-18 |
| Unpacked | 9,282,630 bytes |
| Runtime deps | `@babel/runtime`, `@base-ui/utils`, `@floating-ui/utils`, `@floating-ui/react-dom`, `use-sync-external-store` |

The app has **10 runtime dependencies today**, none of which is a design-system or
component-primitive library: three `@fontsource` packages, `@tauri-apps/api`, React, react-dom, the
markdown pipeline (`react-markdown` + `remark-gfm`), and the two `@xterm` packages — the last of
which is a terminal widget, i.e. a non-substitutable platform binding rather than a UI kit.
Adoption is **+6**.

Note what `@floating-ui/react-dom` is: the JS positioning engine. It is the single largest thing
the library is bought for, and §5 replaces it with CSS.

### 2.1 Bundle cost, measured

The ticket says bundle cost is "largely irrelevant" in a desktop app. That is broadly right, and it
is not the reason for this decision — but the number was measured rather than waved at, in two
throwaway Vite projects outside the repo (one React baseline with a hand-rolled dropdown, one
identical but using `@base-ui/react/select`), production mode, esbuild minify:

| Build | Raw | Gzip | Modules |
|---|---|---|---|
| React baseline | 317.17 kB | 72.56 kB | 25 |
| + base-ui `Select` | 493.43 kB | 124.05 kB | 213 |
| **Delta** | **+176.26 kB** | **+51.49 kB** | **+188** |

For scale: the app's own main chunk is currently 564.52 kB / **154.36 kB gzip**. So one control
would add roughly **a third again** to the shipped main bundle. In a desktop app that is affordable.
It is listed for completeness, not as the argument.

### 2.2 Where the library's own docs mislead

`base-ui.com/react/components/select` reads as though `Select` uses `aria-activedescendant`. **The
source says otherwise**, and this matters because it is the exact thing `docs/DESIGN_SYSTEM.md` §6
legislates:

- `packages/react/src/select/item/SelectItem.tsx` sets `tabIndex: open && highlighted ? 0 : -1` —
  roving tabindex, moving real DOM focus into the popup.
- A code search over `packages/react/src/select` returns **zero** matches for `activedescendant`.
  The pattern *is* used in `combobox/`, `autocomplete/` and the shared `useListNavigation.ts` —
  just not in `Select`. (This asymmetry is load-bearing later; see §9.)

Other defaults read from source rather than docs:

- `packages/react/src/select/trigger/SelectTrigger.tsx` — `tabIndex: disabled ? -1 : 0`, driven by
  the native `disabled` attribute. **There is no `busy` equivalent.**
- `packages/react/src/select/root/SelectRoot.tsx` — `modal = true` by default (scroll lock,
  outside pointers disabled).
- `Select.Portal` is required in the component tree.

## 3. What the hand-rolled control actually is

`src/components/ui/Select.tsx` is 373 lines and imports React plus two blessed bridge hooks
(`useOutsidePointerDown`, 30 lines; `useScrollIntoView`, 21 lines). It has **four call sites in
three files**: two in `SettingsView.tsx` (retention, theme), one in `ConsentNudge.tsx` (retention,
inside a modal), and one per Inbox row in `InboxView.tsx` (the `variant="token"` file picker).

Two of its eleven props (seven of them optional) — `emptyLabel` and `disabled` — are **never passed
at any call site**.

What it lacks against the APG combobox pattern: `Alt+ArrowDown`/`Alt+ArrowUp`, `PageUp`/`PageDown`,
and cycling among same-initial typeahead matches. All three are small additions to an existing
keyboard handler, not architecture.

What it has that a port would have to rebuild is §4.

## 4. What base-ui costs beyond the dependency

### 4.1 The `busy` contract, which base-ui cannot express

This is the decisive one. `Select`'s `busy` prop renders `aria-disabled` + `aria-busy` and **keeps
the control focusable**, because the native `disabled` attribute triggers the HTML focus-fixup rule
and dumps focus to `<body>`. `docs/DESIGN_SYSTEM.md` §6 makes this a rule, and singles out the
modal case as the worst: a keyboard user inside `ConsentNudge` would be stranded in a dialog whose
Escape and Tab handling lives on an ancestor that focus has just left.

base-ui's trigger takes native `disabled` (§2.2). A port would keep the trigger permanently enabled
and re-implement the swallow-your-own-activation logic on top of the library — which is most of what
the hand-rolled trigger already does, now with a library underneath it that disagrees.

This contract is documented in four places and is what five of the six tests in `Select.test.tsx`
exercise. It is not incidental.

### 4.2 Focus model vs. a written design rule

`docs/DESIGN_SYSTEM.md` §6: *"Items in an `aria-activedescendant` listbox are deliberately not
tabbable; focus stays on the controlling input."* base-ui's `Select` moves real focus (§2.2). This
is a genuine divergence from a rule the repo wrote down, not a stylistic preference.

### 4.3 Defaults that must be switched off

`modal = true` (scroll lock plus outside-pointer suppression) is not what a settings dropdown or an
Inbox row picker should do. `Select.Portal` moves the list out of the component's own `rootRef`,
which is what `useOutsidePointerDown` tests containment against.

### 4.4 The Tailwind state idiom collides with the token guard

The idiomatic way to style a headless library is `data-[state=open]:…` /
`group-data-[side=bottom]:…`. The `no-restricted-syntax` block in `eslint.config.js` bans any
`-[…]` in a `className` (`ARBITRARY_VALUE`), so every one of those fails the lint gate. The repo has
zero such usages today.

This is **mitigable, not blocking** — route state styling through co-located CSS
`[data-state="open"]` selectors, which is the house pattern anyway. But note what the mitigation
means: you write the library's CSS the same way `Select.css` is already written.

### 4.5 What a port would actually delete

Honestly stated: roughly 200 lines of keyboard handling, typeahead and activedescendant bookkeeping.
Against that it adds a dependency and five transitive ones, a wrapper re-implementing `busy`, the
variant styling (unchanged — that is all `Select.css`, which stays), four call-site rewrites, and
the divergences above. **The ledger does not clear.**

## 5. The counter-prototype: CSS anchor positioning

The three things the hand-rolled control genuinely lacks are **collision flipping**, **escaping a
clipping ancestor**, and an **origin-aware entrance**. They are visible in the repo today as
workarounds: `Select.css` justifies hardcoded right-alignment as a substitute for collision
handling; `InboxView.css` clips the row *slot* only while a row is collapsing, with a comment saying
it is timed that way so the picker's menu is not cut off; and `createPortal` appears nowhere in
`src/`.

Kodabi ships against **one evergreen Chromium** (WebView2 on Windows). That makes CSS anchor
positioning available in a way it is not on the open web — and it delivers all three, with no
dependency, no portal, and no new effect hook.

### 5.1 Platform floor

| Feature | Chromium | Available |
|---|---|---|
| `@starting-style` | 117 | ✅ |
| `anchor-name`, `position-anchor`, `position-area`, `position-try-fallbacks`, `position-visibility` | 125 | ✅ |
| `anchor-scope` | 131 | ✅ |

Measured **inside the running app's own WebView2**, not from the registry: CDP `/json/version`
reports `Edg/150.0.4078.105` (V8 15.0.23.12); the installed runtime is
`C:\Program Files (x86)\Microsoft\EdgeWebView\Application\150.0.4078.105`. `CSS.supports` returns
`true` for all six properties, and an `insertRule` probe confirms `@starting-style`.

**No WebView2 floor is declared** in `src-tauri/tauri.conf.json` or `src-tauri/Cargo.toml`; Tauri
v2's own documented floor is 125, which is below `anchor-scope`'s 131. Everything is therefore
wrapped in an `@supports` guard, keeping today's behaviour as the base branch — and **that guard has
to test `anchor-scope`**, which is the trap this spike nearly shipped. The prototype as run tested
only `anchor-name` and `position-area`, and those are satisfied in exactly the band this paragraph
names: between 125 and 131 the guard passes, `anchor-scope` is dropped as an unknown declaration,
and every menu on the page collapses onto the last trigger in source order (§5.4) — silently, which
is the worst way to fail. §5.2 records the corrected condition. `anchor-scope` has the highest floor
in the table above, so testing it alone would do; the condition keeps all three rather than resting
on that version ordering. The §5.3 gates are unaffected either way — none of them evaluates an
`@supports` condition.

### 5.2 The prototype

Two files, **+65 / −2 lines**, most of it comment. `Select.tsx` loses `absolute right-0 top-full
mt-2xs` from the list's `className` and gains a `ui-select` class on the wrapper; `Select.css` gains
today's stance as an explicit base branch plus:

```css
@supports (anchor-scope: --x) and (anchor-name: --x) and (position-area: block-end) {
  /* Without anchor-scope every element sharing an anchor-name resolves to the
     LAST one in source order — see §5.4. It carries the highest floor of the
     three (131), so it is what the guard must test; see §5.1. */
  .ui-select          { anchor-scope: --ui-select-trigger; }
  .ui-select__trigger { anchor-name: --ui-select-trigger; }

  .ui-select__list {
    position: fixed;                              /* escapes the clipping ancestor */
    position-anchor: --ui-select-trigger;         /* stays glued to the trigger anyway */
    position-area: block-end span-inline-start;   /* below, right edges aligned */
    position-try-fallbacks: flip-block;           /* collision: flip above */
    position-visibility: anchors-visible;
    inset-inline-end: auto;
    inset-block-start: auto;
    margin-block: var(--space-2xs);               /* symmetric: it can sit above now */

    transform-origin: top right;
    transition:
      opacity var(--dur-plane) var(--ease-standard),
      transform var(--dur-plane) var(--ease-standard);
  }

  @starting-style {
    .ui-select__list { opacity: 0; transform: scale(0.96); }
  }
}
```

**No new token was needed** — `--space-2xs` already carries the offset, and the entrance uses
existing `--dur-plane` / `--ease-standard`. Layer 4 of `design/tokens.css` is untouched.

### 5.3 Gates

Run against the prototype before it was reverted, rather than asserted:

| Gate | Result |
|---|---|
| `pnpm exec eslint . --max-warnings=0` | **pass** (exit 0) |
| `pnpm test` | **pass** — 26 files, 243 tests, including `src/designTokens.test.ts` |
| `tsc -b` + `vite build` | **pass** |

And the non-obvious one: all seven of `anchor-scope`, `anchor-name`, `position-anchor`,
`position-area`, `position-try-fallbacks`, `position-visibility` and `@starting-style` **survive
esbuild's CSS minifier** into `dist/assets/*.css`.

### 5.4 `anchor-scope` — the trap that made this a built prototype, not an argued one

Without `anchor-scope`, every element sharing an `anchor-name` resolves to the **last one in source
order**. `SettingsView` renders two `Select`s on one page; the Inbox renders one per row. Measured
in an app-faithful harness, three right-aligned rows whose triggers sit at x=824, 624 and 724:

| | row 1 menu | row 2 menu | row 3 menu |
|---|---|---|---|
| **with `anchor-scope`** | 824 ✅ | 624 ✅ | 724 ✅ |
| **without** | 724 ❌ | 724 ❌ | 724 |

All three collapse onto the last trigger. Silent, catastrophic, and invisible to a paper spike —
which is why this arm was built and run rather than reasoned about.

### 5.5 Behaviour in the real app

Driven over CDP against the running Tauri window with a populated vault — **9 live `Select`
instances**. Computed styles confirmed the prototype was active (`anchorName: --ui-select-trigger`,
`anchorScope: --ui-select-trigger`, `position: fixed`, `transformOrigin: 236px 0px`, i.e. top-right).

Every fully-visible row, opened in turn:

| Row | Space below trigger | Menu placement | Aligned to own trigger | Hit-testable |
|---|---|---|---|---|
| 1 | 383px | below | ✅ | ✅ |
| 2 | 215px | below | ✅ | ✅ |
| 3 | **46px** | **flipped above** (menu 503–556, trigger 566–594) | ✅ | ✅ |

Row 3 is the whole case: collision flipping worked, on real data, in the real engine, and the menu
stayed inside the viewport. That the menus are hit-testable at their own edges is the second claim —
they escape `main.flex-1`'s `overflow-y: auto` without a portal.

## 6. Caveats, stated rather than glossed

Three things the prototype does **not** do, and one prediction it falsified.

- **`transform-origin` cannot follow a flip.** `@position-try` accepts only inset, margin, sizing,
  self-alignment, `position-anchor` and `position-area` — `transform-origin` is an open CSSWG
  request ([w3c/csswg-drafts#11666](https://github.com/w3c/csswg-drafts/issues/11666)). So a flipped
  menu scales from `top right` when it should scale from `bottom right`. At `--measure-menu` (236px)
  and `scale(0.96)` that is a ~9px origin discrepancy, on the rarer of the two placements. Small,
  real, and worth knowing before #109 lands.

- **Escaping the clipping ancestor is contingent, not free.** `position: fixed` is defeated by any
  ancestor with `transform`, `filter`, `backdrop-filter`, `perspective`, `contain`, `will-change`
  on those, or `container-type`. Demonstrated: in a harness, the same menu inside a
  `backdrop-filter` ancestor still laid out beyond the clipper but was **no longer hit-testable**.

  Audited in the real app: **no ancestor of any Inbox `Select` carries one of the seven**. And
  `.ui-overlay` — which `ConsentNudge`'s `Select` sits inside — computes
  `backdrop-filter: blur(1.5px)` with `overflow: visible`, so it captures `position: fixed` but has
  nothing to clip. Benign today, by computation rather than assumption. It is one CSS property away
  from not being, in a file nobody would think to check, so the grep is the standing guard:

  ```
  transform | filter | backdrop-filter | perspective | contain | will-change | container-type
  ```

- **The anchored menu is ~10px right and ~5px down of today's.** Measured: today's stance gives
  `dxRight −10, dyTop 3`; anchored gives `dxRight 0, dyTop 8`. The cause is real and arguably a fix
  — `absolute` positions against the wrapper, whereas anchor positioning anchors to the *trigger's*
  border box, which for `variant="token"` includes the negative-margin pill bleed. But it **is** a
  visual change, and #109 must accept or compensate for it rather than discover it.

- **Falsified prediction: `position-visibility: anchors-visible` never fires here.** The concern was
  that a `fixed` menu would float over the header once its row scrolled away. It does not: the
  anchored menu tracks its anchor off-screen (trigger at −823, menu at −787). Because `main.flex-1`
  starts at y=0, no case exists in this layout where the anchor is clipped but the menu would still
  paint. The declaration is belt-and-braces, kept for the layouts where it would matter.

## 7. Decision

**Keep the hand-rolled `Select`. Do not add a headless UI dependency.**

The rule already said so; what changed is that it is now evidenced rather than asserted:

1. **The leverage is not there.** Four call sites, two dead props. A library earns its keep by being
   used many times; this would be used four.
2. **The one real gap closes in CSS.** Collision flipping, clipping escape and an origin-aware
   entrance — the three things a popover library is actually bought for — are ~15 declarations that
   pass every gate and were verified working in the real app. The platform lock is what makes this
   available, and it is a genuine advantage of shipping a desktop app that a web app could not take.
3. **The library contradicts two written rules.** The `busy` contract (`DESIGN_SYSTEM.md` §6, and
   the modal case it singles out) and the activedescendant focus model. Adopting means either
   fighting the library or amending the rules.
4. **The cost is +6 runtime dependencies and ~+51 kB gzip** for one control — affordable, but
   nothing is being bought with it.

**No amendment to `.claude/rules/typescript-style.md` is proposed**, because the recommendation is
not to adopt. The rule gains a pointer to this document so it cites evidence.

## 8. Disposition of #109 (`feat/select-menu-origin`)

**Not subsumed. Do it by hand — and widen it.**

#109 asked for an origin-aware entrance using `@starting-style`. §5.2 shows the entrance and the
positioning are the *same fifteen lines in the same file*, and that the entrance needs no
mount-state hook, therefore no new bridge hook, therefore **no amendment to `eslint.config.js` or
`.claude/rules/no-use-effect.md`**. Splitting them into two tickets would mean two branches editing
the same rule block for no gain.

Recommended scope for #109: the entrance **plus** `position-area` / `position-try-fallbacks` /
`anchor-scope` / `@supports`, carrying the three caveats in §6. Ordering is unchanged: **#107
(`@starting-style`) first, then #109.**

### 8.1 One rule #109 must settle first, and this spike deliberately does not

`docs/DESIGN_SYSTEM.md` §4 lists under **Never animates**: *"Overlay entrance, with one sanctioned
exception."* The `Select` menu sits on an overlay plane (§5's table: `Overlay — dropdown`), so on a
plain reading that bullet forbids the entrance #109 asks for.

The reading is not plain, though, and the ambiguity is worth naming precisely:

- Every surface the bullet **enumerates** — the palette, the consent nudge, quick capture, the
  capture pill, `CaptureToast` — is on the **`Overlay — window`** plane (`--lift-palette` /
  `--lift-capture`, `--layer-overlay`). None is a dropdown.
- Its stated **rationale** is about summoned surfaces: *"a hotkey surface that fades in feels slow."*
  A `Select` menu is not summoned; it is opened by a click on a specific trigger and belongs to it.
  #109's own framing is the same distinction from the other side — the palette is opened 100+ times
  a day and should stay instant, whereas a select menu is *occasional*.

So the bullet was written about the window plane and its enumeration was exhaustive of what existed
at the time. **Recommendation: scope that bullet explicitly to the window plane** rather than adding
a second sanctioned exception — the rationale already only supports the narrower rule.

**This spike does not make that edit.** It is a design-system change, it is Shane's call, and it
belongs in the same change as the entrance it licenses. #109 must land it or drop the entrance;
what it must not do is ship an animated dropdown against an unamended §4. Noted here so the
conflict is inherited rather than rediscovered. (`@starting-style` has since landed — #107 added
the mechanism to §4 and converted the Inbox's four entrances to it — but that change is explicitly
*how, not whether*, and left the overlay-entrance bullet untouched. So this is still unadjudicated,
and still #109's to settle.)

While that ticket is open, `Select.css`'s comment justifying right-alignment as a collision
workaround becomes false and should be rewritten — with real flipping, right-alignment reverts to a
free design choice rather than a necessity.

## 9. Does adopting it in one control imply adopting it everywhere?

The ticket asks this directly. **No — and the reason is structural: the app has exactly one anchored
surface, one caret-anchored surface no library solves, and everything else is centred.**

| Surface | Library equivalent | Verdict |
|---|---|---|
| **CommandPalette** (253 lines) | cmdk | **No.** Centred inside `Overlay` at `--palette-top`, so it needs *zero* positioning — the one thing a popover library is uniquely good at. What remains is an activedescendant list, a filter and a keyboard map, which is what it already is. It would also be a second, unrelated dependency. |
| **`Overlay` + 5 dialogs** (90 lines) | base-ui `Dialog` | **No, but the closest call.** Focus trapping and scroll lock are the genuinely fiddly parts. `Overlay` *deliberately* declines trapping because its callers need different strategies, and every dialog is `fixed inset-0`, needing no positioning. If trapping is ever wanted, the zero-dep upgrade is native `<dialog>` + `showModal()`. Flips if a 6th and 7th dialog appear. |
| **NoteEditorView format toolbar** (`src/textareaCaret.ts`, 164 lines) | base-ui `Popover` + virtual anchor | **No — and it is where a library helps *least*.** It anchors to a caret inside a `<textarea>`, which has no DOM node. Floating UI would still need the hidden-div mirror, and **CSS anchor positioning cannot help either** — there is nothing to put `anchor-name` on. The app's only genuinely hard positioning problem is solved by neither answer. |
| **Toasts** (`CaptureToast`, the Inbox filed toast) | Sonner | **No.** Two toasts, no stacking, no swipe-to-dismiss, and `DESIGN_SYSTEM.md` §4 has already adjudicated their motion. |

The corollary: *"we will need it eventually"* has to name **which surface**. Today none is on the
roadmap.

## 10. What this decision changes

- `.claude/rules/typescript-style.md` — the zero-UI-dependency bullet now cites this document.
- `docs/UI_CONVENTIONS.md` — the `Select` entry's "not a headless dependency" sentence links here.
- `docs/FOUNDING_DOC.md` §7 — a born-closed row in Open Decisions.
- **#109** — ruled on and widened (§8).
- **New ticket `test/select-keyboard-coverage`** — see below.

Deliberately untouched:

- `eslint.config.js` and `.claude/rules/no-use-effect.md` — that they need **no** amendment is a
  finding, not an omission: `@starting-style` needs no mount-state hook, so no new bridge hook.
- `docs/DESIGN_SYSTEM.md` §4 — for two separate reasons. #116 owns the refused-motion-candidates
  record, and the overlay-entrance bullet needs a decision this spike is not entitled to make. See
  §8.1: it is #109's to land, and it is flagged rather than silently left.

### The price of saying no

`Select.test.tsx` is 119 lines: five tests on `busy`, one open-and-choose. **Nothing** covers the
arrow keys, `Home`/`End`, `Escape`, typeahead, `aria-activedescendant`, or outside-click; none of the
four overlay bridge hooks has a test at all.

Keeping a hand-rolled combobox is only a safe answer if it is tested. The dependency question is
partly a proxy for that anxiety, and the honest response is coverage, not a library —
`test/select-keyboard-coverage` is filed for it.

## 11. Revisit triggers

Testable conditions, not vibes. Any one of these reopens the question:

1. Phase 4 adds **two or more** of {tooltip, context menu, generic popover, hover card}. The
   library's real value is shared machinery — a dismissal stack, focus scopes, nested-overlay
   ordering — and one control cannot make that argument. Three surfaces can.
2. A second `role="combobox"` with **text filtering** ships. Note the asymmetry in §2.2: base-ui's
   `Combobox`/`Autocomplete` *do* use `aria-activedescendant`, so the §4.2 objection evaporates for
   that component specifically. (The palette's list filter does not count; it is centred and already
   written.)
3. The CommandPalette needs anchored or collision-aware positioning. It is centred today.
4. Any ancestor of a `Select` call site gains one of the seven containing-block properties in §6.
5. A WebView2 floor below 131 is formally declared, or the `@supports` guard is observed false on a
   supported target.
6. `NoteEditorView` moves from `<textarea>` to `contenteditable` — the caret then has a real node to
   anchor to, and `src/textareaCaret.ts` becomes deletable.

## 12. Reproducing

- **Bundle delta** — two Vite projects outside the repo, `react` + `react-dom` versus the same plus
  `@base-ui/react/select`, production mode, esbuild minify; compare reported gzip. Build from a
  short path: the pnpm virtual store under a deep scratchpad exceeds Windows `MAX_PATH` and esbuild
  fails to spawn with a misleading `ENOENT`.
- **base-ui API facts** — read from source, not the docs site:
  `gh api repos/mui/base-ui/contents/packages/react/src/select/…` for `SelectItem.tsx`,
  `SelectTrigger.tsx`, `SelectRoot.tsx`.
- **Feature battery** — with the app running under
  `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9333 --remote-allow-origins=*"`,
  attach to the `http://localhost:1420/` page target and evaluate `CSS.supports` for the six
  properties plus an `insertRule("@starting-style { … }")` probe. `navigator.userAgent` and
  `/json/version` give the WebView2 build; the registry is not the authority the app runs on.
- **Collision and `anchor-scope`** — apply the §5.2 CSS, then open each fully-visible `Select` in
  turn and compare each menu's `getBoundingClientRect().right` against its own trigger's. The
  bottom-most Inbox row is the flip case. Toggling `anchor-scope: none` reproduces §5.4.
- **Containing-block audit** — walk each call site's ancestor chain with `getComputedStyle` for the
  seven properties in §6.
