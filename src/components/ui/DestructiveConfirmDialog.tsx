import {
  useId,
  useRef,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react";
import { useDialogFocus } from "../../useDialogFocus";
import { wrapDialogTab } from "../../dialogTabTrap";
import { Button } from "./Button";
import { Overlay } from "./Overlay";
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
  /** Dismiss without acting: Cancel, Escape, or a backdrop press. */
  onClose: () => void;
  /** The body: what confirming will cost. */
  children: ReactNode;
};

/**
 * The shared shape of a destructive confirmation, exactly as
 * docs/DESIGN_SYSTEM.md §2 prescribes: the action is marked by CONFIRMATION,
 * not colour. Cancel is the primary control and holds initial focus; the
 * confirming button is the non-default `destructive` variant beside it, so the
 * keyboard's first Enter dismisses rather than destroys.
 *
 * This is the boilerplate every such dialog shared byte for byte — the Overlay,
 * the focus hand-off, the Escape/Tab wiring, the Cancel-primary/destructive
 * footer, the error slot — factored out so `DeleteProjectDialog` and the
 * Needs Attention capture delete are one thing, not two hand-rolled copies.
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
  const titleId = useId();
  const cancelId = useId();

  const panelRef = useRef<HTMLDivElement>(null);

  // Cancel is the default action of a confirmation, so it takes focus on open;
  // focus is restored on close.
  useDialogFocus(() => document.getElementById(cancelId));

  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
      return;
    }
    if (event.key !== "Tab") return;
    wrapDialogTab(event, panelRef.current);
  };

  return (
    <Overlay
      onDismiss={onClose}
      labelledBy={titleId}
      panelRef={panelRef}
      onKeyDown={onKeyDown}
      className="flex flex-col gap-md p-md"
    >
      <h2 id={titleId} className="font-serif text-title-panel text-text">
        {title}
      </h2>

      <div className="flex flex-col gap-sm text-body text-text-soft">{children}</div>

      {error && (
        <StatusMessage variant="error" compact>
          {error}
        </StatusMessage>
      )}

      <div className="flex items-center justify-end gap-sm">
        {/* The confirming control is the non-default one: destructive wears
            the quiet ghost's chrome, and the primary beside it is Cancel. */}
        <Button variant="destructive" onClick={onConfirm} loading={busy} loadingLabel={busyLabel}>
          {confirmLabel}
        </Button>
        <Button id={cancelId} onClick={onClose} loading={busy}>
          Cancel
        </Button>
      </div>
    </Overlay>
  );
}
