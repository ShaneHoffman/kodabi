import { useRef, type ReactNode } from "react";
import { clsx } from "clsx";
import { Button } from "./Button";
import { Dialog } from "./Dialog";
import { StatusMessage } from "./StatusMessage";

type Props = {
  /** The heading question ("Delete this note?"), which is also the dialog's
   * accessible name. */
  title: string;
  /** The thing being acted on, named in its own strip under the title. Omit
   * only where the surface the dialog opened from IS the thing. */
  subject?: string;
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
  /** The consequence: one short line on what confirming will cost. The
   * "cannot be undone" warning is not this — the dialog owns that. */
  children: ReactNode;
};

/**
 * The shared shape of a destructive confirmation: the action is marked by
 * CONFIRMATION first (docs/DESIGN_SYSTEM.md §2), and Cancel holds initial focus
 * so the keyboard's first Enter dismisses rather than destroys.
 *
 * The copy is a fixed structure rather than free prose, and each part answers a
 * different question at a glance: the title asks it, the SUBJECT strip says
 * which thing (its own inset surface, not a name buried in a sentence — and it
 * truncates, so a long note title shortens instead of reflowing the dialog),
 * the body says what it costs, and the warning that it is permanent is its own
 * line in the danger tint. That last line lives here rather than in each
 * caller: a destructive confirmation is destructive by definition, and the one
 * dialog that forgot to say so would be the one that needed it.
 *
 * Grove adds the coral `danger` box on the confirming control. That is not a
 * softening of the confirmation-not-colour rule — it is where the rule says the
 * one red in the app is allowed to live: on the confirm INSIDE a confirmation,
 * never on the button that opens one.
 *
 * The rail reads Cancel then confirm, left to right, so the destructive control
 * sits at the end of the line and nothing lands under the pointer on the way to
 * it. Cancel is `quiet` for the same reason it holds focus: leaving is the
 * default, and the default does not need a box.
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
  subject,
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

      {subject && (
        // `title` so the full name is still reachable once the strip has
        // truncated it.
        <p
          title={subject}
          className={clsx(
            "overflow-hidden text-ellipsis whitespace-nowrap",
            "rounded-button border border-edge bg-wash px-3.5 py-2.5",
            "shadow-[inset_0_1px_0_var(--color-edge-lit)]",
            "text-[13.5px] font-semibold text-ink",
          )}
        >
          {subject}
        </p>
      )}

      <div className="flex flex-col gap-2.5 text-[13px] leading-relaxed text-ink-dim">
        {children}
      </div>

      <p className="text-[13px] leading-relaxed text-danger">This cannot be undone.</p>

      {error && (
        <StatusMessage variant="error" compact>
          {error}
        </StatusMessage>
      )}

      <div className="flex items-center justify-end gap-2.5">
        <Button ref={cancelRef} variant="quiet" onClick={onClose} loading={busy}>
          Cancel
        </Button>
        <Button variant="danger" onClick={onConfirm} loading={busy} loadingLabel={busyLabel}>
          {confirmLabel}
        </Button>
      </div>
    </Dialog>
  );
}
