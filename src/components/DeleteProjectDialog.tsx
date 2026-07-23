import { useState } from "react";
import { useNavigation } from "../useNavigation";
import { deleteProject } from "../useProjects";
import { DestructiveConfirmDialog } from "./ui/DestructiveConfirmDialog";

type Props = {
  /** The project to delete, as its canonical slug. */
  slug: string;
  /** Notes contained anywhere under the project (direct plus descendants). */
  noteCount: number;
  /** Child projects that will be removed with it. */
  descendantProjectCount: number;
  onClose: () => void;
};

/**
 * The app's first destructive flow, and now one of two built on the shared
 * `DestructiveConfirmDialog` (docs/DESIGN_SYSTEM.md §2): the action is marked by
 * CONFIRMATION, not colour, with Cancel holding initial focus.
 *
 * Nothing is lost on confirm: the backend (`vault::delete_project`) moves
 * every contained note back to the Inbox before removing the folder tree, so
 * on success this navigates to the Inbox, where the user's notes now are.
 */
export function DeleteProjectDialog({
  slug,
  noteCount,
  descendantProjectCount,
  onClose,
}: Props) {
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { navigate } = useNavigation();

  const confirm = async () => {
    setDeleting(true);
    setError(null);
    try {
      await deleteProject(slug);
      onClose();
      navigate({ kind: "inbox" });
    } catch (err) {
      setError(`Couldn't delete the project: ${String(err)}`);
      setDeleting(false);
    }
  };

  return (
    <DestructiveConfirmDialog
      title={`Delete ${slug}?`}
      confirmLabel="Delete project"
      busyLabel="Deleting…"
      busy={deleting}
      error={error}
      onConfirm={confirm}
      onClose={onClose}
    >
      <p>
        {noteCount === 0
          ? "This project has no notes."
          : `Its ${noteCount} ${
              noteCount === 1 ? "note" : "notes"
            } will move back to the Inbox.`}
      </p>
      {descendantProjectCount > 0 && (
        <p>
          This also deletes {descendantProjectCount}{" "}
          {descendantProjectCount === 1 ? "project" : "projects"} inside it.
        </p>
      )}
    </DestructiveConfirmDialog>
  );
}
