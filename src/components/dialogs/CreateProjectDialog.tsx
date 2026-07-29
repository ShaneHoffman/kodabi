import {
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { useDialogFocus } from "../../useDialogFocus";
import { useNavigation } from "../../useNavigation";
import { createProject } from "../../useProjects";
import { wrapDialogTab } from "../../dialogTabTrap";
import { Button } from "../ui/Button";
import { Overlay } from "../ui/Overlay";
import { TextField } from "../ui/TextField";

type Props = {
  onClose: () => void;
};

const FIELD_ID = "create-project-name";

/**
 * The "New project" dialog: one name field, one committing action. Same
 * overlay shape as ConsentNudge (role=dialog, Escape/backdrop dismiss, Tab
 * wrap, focus save/restore via useDialogFocus).
 *
 * Validation is the backend's alone (`vault::create_project` rejects reserved
 * or illegal names and duplicates); the frontend only trims and requires a
 * non-empty value, and surfaces the rejection on the field. On success it
 * navigates straight to the echoed canonical slug — the sidebar row arrives
 * via the backend's `vault:changed` broadcast.
 */
export function CreateProjectDialog({ onClose }: Props) {
  const [name, setName] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { navigate } = useNavigation();

  const panelRef = useRef<HTMLDivElement>(null);

  // Focus the name field on open; restore focus on close.
  useDialogFocus(() => document.getElementById(FIELD_ID));

  const trimmed = name.trim();

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!trimmed) return;
    setSubmitting(true);
    setError(null);
    try {
      const created = await createProject(trimmed);
      onClose();
      navigate({ kind: "project", slug: created.slug });
    } catch (err) {
      setError(String(err));
      setSubmitting(false);
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
      labelledBy="create-project-title"
      panelRef={panelRef}
      onKeyDown={onKeyDown}
      className="flex flex-col gap-md p-md"
    >
      <h2 id="create-project-title" className="font-serif text-title-panel text-text">
        New project
      </h2>

      <form onSubmit={submit} className="flex flex-col gap-md">
        <TextField
          id={FIELD_ID}
          label="Project name"
          value={name}
          // readOnly, not disabled: a read-only input stays focusable and in
          // the tab order while the create is in flight (docs/DESIGN_SYSTEM.md §6).
          readOnly={submitting}
          onChange={(event) => setName(event.target.value)}
          error={error}
          hint="Use / to nest projects, for example Growth/Q3"
        />

        <div className="flex items-center justify-end gap-sm">
          <Button variant="quiet" onClick={onClose} loading={submitting}>
            Cancel
          </Button>
          {/* filled: the single action that ends this surface. `disabled` is a
              genuine disable (nothing to submit), distinct from the busy state
              `loading` conveys. */}
          <Button
            type="submit"
            variant="filled"
            disabled={!trimmed}
            loading={submitting}
            loadingLabel="Creating…"
          >
            Create
          </Button>
        </div>
      </form>
    </Overlay>
  );
}
