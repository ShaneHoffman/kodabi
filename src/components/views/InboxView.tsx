import { useMemo, useState } from "react";
import { useNavigation } from "../../useNavigation";
import {
  fileNoteToProject,
  INBOX_PROJECT,
  useProjectNotes,
  type NoteSummary,
} from "../../useNotes";
import { formatSlug, useProjects } from "../../useProjects";
import { Select, type SelectOption } from "../ui/Select";
import "./InboxView.css";

/** The quiet meta line for an unfiled note: day, routing score, then tags. */
function inboxMeta(note: NoteSummary): string {
  const score = `${Math.round((note.confidence ?? 0) * 100)}% match`;
  return [note.date.slice(0, 10), score, ...note.tags].join(" · ");
}

/**
 * The Inbox: notes the router couldn't place with confidence, each with a
 * one-click re-route to the correct project (the correction loop —
 * FOUNDING_DOC §3.5). The list reads straight from the `Inbox/` folder; a
 * re-route moves the file, re-scores it, and logs a routing example, then the
 * corrected row settles out of view. Loading renders nothing (the list simply
 * appears), matching ProjectView.
 */
export function InboxView() {
  const { notes, loading, error } = useProjectNotes(INBOX_PROJECT);
  const { entries } = useProjects();
  // Real projects only — the Inbox itself is never a re-route target. Memoized
  // (entries is stable from useProjects) so the same array reference reaches
  // every InboxRow's Select and doesn't force a re-render of every row.
  const options = useMemo<SelectOption[]>(
    () =>
      entries.flatMap((entry) =>
        entry.kind === "project"
          ? [{ value: entry.project.slug, label: formatSlug(entry.project.slug) }]
          : [],
      ),
    [entries],
  );

  return (
    <section className="flex min-h-full flex-col p-xl">
      <div className="mx-auto flex w-full max-w-content flex-col gap-lg">
        <header className="flex flex-col gap-3xs">
          <p className="text-eyebrow uppercase tracking-wide text-text-faint">
            Unfiled
          </p>
          <h2 className="font-serif text-h2 text-text">Inbox</h2>
        </header>

        {error ? (
          <p className="text-body text-text-soft">{error}</p>
        ) : notes.length === 0 ? (
          !loading && (
            <p className="text-body text-text-soft">
              Nothing waiting. Notes the router can&apos;t place land here.
            </p>
          )
        ) : (
          <ul className="flex flex-col gap-3xs">
            {notes.map((note) => (
              // Keyed by path, not id: two files can carry the same id (an
              // external copy), and duplicate keys would mis-reconcile rows.
              <InboxRow key={note.path} note={note} options={options} />
            ))}
          </ul>
        )}
      </div>
    </section>
  );
}

/**
 * One unfiled note. Opening it (the left region) navigates to the editor;
 * choosing a project (the right picker) re-routes it. On success the row plays
 * its collapse/fade exit; the re-route command broadcasts `vault:changed`
 * itself, which refetches the list and the sidebar badge together and drops the
 * row (the file watcher is a fallback for external edits, not the only trigger,
 * so the row leaves even if the watcher never started). A failed re-route keeps
 * the row and surfaces the backend message.
 */
function InboxRow({
  note,
  options,
}: {
  note: NoteSummary;
  options: SelectOption[];
}) {
  const { navigate } = useNavigation();
  const [pending, setPending] = useState(false);
  const [leaving, setLeaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const route = (slug: string) => {
    setPending(true);
    setError(null);
    fileNoteToProject({ id: note.id, project: slug })
      .then(() => {
        // Play the exit transition (InboxView.css, 0.2s). The re-route command
        // broadcasts `vault:changed` itself, so the list refetches and drops the
        // row promptly (and still converges if the file watcher never started);
        // the fade is best-effort polish for the brief window before it does.
        setLeaving(true);
      })
      .catch((err: unknown) => {
        setPending(false);
        setError(String(err));
      });
  };

  return (
    <li className={`inbox-row${leaving ? " inbox-row--leaving" : ""}`}>
      <div className="flex flex-col gap-2xs py-2xs">
        <div className="flex items-start justify-between gap-md">
          <button
            type="button"
            onClick={() =>
              navigate({
                kind: "noteEditor",
                noteId: note.id,
                project: INBOX_PROJECT,
              })
            }
            className="flex min-w-0 flex-1 flex-col gap-3xs rounded-md text-left focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
          >
            <span className="font-serif text-body text-text-soft hover:text-text">
              {note.title}
            </span>
            <span className="text-cap text-text-faint">{inboxMeta(note)}</span>
            {note.snippet && (
              <span className="inbox-row__snippet text-cap text-text-soft">
                {note.snippet}
              </span>
            )}
          </button>
          <div className="w-48 flex-none">
            {options.length === 0 ? (
              <p className="text-cap text-text-faint">
                Create a project to file notes.
              </p>
            ) : pending ? (
              <span className="text-cap text-text-faint">Filing…</span>
            ) : (
              <Select
                hideLabel
                label={`File "${note.title}" to project`}
                value={null}
                placeholder="File to…"
                options={options}
                onChange={route}
              />
            )}
          </div>
        </div>
        {error && <p className="text-cap text-text-soft">{error}</p>}
      </div>
    </li>
  );
}
