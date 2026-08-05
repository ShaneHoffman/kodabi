import { Dialog as BaseDialog } from "@base-ui/react/dialog";
import { clsx } from "clsx";
import type { ReactNode, RefObject } from "react";

type Props = {
  /** Whether the dialog is on screen. The caller owns this. */
  open: boolean;
  /** Dismissed without acting: Escape, or a press on the scrim. */
  onDismiss: () => void;
  /** Accessible name. Pass one of these, not both. */
  label?: string;
  labelledBy?: string;
  /**
   * What takes focus on open. Omit for base-ui's default (the first tabbable
   * thing inside), and pass the safe action's ref where the dialog is
   * destructive — DESIGN_SYSTEM §6: a dialog takes focus on open, to the first
   * field, or to the safe action where confirming would destroy something.
   */
  initialFocus?: RefObject<HTMLElement | null>;
  /** The panel's layout — its width, its spacing. The glass shell is fixed. */
  className?: string;
  children: ReactNode;
};

/**
 * The modal dialog: scrim, glass panel, focus trap, and the center-origin
 * materialize (docs/DESIGN_SYSTEM.md §4-5).
 *
 * It sits on `@base-ui/react`, which owns the behaviour that is tedious to get
 * right and invisible when it works: focus is trapped while the dialog is open
 * and returned to whatever opened it on close, Escape and an outside press
 * dismiss, the page behind stops scrolling, and the popup carries `role`,
 * `aria-modal` and its labelling. The pre-Grove `Overlay` deliberately did NOT
 * trap focus, and each of its callers hand-rolled a Tab strategy — that is the
 * duplication this replaced. The consent nudge and the create-project dialog
 * each moved here in their own screen ticket, and the palette composes
 * base-ui's dialog parts directly, because it is top-anchored and holds cmdk's
 * list instead of this one's centred panel. `Overlay` went with the rest of the
 * pre-Grove layer once the last of them had moved.
 *
 * Two details are load-bearing and easy to undo by accident:
 *
 *   Centering is `inset-0 m-auto h-fit`, NOT the usual
 *   `top-1/2 left-1/2 -translate-1/2`. The `materialize` keyframe animates
 *   `transform`, so a translate-based centering is overwritten by the
 *   animation's `scale(0.96)` and the dialog opens in the corner. Margin
 *   centering leaves `transform` free for the motion.
 *
 *   The scrim only fades, and so needs no reduced-motion partner: it is
 *   opacity from zero, which is what the reduced-motion swap turns everything
 *   else INTO. Its lightness is deliberate too — a dialog's glass can only
 *   read as glass if the app stays visible enough beneath it to blur.
 */
export function Dialog({
  open,
  onDismiss,
  label,
  labelledBy,
  initialFocus,
  className,
  children,
}: Props) {
  return (
    <BaseDialog.Root
      open={open}
      onOpenChange={(next) => {
        // One direction only. The dialog never opens itself, and a caller that
        // owns `open` owns what closing means (navigate away, refetch, unmount).
        if (!next) onDismiss();
      }}
    >
      <BaseDialog.Portal>
        {/* The scrim carries a testid because it is the one part of a dialog
            with no role and no accessible name to find it by — it is a veil,
            not a control — and "a press outside dismisses" is worth a test. */}
        <BaseDialog.Backdrop
          data-testid="dialog-scrim"
          className="glass-scrim fixed inset-0 animate-fade-in data-ending-style:animate-fade-out"
        />
        <BaseDialog.Popup
          initialFocus={initialFocus}
          aria-label={label}
          aria-labelledby={labelledBy}
          className={clsx(
            "glass-dialog fixed inset-0 m-auto h-fit w-full max-w-[min(28rem,calc(100vw-2rem))]",
            "origin-center p-5 font-ui outline-hidden",
            "animate-materialize motion-reduce:animate-fade-in",
            "data-ending-style:animate-dissolve motion-reduce:data-ending-style:animate-fade-out",
            className,
          )}
        >
          {children}
        </BaseDialog.Popup>
      </BaseDialog.Portal>
    </BaseDialog.Root>
  );
}
