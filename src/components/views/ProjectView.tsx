import { useNavigation } from "../../useNavigation";
import { noteMeta } from "../../noteMeta";
import { useProjectNotes } from "../../useNotes";
import { formatSlug } from "../../useProjects";
import { Button } from "../ui/Button";
import { ListRow } from "../ui/ListRow";
import { StatusMessage } from "../ui/StatusMessage";
import { ViewFrame } from "../ui/ViewFrame";

type Props = {
  slug: string;
};

/**
 * A project's notes, newest first — typography-first: serif titles carry the
 * list, hierarchy from type and space, no boxes. Loading renders nothing (the
 * list simply appears); the one action is a quiet New note.
 */
export function ProjectView({ slug }: Props) {
  const { navigate } = useNavigation();
  const { notes, loading, error } = useProjectNotes(slug);

  return (
    <ViewFrame
      eyebrow="Project"
      title={formatSlug(slug)}
      action={
        <Button
          variant="quiet"
          className="text-body text-accent"
          onClick={() => navigate({ kind: "noteEditor", noteId: null, project: slug })}
        >
          New note
        </Button>
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
        <ul className="flex flex-col gap-3xs">
          {notes.map((note) => (
            // Keyed by path, not id: two files can carry the same id (an
            // external copy), and duplicate keys would mis-reconcile rows.
            <li key={note.path}>
              <ListRow
                layout="inline"
                title={note.title}
                meta={noteMeta(note)}
                onOpen={() =>
                  navigate({ kind: "noteEditor", noteId: note.id, project: slug })
                }
              />
            </li>
          ))}
        </ul>
      )}
    </ViewFrame>
  );
}
