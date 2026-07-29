import { useState } from "react";
import { useNavigation } from "../../useNavigation";
import { noteKind, noteMeta } from "../../noteMeta";
import { useProjectNotes } from "../../useNotes";
import { useProjects } from "../../useProjects";
import { DeleteProjectDialog } from "../dialogs/DeleteProjectDialog";
import { Button } from "../ui/Button";
import { StatusMessage } from "../ui/StatusMessage";
import { ViewFrame } from "../ui/ViewFrame";
import "./ProjectView.css";

type Props = {
  slug: string;
};

/**
 * A project's notes, newest first — the app's LIBRARY, and deliberately the
 * opposite stance from the Inbox in every way that can be seen from across
 * the room: centred on a reading measure rather than pinned left, the airiest
 * gutter rather than the densest, the largest title in the app rather than no
 * title at all, and a flat index whose rows only tint rather than lift.
 *
 * Nothing here is waiting on you. The layout is the thing that says so.
 */
export function ProjectView({ slug }: Props) {
  const { navigate } = useNavigation();
  const { notes, loading, error } = useProjectNotes(slug);
  const { entries } = useProjects();
  const [confirmingDelete, setConfirmingDelete] = useState(false);

  // What deleting this project would take with it, from the sidebar listing
  // the shell already holds: the project's own notes plus everything under
  // `slug/` (descendant projects and their notes).
  const projectEntries = entries.filter((entry) => entry.kind === "project");
  const own = projectEntries.find((entry) => entry.project.slug === slug);
  const descendants = projectEntries.filter((entry) =>
    entry.project.slug.startsWith(`${slug}/`),
  );
  const containedNoteCount =
    (own?.project.note_count ?? notes.length) +
    descendants.reduce((sum, entry) => sum + entry.project.note_count, 0);

  return (
    <ViewFrame
      variant="library"
      eyebrow="Project"
      // The slug itself, not a prettified name: a project IS a folder, and the
      // mono-path vocabulary the filing menu and the search breadcrumbs use
      // should resolve to the same string that heads the view.
      title={slug}
      summary={
        notes.length > 0
          ? `${notes.length} ${notes.length === 1 ? "note" : "notes"}`
          : undefined
      }
      action={
        // Ink, not the reserved green: the two verbs on a reading screen earn
        // their place by being quiet text, not by being a colour. Creation
        // leads; the destructive one sits behind a confirmation
        // (DeleteProjectDialog) and so needs no weight of its own here.
        <div className="flex items-center gap-sm">
          <Button
            variant="quiet"
            className="py-3xs text-label text-text-soft"
            onClick={() => navigate({ kind: "noteEditor", noteId: null, project: slug })}
          >
            New note
          </Button>
          <Button
            variant="quiet"
            aria-haspopup="dialog"
            className="py-3xs text-label text-text-soft"
            onClick={() => setConfirmingDelete(true)}
          >
            Delete project
          </Button>
        </div>
      }
    >
      {error ? (
        <StatusMessage variant="error">Couldn&apos;t load notes: {error}</StatusMessage>
      ) : notes.length === 0 ? (
        // Gated on !loading as well, so a cold start shows nothing rather than
        // flashing the empty state before the first read lands.
        !loading && (
          <StatusMessage variant="empty">
            No notes here yet. Notes filed to this project land here.
          </StatusMessage>
        )
      ) : (
        <ul className="project__index">
          {notes.map((note) => (
            // Keyed by path, not id: two files can carry the same id (an
            // external copy), and duplicate keys would mis-reconcile rows.
            <li key={note.path}>
              <button
                type="button"
                className="project__row ui-focus-ring"
                onClick={() =>
                  navigate({ kind: "noteEditor", noteId: note.id, project: slug })
                }
              >
                {/* Serif, because in a library a title is something you read
                    rather than a control you operate. */}
                <span className="min-w-0 truncate font-serif text-h3 text-text">
                  {note.title}
                </span>
                {/* A right rail, so the dates line up into their own column
                    and the titles keep a clean left edge to scan down. */}
                <span className="flex-none whitespace-nowrap font-mono text-meta text-text-faint">
                  {noteMeta(note, noteKind(note.type))}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}

      {confirmingDelete && (
        <DeleteProjectDialog
          slug={slug}
          noteCount={containedNoteCount}
          descendantProjectCount={descendants.length}
          onClose={() => setConfirmingDelete(false)}
        />
      )}
    </ViewFrame>
  );
}
