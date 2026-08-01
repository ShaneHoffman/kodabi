import { useRef, type ReactNode } from "react";
import { Button } from "./Button";
import { Dialog } from "./Dialog";
import { StatusMessage } from "./StatusMessage";

type Props = {
  /** The heading, which is also the dialog's accessible name. */
  title: string;
  /** Label on the destructive confirm control (e.g. "Delete project"). */
  confirmLabel: string;
  /** What the confirm control reads while its action runs (e.g. "Deleting…"). */
  busyLabel: ReactNode;
  /** Whether the confirmed action is in flight: both controls go busy. */
  busy: boolean;
  /** An error to show in the dialog, or null. */
  error: string | null;
  /** Runs the destructive action. */
  onConfirm: () => void;
  /** Dismiss without acting: Cancel, Escape, or a scrim press. */
  onClose: () => void;
  /** The body: what confirming will cost. */
  children: ReactNode;
};

/**
 * The shared shape of a destructive confirmation: the action is marked by
 * CONFIRMATION first (docs/DESIGN_SYSTEM.md §2), and Cancel holds initial focus
 * so the keyboard's first Enter dismisses rather than destroys.
 *
 * Grove adds the coral `danger` box on the confirming control. That is not a
 * softening of the confirmation-not-colour rule — it is where the rule says the
 * one red in the app is allowed to live: on the confirm INSIDE a confirmation,
 * never on the button that opens one.
 *
 * The shell is the Grove `Dialog`, so the focus trap, Escape, the scrim press,
 * the scroll lock and the focus restore are base-ui's rather than three
 * hand-rolled copies of a Tab wrapper. Being mounted IS being open: every
 * caller renders this conditionally, and unmounting is how it closes.
 *
 * It is deliberately presentational: each caller keeps its own async handler,
 * `busy`/`error` state, success behaviour, and error copy, and passes the
 * results down. On confirm this calls `onConfirm`; it never closes itself, so
 * the caller decides what success means (navigate away, refetch, unmount).
 */
export function DestructiveConfirmDialog({
  title,
  confirmLabel,
  busyLabel,
  busy,
  error,
  onConfirm,
  onClose,
  children,
}: Props) {
  // Cancel is the default action of a confirmation, so it takes focus on open
  // rather than base-ui's default of the first tabbable element — which here is
  // the destructive one.
  const cancelRef = useRef<HTMLButtonElement>(null);

  return (
    <Dialog
      open
      onDismiss={onClose}
      label={title}
      initialFocus={cancelRef}
      className="flex flex-col gap-4"
    >
      <h2 className="text-[15px] font-semibold text-ink">{title}</h2>

      <div className="flex flex-col gap-2.5 text-[13px] leading-relaxed text-ink-read">
        {children}
      </div>

      {error && (
        <StatusMessage variant="error" compact>
          {error}
        </StatusMessage>
      )}

      <div className="flex items-center justify-end gap-2.5">
        <Button variant="danger" onClick={onConfirm} loading={busy} loadingLabel={busyLabel}>
          {confirmLabel}
        </Button>
        <Button ref={cancelRef} onClick={onClose} loading={busy}>
          Cancel
        </Button>
      </div>
    </Dialog>
  );
}
