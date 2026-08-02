import {
  useRef,
  type KeyboardEventHandler,
  type ReactNode,
  type Ref,
} from "react";
// eslint-disable-next-line no-restricted-syntax -- pre-Grove; the primitives' Grove ticket deletes it
import "./Overlay.css";

type Props = {
  /** Called when the backdrop is clicked. Escape is the caller's to handle,
   * since what Escape means depends on what is open inside the panel. */
  onDismiss: () => void;
  children: ReactNode;
  /** Accessible name. Pass one of these, not both. */
  label?: string;
  labelledBy?: string;
  /** Extra classes for the panel (its layout; the shell is fixed). */
  className?: string;
  onKeyDown?: KeyboardEventHandler<HTMLDivElement>;
  panelRef?: Ref<HTMLDivElement>;
};

/**
 * The modal shell: scrim, stacking layer, backdrop dismissal, and the raised
 * panel (docs/DESIGN_SYSTEM.md §5).
 *
 * This existed twice, byte for byte, in CommandPalette and ConsentNudge —
 * including both of the non-obvious guards below, which is exactly the kind of
 * subtlety that survives one copy and rots in the other.
 *
 * Focus trapping is deliberately NOT here — each caller held focus its own
 * way and restored it through `useDialogFocus`. This shell is now callerless:
 * the dialogs that wrapped Tab across several controls moved to the Grove
 * `Dialog`, where base-ui owns the trap, and the palette (which held focus on
 * its lone input by swallowing Tab) never used this shell at all — it
 * composes base-ui's dialog parts directly. Left on disk pending its own
 * removal ticket.
 */
export function Overlay({
  onDismiss,
  children,
  label,
  labelledBy,
  className = "",
  onKeyDown,
  panelRef,
}: Props) {
  // Whether the current pointer gesture started on the backdrop itself. A press
  // that begins inside the panel must never dismiss, even if it ends (or
  // retargets, via common-ancestor click) on the backdrop.
  const backdropPressed = useRef(false);

  return (
    // Dismiss fires on click, not pointerdown: unmounting the overlay at
    // pointerdown lets the rest of the gesture fall through to whatever sat
    // underneath (the unmount flushes before mousedown), so a dismissing click
    // would also press the controls behind it. The opening gesture predates the
    // overlay, so it can never self-close.
    <div
      className="ui-overlay fixed inset-0 flex items-start justify-center px-md"
      onPointerDown={(event) => {
        backdropPressed.current = event.target === event.currentTarget;
      }}
      onClick={(event) => {
        if (backdropPressed.current && event.target === event.currentTarget) {
          onDismiss();
        }
      }}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-label={label}
        aria-labelledby={labelledBy}
        onKeyDown={onKeyDown}
        // A fallback focus target, and the reason `onKeyDown` above is worth
        // having on the panel at all. Clicking the panel's own padding blurs
        // whatever held focus, and with nothing focusable underneath it the
        // active element becomes <body> — outside the dialog, where a keydown
        // no longer bubbles through here and Escape silently stops closing the
        // modal. -1 keeps it out of the tab order while still letting it take
        // focus, so the dialog always has somewhere to hold it.
        tabIndex={-1}
        className={`ui-overlay__panel w-full${
          className ? ` ${className}` : ""
        }`}
      >
        {children}
      </div>
    </div>
  );
}
