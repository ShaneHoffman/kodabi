/* eslint-disable react-refresh/only-export-components --
   This file's export is a compound namespace (`Menu.Root`, `Menu.Item`), which
   is how base-ui, and React generally, spell a part-whole component; the rule
   only recognises a bare component export. The cost is that editing this file
   remounts the tree instead of hot-swapping it, which is a dev-loop nicety
   against an API where `Menu.Item` cannot be used outside a `Menu.Root`. */
import { Menu as BaseMenu } from "@base-ui/react/menu";
import { cva, type VariantProps } from "class-variance-authority";
import { clsx } from "clsx";
import type { ComponentPropsWithoutRef, ReactNode } from "react";

/*
 * The anchored menu — the first thing in Kodabi built on @base-ui/react.
 *
 * What base-ui is here for is the part that is genuinely hard and genuinely
 * invisible when it works: anchoring the popup to its trigger and flipping it
 * when the window edge is close, typeahead, roving focus with the arrow keys,
 * Escape, outside-press, focus returning to the trigger, and the ARIA wiring
 * that makes a div behave like a menu. None of that is Grove's opinion, and
 * the hand-rolled version of it in `Select` is 380 lines.
 *
 * What stays ours is the whole appearance: the glass, the radii, the motion.
 * That split is the point of a headless library — see docs/UI_CONVENTIONS.md §4
 * for why the OTHER hand-rolled things (Select, the palette, the toast) are not
 * replaced on sight but each in their own ticket.
 *
 * Two things about the motion are load-bearing:
 *
 *   `origin-(--transform-origin)` reads the CSS variable base-ui writes on the
 *   positioner for the corner the menu actually hangs off, AFTER any flip. A
 *   menu that materializes from its own top-left while hanging off the bottom
 *   right of its trigger reads as a card sliding in from nowhere; matching the
 *   origin to the anchor is what makes it read as growing out of the button.
 *
 *   The exit is an animation, not a transition, and base-ui keeps the element
 *   mounted while `data-ending-style` is set and an animation is running. So
 *   `dissolve` gets its 130ms rather than being cut off by the unmount —
 *   which is exactly the "exits are faster than entrances, always" of
 *   docs/DESIGN_SYSTEM.md §4, at the shorter end of the 110-130ms exit band.
 */

/** The popup surface. Shared by the menu and by anything else that hangs off
 * an anchor with the same material and the same motion. */
const MENU_SURFACE =
  "glass-overlay origin-(--transform-origin) p-1.5 outline-hidden " +
  "animate-materialize motion-reduce:animate-fade-in " +
  "data-ending-style:animate-dissolve motion-reduce:data-ending-style:animate-fade-out";

/** A row inside a menu, in the three appearances a menu row comes in.
 *
 * `data-highlighted` is one attribute for two things — the pointer is over the
 * row, or the keyboard has walked to it — which is precisely the invariant
 * docs/UI_CONVENTIONS.md §4 asks for: hover and focus live on the same
 * element, so a menu never shows two rows lit at once.
 *
 * The two extra appearances are variants rather than classes a call site
 * passes, because font size and text colour are properties this recipe already
 * owns: two utilities for one property are resolved by Tailwind's emission
 * order, not by the order they appear in a `className`, so an override written
 * at the call site is a coin flip that looks like an instruction
 * (docs/UI_CONVENTIONS.md §4). Both of these lost that flip before they were
 * moved here.
 *
 *   suggested — the row the system is proposing, held lit while the menu is
 *               open. It is a claim about content, not a hover state, so it
 *               reads brighter than its siblings even when the highlight is
 *               somewhere else.
 *   foot      — the row that leaves the list to do something else ("New
 *               project…"). Smaller and quieter: it is the way out, not one
 *               more thing to pick.
 *
 * Exported because `Select` — a listbox, not a menu, and hand-rolled — draws
 * the same row on the same surface. It sets `data-highlighted` on its active
 * option itself, so the one attribute keeps carrying "pointer is here OR the
 * keyboard walked here" in both controls. The alternative was a second copy of
 * this recipe drifting from this one.
 */
export const menuRow = cva(
  [
    "flex w-full cursor-default select-none items-center gap-2 rounded-[9px] px-2.5 py-2",
    "font-ui font-medium leading-none outline-hidden",
    "transition-[background-color,color] duration-140 ease-out-strong",
    "data-highlighted:bg-wash data-highlighted:text-ink",
    "data-disabled:text-ink-faint",
  ],
  {
    variants: {
      variant: {
        default: "text-[13px] text-ink-dim",
        suggested: "bg-wash text-[13px] text-ink",
        foot: "text-[11.5px] text-ink-faint",
      },
    },
    defaultVariants: { variant: "default" },
  },
);

type ContentProps = {
  /** Which side of the trigger the menu prefers, before any flip. */
  side?: "top" | "bottom" | "left" | "right";
  /**
   * How the menu lines up along that side. `start` by default — a menu that
   * grows rightward from its trigger's left edge is what a left-hand control
   * implies. Pass `end` for a trigger that sits at the right edge of the thing
   * it acts on, which is what an inbox card's File button is.
   */
  align?: "start" | "center" | "end";
  /** Layout only — the material and the motion belong to the recipe. */
  className?: string;
  children: ReactNode;
};

/** Portal + positioner + popup, so a call site never restates the three. */
function MenuContent({ side = "bottom", align = "start", className, children }: ContentProps) {
  return (
    <BaseMenu.Portal>
      {/* The positioner takes no styling of its own: it exists to be moved by
          the anchoring math, and anything painted on it would move with the
          math rather than with the design. */}
      <BaseMenu.Positioner side={side} align={align} sideOffset={8} className="outline-hidden">
        <BaseMenu.Popup className={clsx(MENU_SURFACE, "min-w-56", className)}>
          {children}
        </BaseMenu.Popup>
      </BaseMenu.Positioner>
    </BaseMenu.Portal>
  );
}

function MenuItem({
  className,
  variant,
  ...rest
}: ComponentPropsWithoutRef<typeof BaseMenu.Item> & VariantProps<typeof menuRow>) {
  return <BaseMenu.Item className={clsx(menuRow({ variant }), className)} {...rest} />;
}

function MenuSeparator({
  className,
  ...rest
}: ComponentPropsWithoutRef<typeof BaseMenu.Separator>) {
  return <BaseMenu.Separator className={clsx("-mx-1.5 my-1.5 h-px bg-edge", className)} {...rest} />;
}

/**
 * `Menu.Trigger` is base-ui's, unstyled on purpose: a trigger composes with
 * whatever control opens the menu via the `render` prop, so the button stays a
 * Grove `Button` and does not grow a second copy of its chrome —
 *
 *   <Menu.Trigger render={<Button variant="quiet">File</Button>} />
 *
 * base-ui merges its props (aria-haspopup, aria-expanded, the handlers) into
 * that element rather than wrapping it, so there is one <button>, not two.
 */
export const Menu = {
  Root: BaseMenu.Root,
  Trigger: BaseMenu.Trigger,
  Content: MenuContent,
  Item: MenuItem,
  Separator: MenuSeparator,
  Group: BaseMenu.Group,
};
