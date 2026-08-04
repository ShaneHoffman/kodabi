import { cva, type VariantProps } from "class-variance-authority";
import { clsx } from "clsx";
import type { ComponentPropsWithRef, ReactNode } from "react";

/**
 * The five shapes a Grove button comes in.
 *
 * Two of them are the same box. `action`, `danger` and `quiet` are all a
 * rectangle at `rounded-button` with 8x16 padding, because they sit next to
 * each other in an action rail and a rail whose buttons disagree about their
 * own height is the thing this padding exists to prevent — `quiet` carries a
 * transparent border for the same reason, so it occupies the same layout box
 * as the bordered two rather than jumping a pixel when it gains one
 * (docs/DESIGN_SYSTEM.md §2: a state change never changes the layout box).
 *
 *   action — the rectangle. Glass at the smallest size the material comes in:
 *            a translucent fill, an edge, and the lit top edge every Grove
 *            surface carries. This is what a verb looks like.
 *   danger  — the same rectangle, tinted coral, for the confirm control inside
 *            a confirmation dialog. Never the button that OPENS one: the thing
 *            that makes an action destructive is the confirmation, and a red
 *            button in a list is a warning the user cannot act on.
 *   quiet   — bare text that gains a wash on hover. The second action in a
 *            pair (Dismiss beside Retry, Cancel beside Delete).
 *   pill    — the token shape. A pill does not perform a verb, it STANDS for
 *            something you can open: an artifact chip, a citation. If the
 *            label is a verb it is a rectangle (DESIGN_SYSTEM §2).
 *   chip    — the pill's material at a softer radius, for a token that has to
 *            fill the width it is given: the note editor's SESSION panel, where
 *            `Audio` and `Transcript` stack in a 272px rail and a pill's ends
 *            would leave two ragged gutters. It stands for something you open,
 *            exactly like `pill`; only the box changed to fit the rail.
 *            A label/value split goes in a `flex w-full justify-between` span
 *            INSIDE the button — not via className, which cannot beat the base
 *            `justify-center` (no tailwind-merge here: the cascade decides, and
 *            both utilities are the same property in the same layer).
 *
 * Colours are tokens rather than values because the alphas invert between
 * grounds — night lightens with white, day darkens with ink — and `.day`
 * remaps them without this table knowing (src/index.css §2).
 */
const buttonVariants = cva(
  [
    "inline-flex select-none items-center justify-center gap-2",
    "font-ui text-[12px] leading-none",
    "border",
    // Never `transition: all`. The properties are named so a press animates
    // the press and nothing accidental rides along — a `filter` or a
    // `box-shadow` caught in an `all` is what makes a cheap button feel
    // gluey. One duration for every leg: the press spec is 140ms on
    // ease-out-strong, and the colour steps ride the same clock so a hover
    // that turns into a press reads as one gesture (DESIGN_SYSTEM §4).
    //
    // `scale`, not `transform`: Tailwind v4's `scale-*` sets the standalone
    // `scale` property rather than a transform function, so a transition that
    // names `transform` animates nothing and the press lands as a snap. Worth
    // checking in the built CSS rather than by eye — the failure is silent.
    "transition-[scale,background-color,border-color,color] duration-140 ease-out-strong",
    // The press: the one state allowed to move its own box, and it moves
    // nothing else on the page (`transform` is compositor-only, so the layout
    // box is untouched). Guarded like every other interactive state, because
    // a busy button carries aria-disabled rather than the native attribute
    // and must be as inert as a genuinely disabled one.
    "not-disabled:not-aria-disabled:active:scale-97",
    // The reduced-motion swap REPEATS both guards, and it has to. `:not()`
    // takes its argument's specificity, so the guarded press above weighs
    // (0,4,0); a bare `motion-reduce:active:scale-100` weighs (0,2,0) and
    // loses on specificity no matter what order Tailwind emits it in — the
    // swap would be in the markup and dead in the cascade. Matched guards make
    // the two equal, and the redefined `motion-reduce` variant sorts after, so
    // stillness wins. The other silent-CSS failure this file warns about.
    "motion-reduce:not-disabled:not-aria-disabled:active:scale-100",
    "focus-ring",
    "disabled:cursor-not-allowed aria-disabled:cursor-not-allowed",
    // The disabled LOOK is one step down the ink ladder for every variant.
    // It used to be per-variant, because `filled` was an ink fill with a
    // page-coloured label and fading THAT label produced an unreadable
    // control; Grove has no inverting variant, so the special case is gone.
    "disabled:text-ink-faint aria-disabled:text-ink-faint",
  ],
  {
    variants: {
      variant: {
        action: [
          "rounded-button px-4 py-2 font-semibold text-ink",
          "border-action-edge bg-action-bg shadow-[inset_0_1px_0_var(--color-edge-lit)]",
          "not-disabled:not-aria-disabled:hover:bg-action-bg-hover",
        ],
        // The lit edge is action's, not a coral one: the tint says what the
        // button does, and the highlight says it is the same physical chip.
        danger: [
          "rounded-button px-4 py-2 font-semibold text-danger",
          "border-danger-edge bg-danger-bg shadow-[inset_0_1px_0_var(--color-edge-lit)]",
          "not-disabled:not-aria-disabled:hover:bg-danger-bg-hover",
        ],
        quiet: [
          "rounded-button border-transparent px-4 py-2 font-medium text-ink-faint",
          "not-disabled:not-aria-disabled:hover:bg-wash",
          "not-disabled:not-aria-disabled:hover:text-ink",
        ],
        pill: [
          "rounded-pill px-3.5 py-2 font-semibold text-ink-dim",
          "border-edge bg-wash shadow-[inset_0_1px_0_var(--color-edge-lit)]",
          "not-disabled:not-aria-disabled:hover:bg-wash-hover",
          "not-disabled:not-aria-disabled:hover:text-ink",
        ],
        chip: [
          "rounded-[10px] px-3 py-2 font-semibold text-ink-dim",
          "border-edge bg-wash shadow-[inset_0_1px_0_var(--color-edge-lit)]",
          "not-disabled:not-aria-disabled:hover:bg-wash-hover",
          "not-disabled:not-aria-disabled:hover:text-ink",
        ],
      },
    },
    defaultVariants: { variant: "action" },
  },
);

type Props = ComponentPropsWithRef<"button"> &
  VariantProps<typeof buttonVariants> & {
    /** Whether the action this button started is still running. */
    loading?: boolean;
    /** What to read while `loading`; falls back to the button's own children. */
    loadingLabel?: ReactNode;
  };

/**
 * The one action control — the single home for the focus ring and every
 * interaction state, so screens never restate them (docs/DESIGN_SYSTEM.md §2).
 *
 * It owns its whole box, unlike the pre-Grove version whose `quiet` deferred
 * padding to each consumer's stylesheet: the rails those buttons sit in only
 * align if the buttons agree, and "whatever shape its context needs" is how
 * they stopped agreeing. A caller may still pass layout classes (`w-full`,
 * `justify-between`), but the geometry is the component's.
 *
 * `loading` exists so a pending action never unmounts its own control. Swapping
 * a focused button for a `<span>Saving…</span>` drops focus to <body> and strips
 * the user's place in the page.
 *
 * Keeping the button mounted is only half of that: the native `disabled`
 * attribute drops focus too. An element that is focused when it becomes
 * disabled is blurred and focus resets to <body> (the HTML focus fixup rule),
 * which is the very thing `loading` exists to prevent — and it takes the
 * surrounding keyboard context with it, since a dialog's Escape/Tab handling
 * listens on an ancestor the focus has just left. So a busy button is marked
 * `aria-disabled` instead: inert to a screen reader, still in the tab order,
 * with its activation swallowed here. `disabled` stays a genuine disable, for
 * a control that has nothing to do rather than something in flight.
 */
export function Button({
  variant,
  type = "button",
  className,
  loading = false,
  loadingLabel,
  disabled,
  children,
  onClick,
  ...rest
}: Props) {
  // Busy, not disabled: an explicit `disabled` wins, since a caller asking for
  // a genuinely inert control means it.
  const busy = loading && !disabled;
  return (
    <button
      type={type}
      className={clsx(buttonVariants({ variant }), className)}
      disabled={disabled}
      aria-disabled={busy || undefined}
      // aria-busy says *why* it went inert; aria-disabled says that it did.
      aria-busy={loading || undefined}
      // preventDefault, not just "skip the handler": a submit button's click is
      // what submits the form, and Enter inside a field reaches the form
      // through a synthesized click on this very button. Cancelling the click
      // stops both, matching what `disabled` used to do.
      onClick={
        busy
          ? (event) => {
              event.preventDefault();
              event.stopPropagation();
            }
          : onClick
      }
      {...rest}
    >
      {loading ? (loadingLabel ?? children) : children}
    </button>
  );
}
