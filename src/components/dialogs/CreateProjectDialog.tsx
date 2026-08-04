import { useRef, useState, type FormEvent } from "react";
import { useNavigation } from "../../useNavigation";
import { createProject, type Project } from "../../useProjects";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { Field } from "../ui/Field";

type Props = {
  onClose: () => void;
  /**
   * What to do with the created project instead of opening it.
   *
   * Creating a project is rarely the whole errand. Opened from the sidebar it
   * is — you made a folder and you want to see it, which is the default below.
   * Opened from an Inbox card's File menu the errand was "file this note
   * somewhere new", and navigating away would answer half of it and leave the
   * note sitting where it was. The caller that knows the errand says so here.
   */
  onCreated?: (project: Project) => void;
};

/**
 * The "New project" dialog: one name field, one committing action. The shell is
 * the Grove `Dialog`, so the focus trap, Escape, the scrim press, the scroll
 * lock and the focus restore are base-ui's rather than a hand-rolled Tab
 * wrapper — being mounted IS being open, and the caller unmounts to close.
 *
 * A field dialog opens ON its field (`initialFocus`), where a destructive one
 * opens on its safe action: there is nothing here to do by accident, and the
 * first thing the user wants is to type.
 *
 * Validation is the backend's alone (`vault::create_project` rejects reserved
 * or illegal names and duplicates); the frontend only trims and requires a
 * non-empty value, and surfaces the rejection on the field. On success it
 * navigates straight to the echoed canonical slug — the sidebar row arrives
 * via the backend's `vault:changed` broadcast — unless the caller passed
 * `onCreated`, which takes over from there.
 */
export function CreateProjectDialog({ onClose, onCreated }: Props) {
  const [name, setName] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { navigate } = useNavigation();

  const fieldRef = useRef<HTMLInputElement>(null);

  const trimmed = name.trim();

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!trimmed) return;
    setSubmitting(true);
    setError(null);
    try {
      const created = await createProject(trimmed);
      onClose();
      // Close first either way: whatever happens next — a navigation or the
      // caller's own follow-up — happens to a surface this dialog is no
      // longer covering.
      if (onCreated) onCreated(created);
      else navigate({ kind: "project", slug: created.slug });
    } catch (err) {
      setError(String(err));
      setSubmitting(false);
    }
  };

  return (
    <Dialog
      open
      onDismiss={onClose}
      labelledBy="create-project-title"
      initialFocus={fieldRef}
      className="flex flex-col gap-4"
    >
      <h2 id="create-project-title" className="text-[15px] font-semibold text-ink">
        New project
      </h2>

      <p className="text-[13px] leading-relaxed text-ink-dim">
        A project is a folder notes can be routed to.
      </p>

      <form onSubmit={submit} className="flex flex-col gap-4">
        <Field
          ref={fieldRef}
          label="Project name"
          placeholder="project-name"
          value={name}
          // readOnly, not disabled: a read-only input stays focusable and in
          // the tab order while the create is in flight (docs/DESIGN_SYSTEM.md §6).
          readOnly={submitting}
          onChange={(event) => setName(event.target.value)}
          error={error}
          // The nesting rule is a hint under the field, not a sentence in the
          // body: a tip about what you may type belongs where you are typing.
          hint={
            <>
              Nested paths like <span className="font-data">household/garage</span> work
              too.
            </>
          }
        />

        <div className="flex items-center justify-end gap-2.5">
          <Button variant="quiet" onClick={onClose} loading={submitting}>
            Cancel
          </Button>
          {/* The action that ends this surface. `disabled` is a genuine
              disable (nothing to submit), distinct from the busy state
              `loading` conveys. */}
          <Button
            type="submit"
            disabled={!trimmed}
            loading={submitting}
            loadingLabel="Creating…"
          >
            Create
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
