import {
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { useDialogFocus } from "../useDialogFocus";
import { useNavigation } from "../useNavigation";
import { deleteProject } from "../useProjects";
import { wrapDialogTab } from "./dialogTabTrap";
import { Button } from "./ui/Button";
import { Overlay } from "./ui/Overlay";
import { StatusMessage } from "./ui/StatusMessage";

type Props = {
  /** The project to delete, as its canonical slug. */
  slug: string;
  /** Notes contained anywhere under the project (direct plus descendants). */
  noteCount: number;
  /** Child projects that will be removed with it. */
  descendantProjectCount: number;
  onClose: () => void;
};

const CANCEL_ID = "delete-project-cancel";

/**
 * The app's first destructive flow, shaped exactly as docs/DESIGN_SYSTEM.md §2
 * prescribes: the action is marked by CONFIRMATION, not colour. Cancel is the
 * primary control and holds initial focus; the confirming button is the
 * non-default `destructive` variant beside it.
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

  const panelRef = useRef<HTMLDivElement>(null);

  // Cancel is the default action of a confirmation, so it takes focus on open;
  // focus is restored on close.
  useDialogFocus(() => document.getElementById(CANCEL_ID));

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
      labelledBy="delete-project-title"
      panelRef={panelRef}
      onKeyDown={onKeyDown}
      className="flex flex-col gap-md p-md"
    >
      <h2 id="delete-project-title" className="font-serif text-title-panel text-text">
        Delete {slug}?
      </h2>

      <div className="flex flex-col gap-sm text-body text-text-soft">
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
      </div>

      {error && (
        <StatusMessage variant="error" compact>
          {error}
        </StatusMessage>
      )}

      <div className="flex items-center justify-end gap-sm">
        {/* The confirming control is the non-default one: destructive wears
            the quiet ghost's chrome, and the primary beside it is Cancel. */}
        <Button variant="destructive" onClick={confirm} loading={deleting} loadingLabel="Deleting…">
          Delete project
        </Button>
        <Button id={CANCEL_ID} onClick={onClose} loading={deleting}>
          Cancel
        </Button>
      </div>
    </Overlay>
  );
}
