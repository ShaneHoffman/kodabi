import { useRef, useState, type FormEvent } from "react";
import { useNavigation } from "../../useNavigation";
import { renameProject } from "../../useProjects";
import { backendCopy } from "../../errorCopy";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { Field } from "../ui/Field";

type Props = {
  /** The project's current full slug, including any parent path. */
  slug: string;
  onClose: () => void;
};

/**
 * The "Rename project" dialog: one name field, one committing action. Built on
 * the Grove `Dialog` and opening ON its field, like `CreateProjectDialog` and
 * unlike a destructive confirm, which opens on its safe action.
 *
 * The field holds the **full slug**, not just the leaf, because the slug is the
 * project's identity: it is the folder path, the handle every note stores in
 * frontmatter, and the name the user types when filing. Editing the parent part
 * therefore moves the project, which the hint says out loud.
 *
 * Validation is the backend's alone (`vault::rename_project` rejects reserved,
 * illegal and taken names, and a move into the project's own subtree); the
 * frontend only trims, requires a non-empty value, and requires a change. The
 * comparison is exact rather than case-insensitive so a case-only rename
 * (`ops` from `Ops`) stays submittable, which the backend honours.
 *
 * On success it navigates to the echoed canonical slug. That navigation is not
 * cosmetic: the current view still names the old project, whose folder no
 * longer exists, so without it the header and note list would silently read
 * empty. Every other surface refreshes off the backend's `vault:changed`.
 */
export function RenameProjectDialog({ slug, onClose }: Props) {
  const [name, setName] = useState(slug);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { navigate } = useNavigation();

  const fieldRef = useRef<HTMLInputElement>(null);

  const trimmed = name.trim();
  const unchanged = trimmed === slug;

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!trimmed || unchanged) return;
    setSubmitting(true);
    setError(null);
    try {
      const renamed = await renameProject(slug, trimmed);
      // Close first: the navigation lands on a surface this dialog is no
      // longer covering.
      onClose();
      navigate({ kind: "project", slug: renamed.slug });
    } catch (err) {
      // As in `CreateProjectDialog`: the validation detail is the point.
      setError(
        backendCopy(
          err,
          "Couldn't rename the project. It keeps its current name; try again.",
        ),
      );
      setSubmitting(false);
    }
  };

  return (
    <Dialog
      open
      onDismiss={onClose}
      labelledBy="rename-project-title"
      initialFocus={fieldRef}
      className="flex flex-col gap-4"
    >
      <h2 id="rename-project-title" className="text-[15px] font-semibold text-ink">
        Rename project
      </h2>

      <p className="text-[13px] leading-relaxed text-ink-dim">
        The folder is renamed and every note inside it is re-filed, child
        projects included.
      </p>

      <form onSubmit={submit} className="flex flex-col gap-4">
        <Field
          ref={fieldRef}
          label="Project name"
          placeholder="project-name"
          value={name}
          // readOnly, not disabled: a read-only input stays focusable and in
          // the tab order while the rename is in flight (docs/DESIGN_SYSTEM.md §6).
          readOnly={submitting}
          onChange={(event) => setName(event.target.value)}
          error={error}
          hint={
            <>
              Changing the path ahead of the name moves the project, so{" "}
              <span className="font-data">archive/2026</span> files it under{" "}
              <span className="font-data">archive</span>.
            </>
          }
        />

        <div className="flex items-center justify-end gap-2.5">
          <Button variant="quiet" onClick={onClose} loading={submitting}>
            Cancel
          </Button>
          {/* `disabled` is a genuine disable (nothing to submit, or nothing
              changed), distinct from the busy state `loading` conveys. */}
          <Button
            type="submit"
            disabled={!trimmed || unchanged}
            loading={submitting}
            loadingLabel="Renaming…"
          >
            Rename
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
