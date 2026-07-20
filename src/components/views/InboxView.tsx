import { useMemo, useState } from "react";
import { useNavigation } from "../../useNavigation";
import { matchScore, noteMeta } from "../../noteMeta";
import {
  fileNoteToProject,
  INBOX_PROJECT,
  useProjectNotes,
  type NoteSummary,
} from "../../useNotes";
import { formatSlug, useProjects } from "../../useProjects";
import { ListRow } from "../ui/ListRow";
import { Select, type SelectOption } from "../ui/Select";
import { StatusMessage } from "../ui/StatusMessage";
import { ViewFrame } from "../ui/ViewFrame";
import "./InboxView.css";

/**
 * The Inbox: notes the router couldn't place with confidence, each with a
 * one-click re-route to the correct project (the correction loop —
 * FOUNDING_DOC §3.5). The list reads straight from the `Inbox/` folder; a
 * re-route moves the file, re-scores it, and logs a routing example, then the
 * corrected row settles out of view. Loading renders nothing (the list simply
 * appears), matching ProjectView.
 *
 * It has exactly one job: decide where these notes go. Captures that failed to
 * become a note used to sit above this list wearing the same clothes — a queue
 * about system health stacked on a queue about filing, no heading between them.
 * They now have their own view (NeedsAttentionView).
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
    <ViewFrame
      variant="queue"
      eyebrow="Unfiled"
      title="Inbox"
      // The work, stated before the list is read. Omitted at zero: the empty
      // state below says it better, and saying it twice says it worse.
      summary={
        notes.length > 0
          ? `${notes.length} ${notes.length === 1 ? "note" : "notes"} to file`
          : undefined
      }
    >
      {error ? (
        <StatusMessage variant="error">Couldn&apos;t load the inbox: {error}</StatusMessage>
      ) : notes.length === 0 ? (
        !loading && (
          <StatusMessage variant="empty">
            Nothing waiting. Notes the router can&apos;t place land here.
          </StatusMessage>
        )
      ) : (
        // A row here is three lines tall (title, meta, snippet). At gap-3xs the
        // space between two rows was no bigger than the space between a row's
        // own lines, so the list read as one undifferentiated block: the gap
        // separating rows has to beat the gap inside them.
        <ul className="flex flex-col gap-md">
          {notes.map((note) => (
            // Keyed by path, not id: two files can carry the same id (an
            // external copy), and duplicate keys would mis-reconcile rows.
            <InboxRow key={note.path} note={note} options={options} />
          ))}
        </ul>
      )}
    </ViewFrame>
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
        // Play the exit transition (InboxView.css, --dur-settle). The re-route
        // command broadcasts `vault:changed` itself, so the list refetches and
        // drops the row promptly (and still converges if the file watcher never
        // started); the fade is best-effort polish for the brief window before
        // it does.
        setLeaving(true);
      })
      .catch((err: unknown) => {
        setPending(false);
        setError(String(err));
      });
  };

  return (
    <li className={`inbox-row${leaving ? " inbox-row--leaving" : ""}`}>
      <div className="flex flex-col gap-2xs">
        <ListRow
          title={note.title}
          meta={noteMeta(note, matchScore(note.confidence))}
          snippet={note.snippet}
          onOpen={() =>
            navigate({
              kind: "noteEditor",
              noteId: note.id,
              project: INBOX_PROJECT,
            })
          }
          action={
            options.length === 0 ? (
              // No picker at all when there is nothing to file into: a control
              // whose only outcome is a dead end should not be offered.
              // variant="empty", not "status": the variant fixes the ARIA role,
              // and a static sentence repeated once per row must not be N live
              // regions (docs/DESIGN_SYSTEM.md §3).
              <StatusMessage variant="empty" compact>
                Create a project to file notes.
              </StatusMessage>
            ) : (
              // The picker stays mounted through the whole re-route rather than
              // being replaced by a <span>Filing…</span>, so the row does not
              // reflow under the user and the message sits on the control that
              // earned it (docs/DESIGN_SYSTEM.md §6).
              //
              // It does NOT preserve focus: `disabled` blurs a focused control
              // just as unmounting it does (the HTML focus fixup rule). Making
              // a Select busy-but-focusable the way Button does is a change to
              // this primitive's contract and is deliberately not made here.
              <Select
                hideLabel
                // Quiet: this picker sits beside the note title, and the note
                // is the subject of the row, not the control.
                variant="quiet"
                label={`File "${note.title}" to project`}
                value={null}
                placeholder={pending ? "Filing…" : "File to…"}
                options={options}
                disabled={pending}
                onChange={route}
              />
            )
          }
        />
        {error && (
          <StatusMessage variant="error" compact>
            Couldn&apos;t file this note: {error}
          </StatusMessage>
        )}
      </div>
    </li>
  );
}
