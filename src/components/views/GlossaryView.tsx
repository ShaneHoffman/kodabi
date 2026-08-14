import { useState } from "react";
import { deleteGlossaryTerm, useGlossary, type GlossaryTerm } from "../../useGlossary";
import { GlossaryTermDialog } from "../dialogs/GlossaryTermDialog";
import { Button } from "../ui/Button";
import { DestructiveConfirmDialog } from "../ui/DestructiveConfirmDialog";
import { StatusMessage } from "../ui/StatusMessage";
import { ViewFrame } from "../ui/ViewFrame";

type Props = {
  /** Which glossary this view edits: `null` is the vault-wide one. */
  slug: string | null;
};

/** Which dialog is open, and for which entry. */
type OpenDialog = { mode: "add" } | { mode: "edit"; term: GlossaryTerm };

/** The key row state is filed under: the same normalized identity the backend
 * matches a term by, so a re-cased edit doesn't strand an error under the old
 * spelling. */
function termKey(term: string): string {
  return term.trim().toLowerCase();
}

/**
 * The glossary editor: one list of terms, with add, edit and delete.
 *
 * TWO SCOPES, ONE SCREEN, and the difference is worth a sentence on the page
 * rather than a shrug. The vault-wide glossary at the knowledge-base root is
 * the one that biases TRANSCRIPTION: a session is transcribed before routing
 * has picked a project, so a project's own glossary cannot reach the audio. A
 * project glossary earns its keep elsewhere — routing signals, and the context
 * an agent reads. Both are the same file format in different folders, which is
 * why one view serves both; but a user adding a coworker's name to the wrong
 * one would get silence and no explanation, so the scope line says which is
 * which every time.
 *
 * A library stance: this is a place to browse and maintain, not a queue with
 * work in it. Rows are flat and hairline-separated rather than lifted cards —
 * nothing here flagged itself, and the plane is not an affordance
 * (docs/UI_CONVENTIONS.md §4).
 */
export function GlossaryView({ slug }: Props) {
  const { terms, loading, error } = useGlossary(slug);

  const [dialog, setDialog] = useState<OpenDialog | null>(null);
  // The term whose delete confirmation is open, plus that modal's own busy and
  // error state. Delete is the one destructive verb here, so it always goes
  // through the shared confirmation rather than firing on a row's first click.
  const [confirmingDelete, setConfirmingDelete] = useState<GlossaryTerm | null>(null);
  const [deleteBusy, setDeleteBusy] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  // Drop dialog state that the refetched list no longer has a row for: a term
  // deleted in another window (or hand-edited out of the file) must not leave
  // an edit form open over an entry that is gone, nor a confirmation asking
  // about it. Pruned during render when the list identity changes (React's
  // adjust-state-on-prop-change pattern) so the drop lands before paint.
  const [previousTerms, setPreviousTerms] = useState(terms);
  if (previousTerms !== terms) {
    setPreviousTerms(terms);
    const listed = new Set(terms.map((entry) => termKey(entry.term)));
    if (confirmingDelete !== null && !listed.has(termKey(confirmingDelete.term))) {
      setConfirmingDelete(null);
      setDeleteError(null);
    }
    if (dialog?.mode === "edit" && !listed.has(termKey(dialog.term.term))) {
      setDialog(null);
    }
  }

  const confirmDelete = () => {
    if (confirmingDelete === null) return;
    setDeleteBusy(true);
    setDeleteError(null);
    deleteGlossaryTerm(slug, confirmingDelete.term)
      .then(() => {
        // The list converges on the backend's `vault:changed` broadcast; all
        // this has to do is stop asking the question.
        setConfirmingDelete(null);
      })
      .catch((err: unknown) => {
        setDeleteError(`Couldn't delete the term: ${String(err)}`);
      })
      .finally(() => setDeleteBusy(false));
  };

  const vaultWide = slug === null;
  const title = vaultWide ? "Vault glossary" : slug.slice(slug.lastIndexOf("/") + 1);
  const countLine = `${terms.length} ${terms.length === 1 ? "term" : "terms"}`;

  return (
    <ViewFrame
      variant="library"
      eyebrow="Glossary"
      title={title}
      summary={
        terms.length > 0
          ? // The slug is how a project is addressed everywhere else, so it
            // stays beside the count; the vault glossary has no second name.
            vaultWide
            ? countLine
            : `${countLine} · ${slug}`
          : // At zero the count's voice yields to the empty state.
            undefined
      }
      action={
        <Button variant="quiet" aria-haspopup="dialog" onClick={() => setDialog({ mode: "add" })}>
          Add term
        </Button>
      }
    >
      {/* Which glossary this is, in one line, because the two behave
          differently and the difference is invisible otherwise. */}
      <p className="mt-2.5 max-w-[62ch] text-[11.5px] leading-relaxed text-ink-dim">
        {vaultWide
          ? "These terms bias transcription for every capture. Add product names, coworkers, and jargon you want spelled right."
          : "These terms help route notes to this project and give agents context. Transcription is biased by the vault glossary."}
      </p>

      {error ? (
        <div className="mt-[26px] flex flex-col gap-2">
          <StatusMessage variant="error">Couldn&apos;t load the glossary: {error}</StatusMessage>
          {/* The core error names the file and the offending term, so the
              guidance only has to say where to go from here. */}
          <p className="text-[11.5px] leading-relaxed text-ink-dim">
            Fix the file in a text editor, then reopen this view.
          </p>
        </div>
      ) : terms.length === 0 ? (
        // Gated on !loading as well, so a cold start shows nothing rather than
        // flashing the empty state before the first read lands.
        !loading && (
          <div className="mt-[26px]">
            <StatusMessage variant="empty">
              {vaultWide
                ? "No terms yet. Add the names transcription keeps getting wrong."
                : "No terms yet. Terms added here help route notes to this project."}
            </StatusMessage>
          </div>
        )
      ) : (
        <ul className="mt-[26px]" data-testid="glossary-terms">
          {terms.map((entry) => (
            <li
              key={termKey(entry.term)}
              className="flex items-start justify-between gap-6 border-t border-edge py-4 first:border-t-0"
            >
              <div className="min-w-0">
                <p className="text-[14.5px] font-semibold text-ink">{entry.term}</p>
                <p className="mt-1 text-[12.5px] leading-relaxed text-ink-dim">
                  {entry.definition}
                </p>
                {/* Aliases read in the data face because that is what they
                    are: alternate spellings, not prose. Omitted entirely when
                    there are none rather than rendering an empty label. */}
                {entry.aliases.length > 0 && (
                  <p className="mt-1 font-data text-[10.5px] text-ink-faint">
                    also: {entry.aliases.join(" · ")}
                  </p>
                )}
              </div>
              {/* Two affordances, the row ceiling (docs/UI_CONVENTIONS.md §5),
                  behind a hairline so they read as this row's options rather
                  than page controls that happen to sit nearby. */}
              <div className="flex flex-none flex-col items-stretch gap-1.5 border-l border-edge pl-6">
                <Button
                  variant="quiet"
                  aria-haspopup="dialog"
                  onClick={() => setDialog({ mode: "edit", term: entry })}
                >
                  Edit
                </Button>
                <Button
                  variant="quiet"
                  aria-haspopup="dialog"
                  onClick={() => {
                    setDeleteError(null);
                    setConfirmingDelete(entry);
                  }}
                >
                  Delete
                </Button>
              </div>
            </li>
          ))}
        </ul>
      )}

      {dialog && (
        <GlossaryTermDialog
          slug={slug}
          initial={dialog.mode === "edit" ? dialog.term : undefined}
          onClose={() => setDialog(null)}
        />
      )}

      {confirmingDelete && (
        <DestructiveConfirmDialog
          title="Delete this term?"
          subject={confirmingDelete.term}
          confirmLabel="Delete term"
          busyLabel="Deleting…"
          busy={deleteBusy}
          error={deleteError}
          onConfirm={confirmDelete}
          onClose={() => {
            setConfirmingDelete(null);
            setDeleteError(null);
          }}
        >
          <p>
            {vaultWide
              ? "Transcription stops recognizing it on future captures."
              : "Routing and project context stop seeing it."}
          </p>
        </DestructiveConfirmDialog>
      )}
    </ViewFrame>
  );
}
