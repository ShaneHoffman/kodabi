import {
  useRef,
  type KeyboardEventHandler,
  type ReactNode,
  type Ref,
} from "react";
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
 * Focus trapping is deliberately NOT here. The palette holds focus on its lone
 * input by swallowing Tab; the consent nudge wraps Tab across several controls.
 * Those are genuinely different strategies, so each dialog keeps its own via
 * `onKeyDown`, and both restore focus through `useDialogFocus`.
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
        className={`ui-overlay__panel w-full${
          className ? ` ${className}` : ""
        }`}
      >
        {children}
      </div>
    </div>
  );
}
