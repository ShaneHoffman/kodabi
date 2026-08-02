import { useState } from "react";
import { deleteNote } from "../../useNotes";
import { DestructiveConfirmDialog } from "../ui/DestructiveConfirmDialog";

type Props = {
  /** The note to delete, by its stable id. */
  id: string;
  /** The note's title, named in the dialog's subject strip so the user is
   * confirming a thing rather than a pronoun. */
  noteTitle: string;
  /** Whether the note was distilled from a captured session. When true,
   * deleting the note also removes the paired recording and transcript, which
   * the confirmation copy states. */
  sessionBacked: boolean;
  /** Dismiss without deleting: Cancel, Escape, or a backdrop press. */
  onClose: () => void;
  /** Ran after a successful delete. The caller decides what success means:
   * navigate away from the open note, or play the inbox row's exit. The backend
   * broadcasts `vault:changed`, so lists refresh without extra wiring. */
  onDeleted: () => void;
};

/**
 * The note-delete confirmation — a third flow built on the shared
 * `DestructiveConfirmDialog` (docs/DESIGN_SYSTEM.md §2): marked by CONFIRMATION,
 * not colour, with Cancel holding initial focus. Unlike Needs Attention's
 * Dismiss (which hides a capture and deletes nothing), deleting a note is
 * destructive and cannot be undone.
 *
 * Presentational plus its own async: it owns the in-flight and error state and
 * calls `deleteNote`, then hands control back through `onDeleted`. Like the
 * primitive it composes, it never closes itself — the caller owns success.
 */
export function DeleteNoteDialog({ id, noteTitle, sessionBacked, onClose, onDeleted }: Props) {
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const confirm = async () => {
    setDeleting(true);
    setError(null);
    try {
      await deleteNote(id);
      onDeleted();
    } catch (err) {
      setError(`Couldn't delete the note: ${String(err)}`);
      setDeleting(false);
    }
  };

  return (
    <DestructiveConfirmDialog
      title="Delete this note?"
      subject={noteTitle}
      confirmLabel="Delete note"
      busyLabel="Deleting…"
      busy={deleting}
      error={error}
      onConfirm={confirm}
      onClose={onClose}
    >
      <p>
        {sessionBacked
          ? "Its recording and transcript are deleted with it."
          : "The note file is deleted from your vault."}
      </p>
    </DestructiveConfirmDialog>
  );
}
