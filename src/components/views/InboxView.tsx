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
import { useFailedSessions } from "../../useSessions";
import { ListRow } from "../ui/ListRow";
import { Select, type SelectOption } from "../ui/Select";
import { StatusMessage } from "../ui/StatusMessage";
import { ViewFrame } from "../ui/ViewFrame";
import { NeedsAttentionSection } from "./NeedsAttentionSection";
import "./InboxView.css";

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
  // Owned here, not inside the section: the empty state below has to know
  // whether anything needs attention before it can claim nothing is waiting.
  const { sessions, error: sessionsError } = useFailedSessions();
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
    <ViewFrame eyebrow="Unfiled" title="Inbox">
      {/* Captured meetings that never became a note: they need an action
          (a retry), so they sit above the notes waiting to be filed. Renders
          nothing when there are none, which is the normal case. */}
      <NeedsAttentionSection sessions={sessions} error={sessionsError} />

      {error ? (
        <StatusMessage variant="error">Couldn&apos;t load the inbox: {error}</StatusMessage>
      ) : notes.length === 0 ? (
        // "Nothing waiting" has to mean the whole view, not just this list:
        // claiming it above a populated needs-attention section would tell
        // the user there is nothing to do while showing them something to do.
        !loading &&
        sessions.length === 0 &&
        !sessionsError && (
          <StatusMessage variant="empty">
            Nothing waiting. Notes the router can&apos;t place land here.
          </StatusMessage>
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
