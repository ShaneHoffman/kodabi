/**
 * The motion a row in a working list spends on arriving and on being cleared.
 *
 * Kept here rather than in the one view that draws them because two lists play
 * the same transitions — the Inbox's cards and the capture surfaces' rows — and
 * a list that agreed with its neighbour only by coincidence is a list that will
 * stop agreeing. These are class strings, not components: the element, its
 * material and its layout stay the caller's, and only the timing is shared.
 *
 * The exit is a collapse, not a disappearance. A row that vanishes leaves the
 * rows below it jumping up through the space it held; a row that slides out
 * while its slot closes hands that space back at a speed the eye can follow, so
 * the row you were about to click is still where you were about to click it.
 * The two halves are on the same clock and the same curve, which is what makes
 * them read as one movement rather than two effects.
 *
 * Every transition here is a NAMED property list. `transition-all` would sweep
 * up whatever a caller adds later — a colour, a shadow, the hover lift — and
 * put it on the exit's clock (docs/DESIGN_SYSTEM.md §4).
 *
 * ONE of these recipes goes on an element at a time, never two, and never
 * alongside a transition the caller wrote itself. `transition-[…]` and
 * `duration-*` are each a single CSS property, so a second utility for either
 * is decided by Tailwind's emission order rather than by the className: the
 * longest property list wins, and the longest duration wins. Stacking the
 * entrance and the exit "so both are ready" therefore gives BOTH legs the
 * entrance's clock and silently retires the exit band. Pick the recipe from
 * the state (a ternary, not an override) — a transition is read from the
 * after-change style, so applying it in the same commit as the leaving values
 * still animates.
 */

/**
 * The outer slot: a one-row grid whose track collapses to nothing. This is the
 * only way to animate a height that isn't known in advance — a card's height
 * depends on its snippet — and `1fr → 0fr` is the trick that gets it without
 * measuring anything in JavaScript.
 *
 * The slot's child must be the `LIST_SLOT_INNER` wrapper, which is where the
 * clipping lives.
 */
export const LIST_SLOT =
  "grid grid-rows-[1fr] transition-[grid-template-rows] duration-200 ease-out-strong " +
  "motion-reduce:transition-none";

/** The slot, collapsing. Pair with `LIST_LEAVING` on the row inside it. */
export const LIST_SLOT_LEAVING = "grid-rows-[0fr]";

/**
 * The clipping wrapper between the slot and the row.
 *
 * `overflow-hidden` only while collapsing, never at rest: a resting row with a
 * clipped box would crop its own hover lift and its focus ring, and a focus
 * ring that disappears where the list is densest is the one place it has to be
 * visible. `min-h-0` because a grid item's automatic minimum size is its
 * content, which would hold the track open against the `0fr`.
 */
export const LIST_SLOT_INNER = "min-h-0";
export const LIST_SLOT_INNER_LEAVING = "overflow-hidden";

/**
 * The row's own exit: out to the left and gone. Left because that is where the
 * list came from — the row leaves along the axis it arrived on rather than
 * picking a new direction on the way out.
 *
 * Shorter than the collapse (the exit band, DESIGN_SYSTEM §4), so the row has
 * cleared before the space it held finishes closing; the reverse order reads
 * as the list shoving it out.
 */
export const LIST_ROW_EXIT =
  "transition-[translate,opacity] duration-130 ease-out-strong motion-reduce:transition-none";
export const LIST_ROW_LEAVING = "-translate-x-9 opacity-0";

/**
 * A row arriving: it fades up into place from just above where it lands.
 *
 * `starting:` is Tailwind v4's `@starting-style` — the browser's own answer to
 * "what does this element transition FROM the first time it is painted", which
 * is what an entrance needs and what a plain transition cannot express. No
 * keyframes, no mount-time state, and nothing to clean up.
 */
export const LIST_ROW_ENTER =
  "transition-[translate,opacity] duration-220 ease-out-strong " +
  "starting:-translate-y-4 starting:opacity-0 motion-reduce:transition-none";
