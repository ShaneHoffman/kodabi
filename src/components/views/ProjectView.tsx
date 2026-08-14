import { Fragment, useState, type CSSProperties } from "react";
import { useNavigation } from "../../useNavigation";
import { groupNotes } from "../../noteGroups";
import { projectRowMeta } from "../../noteMeta";
import { todayIsoDate, useProjectNotes } from "../../useNotes";
import { folderHue, useProjects, type FolderHue } from "../../useProjects";
import { DeleteProjectDialog } from "../dialogs/DeleteProjectDialog";
import { RenameProjectDialog } from "../dialogs/RenameProjectDialog";
import { Button } from "../ui/Button";
import { Menu } from "../ui/Menu";
import { StatusMessage } from "../ui/StatusMessage";
import { ViewFrame } from "../ui/ViewFrame";

type Props = {
  slug: string;
};

/** Spelled out per hue, never built from a template: Tailwind reads source
 * text, so a constructed class name is a class that does not ship. */
const HUE_DOT: Record<FolderHue, string> = {
  coral: "bg-coral",
  cobalt: "bg-cobalt",
  teal: "bg-teal",
  plum: "bg-plum",
};

/**
 * A row in the index. THE ROWS DO NOT LIFT — no plane, no ring, no shadow, one
 * wash and nothing else. A card lifts because it is waiting on you; you are
 * already inside this folder, and nothing in it is. That difference is the
 * whole reason the Inbox and a project look unlike each other from across the
 * room (docs/DESIGN_SYSTEM.md §1).
 *
 * The folder's hue is absent for the same reason: it is spent once, on the dot
 * in the header. Repeating it down the rows would colour the calm out of them.
 */
const PROJECT_ROW = [
  "focus-ring-inset block w-full rounded-button px-2.5 py-3 text-left",
  "transition-[background-color] duration-140 ease-out-strong hover:bg-wash",
].join(" ");

/** The eyebrow over each run of rows. The view's ONE level of eyebrow — the
 * header deliberately carries none, because two levels of the same small
 * uppercase voice on one surface is one too many (DESIGN_SYSTEM §1). */
const GROUP_LABEL = [
  "font-data text-[10px] uppercase tracking-[0.22em] text-ink-faint",
  "mt-6.5 mb-1.5 px-2.5 first:mt-4.5",
].join(" ");

/** The entrance, on every label and every row. `animate-rise-in` is 280ms of
 * 8px, and its reduced partner drops the movement and keeps the fade. */
const RISES_IN = "animate-rise-in motion-reduce:animate-fade-in";

/** The stagger, and its cap. Past about five steps a cascade stops reading as
 * one gesture and starts reading as lag, so the sixth item onward rises with
 * the fifth rather than trailing further behind it (DESIGN_SYSTEM §4). Inline
 * because the delay is data — the item's position — not a design step. */
const STAGGER_STEP_MS = 45;
const STAGGER_CAP = 5;
function staggerStyle(position: number): CSSProperties | undefined {
  return position === 0
    ? undefined
    : { animationDelay: `${Math.min(position, STAGGER_CAP - 1) * STAGGER_STEP_MS}ms` };
}

/**
 * A project's notes, newest first — the app's LIBRARY, and deliberately the
 * opposite stance from the Inbox in every way that can be seen from across the
 * room: a quiet index of full-width rows that only wash, rather than a stack of
 * cards on coloured edges that lift.
 *
 * The header is where the folder is identified: its hue as a dot, its display
 * name at the view step, and the count beside the slug in the data face, so the
 * mono-path vocabulary the filing menu and the search breadcrumbs use still
 * resolves to a string on this screen. The rows below it carry no hue at all.
 *
 * Nothing here is waiting on you. The layout is the thing that says so.
 */
export function ProjectView({ slug }: Props) {
  const { navigate } = useNavigation();
  const { notes, loading, error } = useProjectNotes(slug);
  const { entries } = useProjects();
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [renaming, setRenaming] = useState(false);

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

  // The listing has not landed on a cold start, so fall back to the slug's last
  // segment rather than flashing the full path and then swapping it out.
  const displayName = own?.project.display_name ?? slug.slice(slug.lastIndexOf("/") + 1);
  const groups = groupNotes(notes, todayIsoDate());

  return (
    <ViewFrame
      variant="library"
      title={
        <span className="flex items-center gap-3">
          {/* The one place the folder's hue is spent. Decorative: the name
              beside it says the same thing in words, and a colour is not a
              label a screen reader can read out. */}
          <span
            aria-hidden
            className={`size-3 flex-none rounded-[4px] ${HUE_DOT[folderHue(slug)]}`}
          />
          {displayName}
        </span>
      }
      // The composed title above names no landmark on its own — `label` is what
      // keeps the section a named region.
      label={displayName}
      summary={
        notes.length > 0
          ? `${notes.length} ${notes.length === 1 ? "note" : "notes"} · ${slug}`
          : // At zero the count's voice yields to the empty state (two "nothing
            // here" sentences in one view is one too many), but the identity
            // stays: the slug is how this folder is addressed everywhere else.
            slug
      }
      action={
        // Ink, not the reserved green: the verbs on a reading screen earn
        // their place by being quiet text, not by being a colour. Creation
        // leads and stays a button, because it is the thing you came here to
        // do.
        //
        // EVERYTHING ELSE THIS FOLDER CAN DO GOES IN THE MENU. The header slot
        // holds one action, and the glossary made three — so rather than grow
        // a toolbar the frame's doc forbids, the project's other verbs collapse
        // behind one trigger. Rename is exactly the next project-scoped verb
        // that home was built for.
        <div className="flex items-center gap-2">
          <Button
            variant="quiet"
            onClick={() => navigate({ kind: "noteEditor", noteId: null, project: slug })}
          >
            New note
          </Button>
          <Menu.Root>
            <Menu.Trigger
              render={
                <Button variant="quiet" aria-label={`${displayName} project actions`}>
                  Project
                  <span aria-hidden="true" className="text-[10px] text-ink-faint">
                    ▾
                  </span>
                </Button>
              }
            />
            {/* `end`: the trigger sits at the right edge of the header, so the
                menu hangs back over the view rather than off the panel. */}
            <Menu.Content align="end">
              <Menu.Item onClick={() => navigate({ kind: "glossary", slug })}>
                Glossary
              </Menu.Item>
              {/* Rename and delete both need more from the user before they
                  take effect (a name, a confirmation), which the ellipsis
                  promises on either. */}
              <Menu.Item onClick={() => setRenaming(true)}>Rename…</Menu.Item>
              <Menu.Item onClick={() => setConfirmingDelete(true)}>Delete project…</Menu.Item>
            </Menu.Content>
          </Menu.Root>
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
        // Keyed by slug so walking between folders REMOUNTS the index: without
        // it React reconciles one folder's rows into the next one's and the
        // entrance never replays, which reads as the view having not changed.
        <div key={slug} className="mt-2.5">
          {groups.map((group, groupIndex) => (
            <Fragment key={group.key}>
              <p
                className={`${GROUP_LABEL} ${RISES_IN}`}
                style={staggerStyle(staggerPosition(groups, groupIndex, -1))}
              >
                {group.label}
              </p>
              <ul>
                {group.notes.map((note, noteIndex) => (
                  // Keyed by path, not id: two files can carry the same id (an
                  // external copy), and duplicate keys would mis-reconcile rows.
                  <li
                    key={note.path}
                    className={RISES_IN}
                    style={staggerStyle(staggerPosition(groups, groupIndex, noteIndex))}
                  >
                    <button
                      type="button"
                      className={PROJECT_ROW}
                      onClick={() =>
                        navigate({
                          kind: "noteEditor",
                          noteId: note.id,
                          project: slug,
                          origin: { kind: "project", slug },
                        })
                      }
                    >
                      {/* Only phrasing content inside: the title, meta and
                          snippet are block <span>s, because a button's content
                          model allows nothing more and the whole row is the
                          button (the same shape InboxView's row settled on). */}
                      <span className="block text-[14.5px] font-semibold text-ink">
                        {note.title}
                      </span>
                      <span className="mt-1 block font-data text-[10.5px] text-ink-faint tabular-nums">
                        {projectRowMeta(note)}
                      </span>
                      {/* A note can have no body to preview, and an empty line
                          would still take its 4px and its height. */}
                      {note.snippet && (
                        <span className="mt-1 block truncate text-[12.5px] text-ink-dim">
                          {note.snippet}
                        </span>
                      )}
                    </button>
                  </li>
                ))}
              </ul>
            </Fragment>
          ))}
        </div>
      )}

      {renaming && <RenameProjectDialog slug={slug} onClose={() => setRenaming(false)} />}

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

/**
 * Where an item falls in the single top-to-bottom sequence the stagger counts.
 *
 * The cascade is one gesture over the whole index, not one per group: the
 * labels are items in it too, so a second group's label does not restart the
 * count and arrive alongside the first group's first row. `noteIndex` is -1 for
 * a label, which is what puts it immediately above the rows it heads.
 */
function staggerPosition(
  groups: { notes: unknown[] }[],
  groupIndex: number,
  noteIndex: number,
): number {
  let position = 0;
  for (let index = 0; index < groupIndex; index += 1) {
    position += 1 + groups[index].notes.length;
  }
  return position + 1 + noteIndex;
}
