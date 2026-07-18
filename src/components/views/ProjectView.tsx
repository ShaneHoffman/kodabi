import { useNavigation } from "../../useNavigation";
import { useProjectNotes, type NoteSummary } from "../../useNotes";
import { formatSlug } from "../../useProjects";

type Props = {
  slug: string;
};

/** The quiet meta line a list row earns: day, then tags. */
function noteMeta(note: NoteSummary): string {
  return [note.date.slice(0, 10), ...note.tags].join(" · ");
}

/**
 * A project's notes, newest first — typography-first: serif titles carry the
 * list, hierarchy from type and space, no boxes. Loading renders nothing (the
 * list simply appears); the one action is a quiet New note.
 */
export function ProjectView({ slug }: Props) {
  const { navigate } = useNavigation();
  const { notes, loading, error } = useProjectNotes(slug);

  return (
    <section className="flex min-h-full flex-col p-xl">
      <div className="mx-auto flex w-full max-w-content flex-col gap-lg">
        <header className="flex items-baseline justify-between gap-md">
          <div className="flex flex-col gap-3xs">
            <p className="text-eyebrow uppercase tracking-wide text-text-faint">
              Project
            </p>
            <h2 className="font-serif text-h2 text-text">{formatSlug(slug)}</h2>
          </div>
          <button
            type="button"
            onClick={() =>
              navigate({ kind: "noteEditor", noteId: null, project: slug })
            }
            className="flex-none text-body text-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
          >
            New note
          </button>
        </header>

        {error ? (
          <p className="text-body text-text-soft">{error}</p>
        ) : notes.length === 0 ? (
          !loading && (
            <p className="text-body text-text-soft">No notes here yet.</p>
          )
        ) : (
          <ul className="flex flex-col gap-3xs">
            {notes.map((note) => (
              // Keyed by path, not id: two files can carry the same id (an
              // external copy), and duplicate keys would mis-reconcile rows.
              <li key={note.path}>
                <button
                  type="button"
                  onClick={() =>
                    navigate({ kind: "noteEditor", noteId: note.id, project: slug })
                  }
                  className="flex w-full items-baseline justify-between gap-md rounded-md py-2xs text-left text-text-soft hover:text-text focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
                >
                  <span className="font-serif text-body">{note.title}</span>
                  <span className="flex-none text-cap text-text-faint">
                    {noteMeta(note)}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </section>
  );
}
