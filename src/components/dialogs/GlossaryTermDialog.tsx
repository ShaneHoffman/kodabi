import { useRef, useState, type FormEvent } from "react";
import {
  addGlossaryTerm,
  formatAliases,
  parseAliases,
  updateGlossaryTerm,
  type GlossaryTerm,
} from "../../useGlossary";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { Field } from "../ui/Field";

type Props = {
  /** Which glossary to write to: `null` is the vault-wide one. */
  slug: string | null;
  /**
   * The entry being edited, or absent to add a new one. Present means the
   * fields open prefilled and the submit replaces that entry — including
   * renaming it, since the term field is editable either way.
   */
  initial?: GlossaryTerm;
  onClose: () => void;
};

/**
 * The add / edit dialog for one glossary term. One dialog for both verbs
 * because they are the same three fields against the same validation: the only
 * difference is whether an existing entry is named as the thing being replaced.
 *
 * A field dialog opens ON its field (`initialFocus`), where a destructive one
 * opens on its safe action. Definition is a single-line field rather than a
 * textarea: a definition here is a gloss fed to a transcription prompt, not a
 * document, and the 2000-character cap is a bound rather than an invitation.
 *
 * Validation is the backend's (`vault::upsert_glossary_term` and its update
 * sibling own the length bounds, the conflict rule and the rename check); the
 * frontend only requires non-empty term and definition, and surfaces whatever
 * the backend rejects on the term field, which is what both the conflict and
 * the not-found messages name.
 */
export function GlossaryTermDialog({ slug, initial, onClose }: Props) {
  const [term, setTerm] = useState(initial?.term ?? "");
  const [definition, setDefinition] = useState(initial?.definition ?? "");
  const [aliases, setAliases] = useState(formatAliases(initial?.aliases ?? []));
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const fieldRef = useRef<HTMLInputElement>(null);

  const editing = initial !== undefined;
  const trimmedTerm = term.trim();
  const trimmedDefinition = definition.trim();
  const submittable = trimmedTerm.length > 0 && trimmedDefinition.length > 0;

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!submittable) return;
    setSubmitting(true);
    setError(null);
    const input = {
      term: trimmedTerm,
      definition: trimmedDefinition,
      aliases: parseAliases(aliases),
    };
    try {
      if (initial) await updateGlossaryTerm(slug, initial.term, input);
      else await addGlossaryTerm(slug, input);
      // The list refreshes off the backend's `vault:changed` broadcast, so
      // closing is the whole of the success path.
      onClose();
    } catch (err) {
      setError(String(err));
      setSubmitting(false);
    }
  };

  return (
    <Dialog
      open
      onDismiss={onClose}
      labelledBy="glossary-term-title"
      initialFocus={fieldRef}
      className="flex flex-col gap-4"
    >
      <h2 id="glossary-term-title" className="text-[15px] font-semibold text-ink">
        {editing ? "Edit term" : "Add term"}
      </h2>

      <p className="text-[13px] leading-relaxed text-ink-dim">
        {slug === null
          ? "Vault terms bias transcription so names and jargon come through spelled right."
          : "Project terms help route notes here and give agents context."}
      </p>

      <form onSubmit={submit} className="flex flex-col gap-4">
        <Field
          ref={fieldRef}
          label="Term"
          placeholder="MERIDIAN"
          value={term}
          maxLength={200}
          // readOnly, not disabled: a read-only input stays focusable and in
          // the tab order while the write is in flight (docs/DESIGN_SYSTEM.md §6).
          readOnly={submitting}
          onChange={(event) => setTerm(event.target.value)}
          error={error}
        />

        <Field
          label="Definition"
          placeholder="A regional systems-migration project."
          value={definition}
          maxLength={2000}
          readOnly={submitting}
          onChange={(event) => setDefinition(event.target.value)}
        />

        <Field
          label="Aliases"
          placeholder="meridian, mer-idian"
          value={aliases}
          readOnly={submitting}
          onChange={(event) => setAliases(event.target.value)}
          hint="Separate aliases with commas. They resolve to the term when heard."
        />

        <div className="flex items-center justify-end gap-2.5">
          <Button variant="quiet" onClick={onClose} loading={submitting}>
            Cancel
          </Button>
          {/* `disabled` is a genuine disable (nothing to submit), distinct
              from the busy state `loading` conveys. */}
          <Button
            type="submit"
            disabled={!submittable}
            loading={submitting}
            loadingLabel={editing ? "Saving…" : "Adding…"}
          >
            {editing ? "Save term" : "Add term"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
