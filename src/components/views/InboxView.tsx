import { useMemo, useState } from "react";
import { useNavigation } from "../../useNavigation";
import { matchScore, noteMeta } from "../../noteMeta";
import {
  fileNoteToProject,
  INBOX_PROJECT,
  useProjectNotes,
  type NoteSummary,
} from "../../useNotes";
import { useProjects } from "../../useProjects";
import { Select, type SelectOption } from "../ui/Select";
import { StatusMessage } from "../ui/StatusMessage";
import { ViewFrame } from "../ui/ViewFrame";
import "./InboxView.css";

/**
 * The Inbox: notes the router couldn't place with confidence, each with a
 * one-click file-to-project (the correction loop — FOUNDING_DOC §3.5). The
 * list reads straight from the `Inbox/` folder; filing moves the file,
 * re-scores it, and logs a routing example, then the row settles out of view.
 *
 * It is the app's one WORKING QUEUE, and the layout says so before a word is
 * read: pinned hard left, the densest gutter in the app, a compact one-line
 * masthead instead of a title, and the only rows that lift under the pointer.
 * Nothing here is for browsing. Everything here is for clearing.
 */
export function InboxView() {
  const { notes, loading, error } = useProjectNotes(INBOX_PROJECT);
  const { entries } = useProjects();
  // Counted in this session rather than read from disk: nothing persists a
  // daily tally, and inventing one that resets at midnight would be claiming
  // more than the app knows. This says exactly what it means — how many you
  // have cleared since opening the app.
  const [filedThisSession, setFiledThisSession] = useState(0);

  // Real projects only — the Inbox itself is never a filing target. Memoized
  // (entries is stable from useProjects) so the same array reference reaches
  // every row's picker and doesn't force a re-render of every row.
  const options = useMemo<SelectOption[]>(
    () =>
      entries.flatMap((entry) =>
        entry.kind === "project"
          ? // The menu lists PATHS, not display names: filing is choosing a
            // location, and a nested project's parentage is the whole reason
            // you'd pick it over its sibling.
            [{ value: entry.project.slug, label: entry.project.slug }]
          : [],
      ),
    [entries],
  );

  const remaining = notes.length;
  const handled = filedThisSession + remaining;
  const cleared = handled > 0 ? (filedThisSession / handled) * 100 : 0;

  return (
    <ViewFrame
      variant="queue"
      eyebrow="Unfiled"
      title="Inbox"
      // The work, stated before the list is read. Omitted at zero: the empty
      // state below says it better, and saying it twice says it worse.
      summary={remaining > 0 ? `${remaining} to file` : undefined}
    >
      {error ? (
        <StatusMessage variant="error">Couldn&apos;t load the inbox: {error}</StatusMessage>
      ) : remaining === 0 ? (
        !loading && (
          <StatusMessage variant="empty">
            Nothing waiting. Notes the router can&apos;t place land here.
          </StatusMessage>
        )
      ) : (
        <>
          <Progress filed={filedThisSession} remaining={remaining} percent={cleared} />
          <ul className="inbox__list">
            {notes.map((note) => (
              // Keyed by path, not id: two files can carry the same id (an
              // external copy), and duplicate keys would mis-reconcile rows.
              <InboxRow
                key={note.path}
                note={note}
                options={options}
                onFiled={() => setFiledThisSession((count) => count + 1)}
              />
            ))}
          </ul>
        </>
      )}
    </ViewFrame>
  );
}

/**
 * How much of the queue you have cleared, as a rule and a sentence.
 *
 * It sits between the masthead and the list because that is where a progress
 * reading is useful — before you start, not after you scroll. At zero filed it
 * still renders: an empty track that says "4 to go" is the honest starting
 * state, and a bar that only appears once you are winning is a bar that never
 * helps you begin.
 */
function Progress({
  filed,
  remaining,
  percent,
}: {
  filed: number;
  remaining: number;
  percent: number;
}) {
  return (
    <div className="inbox__progress mt-sm">
      <div className="inbox__track">
        <div className="inbox__fill" style={{ width: `${percent}%` }} />
      </div>
      <p className="mt-2xs font-mono text-eyebrow text-text-faint">
        {filed} filed this session · {remaining} to go
      </p>
    </div>
  );
}

/**
 * One unfiled note. The title opens it; the picker in the right gutter files
 * it. On success the row plays its collapse/fade exit; the file command
 * broadcasts `vault:changed` itself, which refetches the list and the sidebar
 * count together and drops the row (the file watcher is a fallback for
 * external edits, not the only trigger, so the row leaves even if the watcher
 * never started). A failed file keeps the row and surfaces the message.
 */
function InboxRow({
  note,
  options,
  onFiled,
}: {
  note: NoteSummary;
  options: SelectOption[];
  onFiled: () => void;
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
        onFiled();
        setLeaving(true);
      })
      .catch((err: unknown) => {
        setPending(false);
        setError(String(err));
      });
  };

  return (
    <li className={`inbox__slot${leaving ? " inbox__slot--leaving" : ""}`}>
      <div>
        <div className="inbox__row">
          <div>
            <button
              type="button"
              className="inbox__open ui-focus-ring text-row font-semibold tracking-row text-text"
              onClick={() =>
                navigate({
                  kind: "noteEditor",
                  noteId: note.id,
                  project: INBOX_PROJECT,
                })
              }
            >
              {note.title}
            </button>
            <p className="mt-2xs font-mono text-cap text-text-faint">
              {noteMeta(note, matchScore(note.confidence))}
            </p>
            {note.snippet && (
              <p className="inbox__snippet mt-2xs font-serif text-snippet leading-snippet text-text-soft">
                {note.snippet}
              </p>
            )}
          </div>
          <div>
            {options.length === 0 ? (
              // No picker at all when there is nothing to file into: a control
              // whose only outcome is a dead end should not be offered.
              // variant="empty", not "status": the variant fixes the ARIA role,
              // and a static sentence repeated once per row must not be N live
              // regions (docs/DESIGN_SYSTEM.md §3).
              <StatusMessage variant="empty" compact>
                Create a project to file notes.
              </StatusMessage>
            ) : (
              <Select
                hideLabel
                variant="token"
                label={`File "${note.title}" to project`}
                value={null}
                placeholder={pending ? "Filing" : "File"}
                options={options}
                disabled={pending}
                onChange={route}
              />
            )}
          </div>
        </div>
        {error && (
          <StatusMessage variant="error" compact>
            Couldn&apos;t file this note: {error}
          </StatusMessage>
        )}
      </div>
    </li>
  );
}
