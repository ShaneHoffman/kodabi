import type { Variants } from "motion/react";

/*
 * THE MOTION OF A ROW LEAVING A WORKING LIST.
 *
 * Shared by the two lists that have one: the Inbox, where a routed capture
 * leaves under the app's power, and Commitments, where a checked card leaves
 * under the user's. Doctrine treats them as one move and says so
 * (docs/DESIGN_SYSTEM.md §4): "a row that leaves because the user disposed of
 * it gets the same movement for the same reason." One table, so the two
 * surfaces cannot drift into two dialects of the same departure.
 *
 * The `motion` package drives it rather than CSS, and these lists are why the
 * package is in the stack at all: a row leaving has to collapse a height nobody
 * measured, while its neighbours glide up to meet it, while a new row may be
 * arriving into the same list — three things on one timeline, interruptible
 * halfway through. CSS can express any one of them and none of them together.
 * Everything else in the app stays CSS (docs/UI_CONVENTIONS.md).
 *
 * Two elements per row, and the split is load-bearing: the SLOT (the `li`) owns
 * the space, the CARD inside it owns the travel. Collapsing the space and
 * sliding the card are different clocks on purpose — the card is gone at 220ms
 * while the slot is still closing at 280, so the space hands itself back behind
 * a card that has already left, and the row below never appears to shove it
 * out. They are one movement because they overlap, not because they match.
 *
 * The variants are keyed by label, so the slot and the card can be told
 * "you are leaving" once, on the parent, and each answer in its own values.
 *
 * TWO THINGS THIS DELIBERATELY DOES NOT USE.
 *
 * No `layout` prop. The neighbours glide because the slot's HEIGHT is animating
 * and they are in normal flow behind it, and that needs no projection. `layout`
 * would put every row in a projection tree that writes its own transforms,
 * fighting the card's `x` underneath it: two writers of one property. It earns
 * its keep when items REORDER, and neither list reorders. Rows only arrive and
 * leave.
 *
 * No `exit` variant on the list's children either, and this one is a real
 * constraint rather than a preference. A departure here is always something the
 * app already knows about BEFORE the element unmounts, because it starts the
 * moment the gesture is made rather than whenever the vault refetch happens to
 * land. Animating from that state is both earlier and more honest than
 * animating on the way out. `exit` would also make each row's unmount wait on
 * an animation completing inside a subtree that holds a base-ui menu and two
 * dialogs, and a row that can fail to unmount is a far worse bug than a
 * collapse that gets clipped.
 *
 * Each table is one FROZEN module constant per reduced-motion setting, picked
 * by a getter, rather than an object literal built during render. A variants
 * object is part of a `motion` element's identity: handing it a structurally
 * identical but newly-allocated one on every render is a change as far as the
 * animator is concerned, and it re-reads the whole table each time — on lists
 * that re-render on every vault refetch, every keystroke in a dialog and every
 * menu open.
 */

/** `--ease-out-strong`, as numbers: entrances and arrivals. */
export const EASE_OUT_STRONG = [0.23, 1, 0.32, 1] as const;
/** `--ease-in-out-strong`, as numbers: departures, which leave under power. */
export const EASE_IN_OUT_STRONG = [0.77, 0, 0.175, 1] as const;

/** How long the travel-left-and-vanish plays: the card clears in 220ms inside a
 * slot that closes in 280, so this is the slot's clock — the moment there is
 * nothing left to see. */
export const VANISH_MS = 280;

/** The app's Exit band (110–130ms, docs/DESIGN_SYSTEM.md §4), spent on the
 * reduced-motion fade that stands in for the whole choreography. Shorter than
 * the entrance it undoes, which is the rule for every exit in the app. */
export const EXIT_FADE_S = 0.13;

/** The gap between rows, in px. It lives on each slot as an animated margin
 * rather than on the list as a flex `gap`, because a gap cannot collapse: a
 * row whose height reaches zero with a live gap under it still holds 14px of
 * the list open, and the neighbours land with a jump at the end of a
 * choreography built to avoid exactly that. */
export const ROW_GAP = 14;

/**
 * The slot: the space a row occupies, and the only thing that ever animates a
 * height. `height: "auto"` is motion measuring the row for us — the same
 * problem the `1fr → 0fr` grid trick used to solve, minus the grid.
 *
 * `overflow: "clip"` is set (not animated) at the start of the exit, so the
 * clipping exists only while collapsing. A resting row must never be clipped:
 * it would crop its own hover lift and, worse, its focus ring, in the densest
 * list in the app.
 */
const SLOT_MOVING: Variants = {
  enter: { height: 0, opacity: 0, marginBottom: 0 },
  rest: {
    height: "auto",
    opacity: 1,
    marginBottom: ROW_GAP,
    // Rise-in, the band for a row appearing (docs/DESIGN_SYSTEM.md §4), and
    // the same 280 the collapse below runs on.
    transition: { duration: 0.28, ease: EASE_OUT_STRONG },
  },
  gone: {
    height: 0,
    marginBottom: 0,
    overflow: "clip",
    transition: { duration: 0.28, ease: EASE_IN_OUT_STRONG },
  },
};

/** Fades only. No height: the collapse IS the movement, and the rule is that
 * movement goes while life stays (docs/DESIGN_SYSTEM.md §4).
 *
 * The margin is NOT movement, though, and it is not optional either: it is the
 * list's 14px gap, which lives here rather than on the `ul` so it can collapse
 * with the row that owns it. Every state carries the same value, so it is set
 * once and never animated — leave it out of one of them and the rows go flush
 * the moment the preference is on. */
const SLOT_STILL: Variants = {
  enter: { opacity: 0, marginBottom: ROW_GAP },
  rest: { opacity: 1, marginBottom: ROW_GAP, transition: { duration: 0.2 } },
  gone: {
    opacity: 0,
    marginBottom: ROW_GAP,
    transition: { duration: EXIT_FADE_S },
  },
};

export function slotVariants(reduce: boolean): Variants {
  return reduce ? SLOT_STILL : SLOT_MOVING;
}

/**
 * The card: what the eye actually follows. Out to the LEFT, because that is
 * where these lists came from — a row leaves along the axis it arrived on
 * rather than picking a new direction on the way out — carrying a 2px blur,
 * which is the part that makes it read as departing rather than as being
 * deleted.
 */
/* The blur appears ONLY in `gone`, never in the resting states, and that is a
   correctness point rather than a tidiness one: a `filter` in the resting
   variant would sit in the inline style of every card in the list forever, and
   a filtered element is a containing block for fixed-position descendants and
   its own stacking context. Each row holds a menu and dialogs that position
   against the viewport, so a resting `blur(0px)` would quietly re-anchor them
   to the card. Motion reads the computed value as the start of the exit, which
   is what it should be animating from anyway. */
const CARD_MOVING: Variants = {
  enter: { opacity: 0, x: 0 },
  rest: { opacity: 1, x: 0 },
  gone: {
    opacity: 0,
    x: -24,
    filter: "blur(2px)",
    transition: { duration: 0.22, ease: EASE_IN_OUT_STRONG },
  },
};
const CARD_STILL: Variants = {
  enter: { opacity: 0 },
  rest: { opacity: 1 },
  gone: { opacity: 0, transition: { duration: EXIT_FADE_S } },
};

export function cardVariants(reduce: boolean): Variants {
  return reduce ? CARD_STILL : CARD_MOVING;
}
