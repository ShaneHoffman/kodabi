import { useRef, useState, type FormEvent, type ReactNode } from "react";
import { clsx } from "clsx";
import { useNavigation, type View } from "../../useNavigation";
import {
  createNote,
  INBOX_PROJECT,
  saveNote,
  todayIsoDate,
  useNote,
  type NoteDetail,
  type NoteType,
} from "../../useNotes";
import { folderHue, useProjects, type FolderHue } from "../../useProjects";
import { isSessionSource, useSessionArtifacts, type SessionArtifacts } from "../../useSessions";
import { applyMarkup, selectionAnchor } from "../../textareaCaret";
import { DeleteNoteDialog } from "../dialogs/DeleteNoteDialog";
import { Button } from "../ui/Button";
import { StatusMessage } from "../ui/StatusMessage";
import { NoteMarkdown } from "./NoteMarkdown";
import { PANEL_EYEBROW, SessionPanel, TranscriptTurns } from "./SessionPanel";

type Props = {
  noteId: string | null;
  project: string | null;
  /** Where the note was opened from, when the opener knew. See `backTarget`. */
  origin?: View;
};

/**
 * The note screen — the app's READING ROOM, and the composing mode that
 * shares its measure. All writes go through the backend's Markdown writer, so
 * the on-disk frontmatter stays schema-valid; on edit the backend preserves
 * `id`, `source`, and routing verbatim.
 */
export function NoteEditorView({ noteId, project, origin }: Props) {
  if (noteId === null) {
    return <CreateNote initialProject={project} />;
  }
  if (project === null) {
    // Unreachable via current navigation: every opened note arrives with its
    // project. A quiet dead-end beats a crash if a future caller slips.
    return (
      <NoteFrame label="Note">
        <StatusMessage variant="empty">
          This note arrived without its project. Open it from a project list.
        </StatusMessage>
      </NoteFrame>
    );
  }
  return <OpenedNote key={noteId} noteId={noteId} project={project} origin={origin} />;
}

/**
 * The note screen's own frame.
 *
 * This view used to render into `ViewFrame variant="doc"`, which capped its
 * column at a single measure — the right answer while the note WAS one column,
 * and impossible once prose at 66ch has a 272px rail beside it. What ViewFrame
 * contributed otherwise was the landmark and the view gutter, and both are two
 * lines here. The landmark even gains a name it never had: `doc` drew its
 * `<section>` with no `title` or `eyebrow` to label it, a documented gap in
 * ViewFrame itself.
 *
 * `@container`, not a media query: what decides whether the rail fits is the
 * width of the main panel — which the dock's presence changes without the
 * window resizing at all.
 */
function NoteFrame({ label, children }: { label: string; children: ReactNode }) {
  return (
    <section
      aria-label={label}
      className="@container flex min-h-full flex-col px-10 py-[34px]"
    >
      {children}
    </section>
  );
}

/**
 * The reading room's two columns: prose at its measure, and whatever width is
 * left over as a details rail (docs/DESIGN_SYSTEM.md §1 — the leftover becomes
 * a rail, never longer lines).
 *
 * Below the threshold there is no leftover to become anything, so the rail
 * stops being a rail and stacks under the note. 42rem is the width at which the
 * body still holds a readable line once 272px and the gap are taken out of it;
 * a rail that appears before then is one that took its width from the prose.
 */
const NOTE_GRID =
  "mt-8 grid gap-8 @[42rem]:grid-cols-[minmax(0,1fr)_272px] @[42rem]:gap-11";

/** The reading measure. Prose stops here; structure does not. */
const NOTE_COLUMN = "max-w-[66ch] font-note text-note text-ink-read";

/** The note's title, in read and compose alike — they draw the same document
 * and must not disagree about what it looks like. */
const NOTE_TITLE =
  "max-w-[30ch] text-balance font-ui text-[26px] font-semibold leading-[1.15] tracking-[-0.01em] text-ink";

/**
 * Where "back" goes from a note, and what it is called.
 *
 * The origin wins when there is one: a note opened from a search belongs, on
 * the way out, to those results — not to the folder it happens to live in.
 * Without one (the palette's New note, the note just created) the filing
 * location is still the true answer, and an unfiled note came from the Inbox,
 * not from a `project` view for the Inbox sentinel, which is a dead end.
 *
 * A `noteEditor` origin falls through to the same fallback deliberately: notes
 * do not open notes today, and if they ever do, "back" must not become a chain
 * that walks a reader through their own history one screen at a time.
 */
function backTarget(project: string, origin?: View): { view: View; label: string } {
  switch (origin?.kind) {
    case "inbox":
      return { view: origin, label: "Inbox" };
    case "project":
      return { view: origin, label: origin.slug };
    case "search":
      return { view: origin, label: "Search" };
    case "chat":
      return { view: origin, label: "Chat" };
    default:
      return project === INBOX_PROJECT
        ? { view: { kind: "inbox" }, label: "Inbox" }
        : { view: { kind: "project", slug: project }, label: project };
  }
}

/** The quiet way out, at the top of every note. */
function BackLink({ project, origin }: { project: string; origin?: View }) {
  const { navigate } = useNavigation();
  const { view, label } = backTarget(project, origin);
  return (
    <Button variant="quiet" onClick={() => navigate(view)} className="-ml-4 self-start">
      <span aria-hidden="true">←</span>
      <span>{label}</span>
    </Button>
  );
}

function OpenedNote({
  noteId,
  project,
  origin,
}: {
  noteId: string;
  project: string;
  origin?: View;
}) {
  const { note, error, setNote } = useNote(project, noteId);
  const [editing, setEditing] = useState(false);

  if (error) {
    return (
      <NoteFrame label="Note">
        <StatusMessage variant="error">Couldn&apos;t open this note: {error}</StatusMessage>
      </NoteFrame>
    );
  }
  if (!note) {
    // Quiet loading: the page simply appears when ready.
    return null;
  }

  return editing ? (
    <EditNote
      note={note}
      project={project}
      onDone={(saved) => {
        // The editor shows the save's echo immediately; other windows' lists
        // refresh because the save_note command broadcasts `vault:changed`.
        if (saved) setNote(saved);
        setEditing(false);
      }}
    />
  ) : (
    <ReadNote
      note={note}
      project={project}
      origin={origin}
      onEdit={() => setEditing(true)}
    />
  );
}

/**
 * Read mode. The one screen in the app with no list on it, and the only one
 * whose type is sized for reading rather than for scanning.
 *
 * The source pairing decides the shape of the tree, not just what renders: a
 * keyword source (manual, quick-capture) names no artifact, so the fetch must
 * not exist for those notes at all. A hook cannot be called conditionally, so
 * the branch happens here — one wrapper that fetches, one that does not, and a
 * single layout underneath that takes what it is given.
 */
function ReadNote({
  note,
  project,
  origin,
  onEdit,
}: {
  note: NoteDetail;
  project: string;
  origin?: View;
  onEdit: () => void;
}) {
  const shared = { note, project, origin, onEdit };
  return isSessionSource(note.source) ? (
    <SessionReadNote {...shared} />
  ) : (
    <ReadNoteLayout {...shared} session={null} />
  );
}

type ReadProps = {
  note: NoteDetail;
  project: string;
  origin?: View;
  onEdit: () => void;
};

/** A note distilled from a captured session, and the only caller that reads
 * the artifacts behind its `source:` field. */
function SessionReadNote(props: ReadProps) {
  const { artifacts, error } = useSessionArtifacts(props.note.source);
  return <ReadNoteLayout {...props} session={{ artifacts, error }} />;
}

/**
 * The chrome is four places and no more: the way out on its own line, two quiet
 * text buttons beside the title, and — on a note distilled from a session — the
 * rail's chips, one per artifact.
 *
 * Against docs/UI_CONVENTIONS.md, *Composition*: the back link sits outside the
 * count (getting around is not acting); Edit and Delete are the view-owned
 * header's ONE cluster; each chip is one disclosure over one subordinate
 * section, and neither nests inside the other. What this must never grow is a
 * second cluster or a toggle inside a toggle — the drift a count is for, since
 * a reader should not have to sweep a document to learn what they can do to it.
 */
function ReadNoteLayout({
  note,
  project,
  origin,
  onEdit,
  session,
}: ReadProps & {
  session: { artifacts: SessionArtifacts | null; error: string | null } | null;
}) {
  const { navigate } = useNavigation();
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [transcriptOpen, setTranscriptOpen] = useState(false);

  const segments = session?.artifacts?.transcript_available
    ? session.artifacts.segments
    : null;

  return (
    <NoteFrame label={note.title}>
      {/* The one signal that read mode has rendered. The E2E harness needs a
          deterministic wait for "the note screen is up" that is not any of the
          things the source-pairing slice is about to assert on, or the wait
          would beg the question. */}
      <article data-testid="note-read">
        <BackLink project={project} origin={origin} />

        <header className="mt-3.5 flex items-start justify-between gap-6">
          <h2 className={NOTE_TITLE}>{note.title}</h2>
          <div className="flex flex-none items-center gap-1">
            <Button variant="quiet" onClick={onEdit}>
              Edit
            </Button>
            <Button
              variant="quiet"
              aria-haspopup="dialog"
              onClick={() => setConfirmingDelete(true)}
            >
              Delete note
            </Button>
          </div>
        </header>

        <div className={NOTE_GRID}>
          <div className="min-w-0">
            {note.body_markdown ? (
              <div className={NOTE_COLUMN}>
                <NoteMarkdown body={note.body_markdown} />
              </div>
            ) : (
              <StatusMessage variant="empty">
                This note has no body yet. Choose Edit to write one.
              </StatusMessage>
            )}

            {/* The turns land here rather than in the rail because a turn is a
                sentence and the rail is 272px wide. */}
            {transcriptOpen && segments !== null && <TranscriptTurns segments={segments} />}
          </div>

          <aside aria-label="Note details" className="flex flex-col gap-3.5 self-start">
            <DetailsPanel note={note} project={project} />
            {session !== null && (
              <SessionPanel
                artifacts={session.artifacts}
                error={session.error}
                transcriptOpen={transcriptOpen}
                onToggleTranscript={() => setTranscriptOpen((open) => !open)}
              />
            )}
          </aside>
        </div>

        {confirmingDelete && (
          <DeleteNoteDialog
            id={note.id}
            noteTitle={note.title}
            sessionBacked={isSessionSource(note.source)}
            onClose={() => setConfirmingDelete(false)}
            // Navigate away before the delete's `vault:changed` refetch can run:
            // reading the just-deleted note would otherwise flash the not-found
            // state. The note's "back" and its "after-delete" are the same place.
            onDeleted={() => navigate(backTarget(project, origin).view)}
          />
        )}
      </article>
    </NoteFrame>
  );
}

/** Spelled out per hue, never built from a template: Tailwind reads source
 * text, so a constructed class name is a class that does not ship. */
const HUE_DOT: Record<FolderHue, string> = {
  coral: "bg-coral",
  cobalt: "bg-cobalt",
  teal: "bg-teal",
  plum: "bg-plum",
};

/**
 * What the note is, beside what it says.
 *
 * This replaces a single `·`-joined meta line under the title — which is where
 * a note's date, kind and tags used to go, all at metadata weight, all fighting
 * the title for the one line below it. In a rail each fact gets its own row and
 * its own name, and the two that were never shown at all (which folder the note
 * is filed in, and its id) fit without crowding anything.
 *
 * Compose mode drops the Tags row, because compose mode has the tags themselves:
 * a chip you can remove sits two lines up, and a second copy of the same list
 * would both duplicate it and go stale the moment one was added — this reads the
 * SAVED note, which is exactly what an editor's tags are not.
 */
function DetailsPanel({
  note,
  project,
  showTags = true,
}: {
  note: NoteDetail;
  project: string;
  showTags?: boolean;
}) {
  const unfiled = project === INBOX_PROJECT;
  return (
    <section className="glass-card px-4 py-3.5">
      <p className={PANEL_EYEBROW}>Details</p>
      <dl className="mt-1">
        <Row label="Filed in">
          {unfiled ? (
            "Inbox"
          ) : (
            <>
              <span
                aria-hidden="true"
                className={clsx("size-2 flex-none rounded-[2px]", HUE_DOT[folderHue(project)])}
              />
              <span className="truncate">{project}</span>
            </>
          )}
        </Row>
        <Row label="Kind">{note.type}</Row>
        <Row label="Captured">
          {/* The date only: a frontmatter `date` is either a local calendar day
              or an offset-bearing timestamp (docs/FRONTMATTER_SCHEMA.md), and
              the day is the part a reader is looking for. */}
          <span className="font-data text-[11px] tabular-nums">{note.date.slice(0, 10)}</span>
        </Row>
        <Row label="Note id">
          <span className="font-data text-[11px]">{note.id}</span>
        </Row>
        {showTags && note.tags.length > 0 && (
          <Row label="Tags">
            <span className="truncate font-data text-[11px]">{note.tags.join(" · ")}</span>
          </Row>
        )}
      </dl>
    </section>
  );
}

/** One hairline key/value row. The rule goes on top and the first row drops it,
 * so the panel's eyebrow is not underlined by its own first fact. */
function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-3 border-t border-edge py-2 first:border-t-0">
      <dt className="flex-none text-[12px] text-ink-faint">{label}</dt>
      <dd className="flex min-w-0 items-center gap-[7px] text-right text-[12px] text-ink-dim">
        {children}
      </dd>
    </div>
  );
}

/** One button in the floating format toolbar. */
function Tool({
  label,
  name,
  onApply,
  className = "",
}: {
  label: ReactNode;
  /** The spoken name. The visible label is a single letter for three of these
   * ("B", "I", "H"), which a screen reader reads out as a letter. */
  name: string;
  onApply: () => void;
  className?: string;
}) {
  return (
    <button
      type="button"
      aria-label={name}
      // pointerdown, not click: clicking would blur the textarea first, which
      // collapses the very selection the button is about to act on.
      onPointerDown={(event) => {
        event.preventDefault();
        onApply();
      }}
      // ...and click as well, but ONLY for a keyboard activation. Enter and
      // Space fire `click` with no preceding pointer event, and those carry
      // `detail === 0`; a real pointer click carries the click count, so this
      // cannot double-apply on top of the handler above. Without it the whole
      // toolbar was pointer-only, which docs/DESIGN_SYSTEM.md §6 forbids
      // outright ("every mouse flow has a keyboard flow, and the reverse").
      onClick={(event) => {
        if (event.detail === 0) onApply();
      }}
      className={clsx(
        "focus-ring-inset rounded-[6px] px-2 py-1 font-ui text-[12px] leading-none text-ink",
        "transition-colors duration-140 ease-out-strong hover:bg-wash active:bg-wash-hover",
        className,
      )}
    >
      {label}
    </button>
  );
}

/**
 * Compose mode. The same measure, the same serif, the same left edge as read
 * mode — you write in the ramp you read in. What changes is the chrome, and
 * it has to work harder precisely because the document did not change: a mono
 * EDITING eyebrow, a save state, a filled Done button, tag chips you can
 * remove, and the reserved green on the caret in the body.
 *
 * The rail comes along, carrying Details and only Details. Session stays behind
 * in read mode: its chips open things to read, and compose mode is not where
 * you read them — nor is it where the artifact fetch belongs.
 *
 * The format toolbar is chrome on demand: it exists only while text is
 * selected, anchored just above the selection rather than parked in a bar at
 * the top of the screen.
 */

/** How far above the selection the toolbar's tail sits, in px.
 *
 * A screen coordinate rather than a design value: it is added to a caret
 * position measured at runtime, so it is spelled here rather than in a token,
 * which the component cannot read numerically. It is the tail's own 8px square
 * (`size-2`) plus the 4px it is pulled back by (`-translate-y-[4px]`) minus
 * one, so the point of the tail lands on the selection rather than overlapping
 * it. Change either utility on the tail and change this. */
const TOOLBAR_LIFT = 11;

/** The tag chip and the affordance that adds one, at the same size and the same
 * box — the ghost is dashed and the chip is solid, and nothing else differs, so
 * committing a tag does not move the row. */
const TAG_BOX = "rounded-[8px] border px-2 py-1 font-data text-[11px]";

function EditNote({
  note,
  project,
  onDone,
}: {
  note: NoteDetail;
  project: string;
  onDone: (saved: NoteDetail | null) => void;
}) {
  const [title, setTitle] = useState(note.title);
  const [body, setBody] = useState(note.body_markdown);
  const [tags, setTags] = useState<string[]>(note.tags);
  const [addingTag, setAddingTag] = useState(false);
  const [draftTag, setDraftTag] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [anchor, setAnchor] = useState<{ left: number; top: number } | null>(null);
  const bodyRef = useRef<HTMLTextAreaElement>(null);

  // Recomputed from the control itself on every interaction that could change
  // a selection. In an event handler, not an effect: it happens because the
  // user did something (.claude/rules/no-use-effect.md).
  const syncToolbar = () => {
    const textarea = bodyRef.current;
    setAnchor(textarea ? selectionAnchor(textarea) : null);
  };

  // Blank reads as absence, not as a value: the placeholder says "Untitled",
  // and clearing the field would otherwise strip the frontmatter key and snap
  // the display to a lowercased de-slugged filename. So an empty box means
  // "unchanged", and only a real edit is sent.
  const trimmedTitle = title.trim();
  const titleDirty = trimmedTitle !== "" && trimmedTitle !== note.title;

  // Derived, never tracked. A `saved` flag could only ever be false here — a
  // successful save unmounts this component — so the line it fed claimed
  // "Unsaved changes" over a note nobody had touched yet.
  const dirty =
    titleDirty ||
    body !== note.body_markdown ||
    tags.length !== note.tags.length ||
    tags.some((tag, index) => tag !== note.tags[index]);

  const format = (markup: { wrap?: string; prefix?: string; link?: true }) => {
    const textarea = bodyRef.current;
    if (!textarea) return;
    const next = applyMarkup(
      textarea.value,
      textarea.selectionStart,
      textarea.selectionEnd,
      markup,
    );
    setBody(next.value);
    // Restore the selection after React lands the new value, so a second
    // format acts on the same words rather than on a collapsed caret.
    queueMicrotask(() => {
      textarea.focus();
      textarea.setSelectionRange(next.start, next.end);
      syncToolbar();
    });
  };

  const commitTag = () => {
    const tag = draftTag.trim().toLowerCase();
    // Normalization proper lives in the backend's `Tag::parse_normalized` —
    // the one authority, so every entry point folds identically. This only
    // stops an obvious duplicate before the round trip.
    if (tag && !tags.includes(tag)) setTags([...tags, tag]);
    setDraftTag("");
    setAddingTag(false);
  };

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (saving) return;
    setSaving(true);
    setError(null);
    saveNote({
      id: note.id,
      project,
      type: note.type,
      date: note.date,
      tags,
      body,
      title: titleDirty ? trimmedTitle : null,
    })
      .then((result) => onDone(result))
      .catch((err: unknown) => {
        setSaving(false);
        setError(String(err));
      });
  };

  return (
    <NoteFrame label={`Editing ${note.title}`}>
      <form onSubmit={submit}>
        {/* The mode bar. It is the whole reason compose mode is legible: the
            document below it is identical to read mode, so this line is what
            says which one you are in. */}
        <header className="flex items-center justify-between gap-6">
          <p className={PANEL_EYEBROW}>Editing</p>
          <div className="flex items-center gap-1">
            {/* Says something only when there is something to say. "No
                changes" was the resting label on every freshly opened editor:
                an answer to a question nobody had asked yet, occupying the
                one line that is supposed to mean "you have unsaved work". */}
            <span className="font-data text-[11px] text-ink-faint">
              {saving ? "Saving…" : dirty ? "Unsaved changes" : ""}
            </span>
            {/* The way out that does not write. Without it the only exit from
                compose mode was Done, so changing your mind still rewrote the
                file on disk. */}
            <Button variant="quiet" onClick={() => onDone(null)} disabled={saving}>
              Cancel
            </Button>
            <Button type="submit" loading={saving} loadingLabel="Saving…">
              Done
            </Button>
          </div>
        </header>

        {/* The title edits in place; the filename does not. Renaming the file
            would break the note↔source pairing (a distilled note finds its
            recording and transcript by filename stem) and every link pointing
            at the old path — the slug rules apply once, at creation, and
            frontmatter `title` carries the full editable string from then on.
            Sent only when actually changed: `note.title` is the *effective*
            title (the de-slugged filename when the key is absent), so echoing
            it back verbatim would materialize that fallback into the file. */}
        {/* <header>, matching read mode. Compose and read draw the same
            region of the same document and used to disagree about what it
            was: read mode said <header>, edit and create said <div>. */}
        <header className="mt-3.5 flex items-center">
          <input
            value={title}
            onChange={(event) => setTitle(event.target.value)}
            placeholder="Untitled"
            aria-label="Title"
            className={clsx(
              NOTE_TITLE,
              "focus-ring w-full border-none bg-transparent p-0 caret-kodama placeholder:text-ink-faint",
            )}
          />
        </header>

        {/* The date sits OUTSIDE the tags list: it is not a tag, and folding
            it in as the first <li> made a screen reader announce the note's
            date as a member of "Tags" and count one item too many. It is a
            sibling caption; the <ul> holds only the tags and the affordance
            that adds one. */}
        <div className="mt-2 flex flex-wrap items-center gap-1.5">
          <span className="mr-1 font-data text-[11px] tabular-nums text-ink-faint">
            {note.date}
          </span>
          {/* A real list. The tags were a <div> of <span>s, so nothing
              announced that there were tags, or how many, or where the row
              ended. */}
          <ul className="flex flex-wrap items-center gap-1.5" aria-label="Tags">
            {tags.map((tag) => (
              <li
                key={tag}
                className={clsx(
                  TAG_BOX,
                  "inline-flex items-center gap-1.5 border-edge bg-wash text-ink-dim",
                  "shadow-[inset_0_1px_0_var(--color-edge-lit)]",
                )}
              >
                {tag}
                <button
                  type="button"
                  aria-label={`Remove tag ${tag}`}
                  onClick={() => setTags(tags.filter((each) => each !== tag))}
                  className="focus-ring text-ink-faint transition-colors duration-140 ease-out-strong hover:text-ink"
                >
                  ×
                </button>
              </li>
            ))}
            <li>
              {addingTag ? (
                <input
                  autoFocus
                  value={draftTag}
                  onChange={(event) => setDraftTag(event.target.value)}
                  onBlur={commitTag}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") {
                      event.preventDefault();
                      commitTag();
                    } else if (event.key === "Escape") {
                      setDraftTag("");
                      setAddingTag(false);
                    }
                  }}
                  aria-label="New tag"
                  className={clsx(
                    TAG_BOX,
                    "focus-ring w-24 border-dashed border-edge bg-transparent text-ink caret-kodama",
                  )}
                />
              ) : (
                <button
                  type="button"
                  onClick={() => setAddingTag(true)}
                  className={clsx(
                    TAG_BOX,
                    "focus-ring border-dashed border-edge text-ink-faint",
                    "transition-colors duration-140 ease-out-strong hover:border-ink-faint hover:text-ink",
                  )}
                >
                  + tag
                </button>
              )}
            </li>
          </ul>
        </div>

        <div className={NOTE_GRID}>
          <div className="relative min-w-0">
            <textarea
              ref={bodyRef}
              value={body}
              aria-label="Note body"
              onChange={(event) => {
                setBody(event.target.value);
                syncToolbar();
              }}
              onSelect={syncToolbar}
              onKeyUp={syncToolbar}
              onPointerUp={syncToolbar}
              // Tearing the toolbar down on ANY blur made it unreachable by
              // keyboard: Tab moved focus toward it and unmounted it on the way.
              // A textarea keeps selectionStart/End while blurred, so holding
              // the anchor open while focus is inside the toolbar keeps the
              // selection the tools act on intact.
              onBlur={(event) => {
                const next = event.relatedTarget;
                if (next instanceof HTMLElement && next.closest('[role="toolbar"]')) return;
                setAnchor(null);
              }}
              className={clsx(
                NOTE_COLUMN,
                "focus-ring min-h-[300px] w-full resize-none border-none bg-transparent p-0",
                "caret-kodama placeholder:text-ink-faint",
              )}
            />
            {anchor && (
              <div
                role="toolbar"
                aria-label="Format selection"
                className="glass-overlay absolute z-30 flex -translate-x-1/2 -translate-y-full items-center gap-0.5 whitespace-nowrap p-1"
                style={{ left: anchor.left, top: anchor.top - TOOLBAR_LIFT }}
              >
                <Tool
                  label="B"
                  name="Bold"
                  onApply={() => format({ wrap: "**" })}
                  className="font-bold"
                />
                <Tool
                  label="I"
                  name="Italic"
                  onApply={() => format({ wrap: "*" })}
                  className="italic"
                />
                <Tool
                  label="H"
                  name="Heading"
                  onApply={() => format({ prefix: "## " })}
                  className="font-semibold"
                />
                <span className="mx-0.5 h-[18px] w-px bg-edge" />
                <Tool label="List" name="Bullet list" onApply={() => format({ prefix: "- " })} />
                <Tool label="Link" name="Link" onApply={() => format({ link: true })} />
                {/* The point. Its fill is the panel's, which is why the utility
                    exists at all (src/index.css §3). */}
                <span
                  aria-hidden="true"
                  className="glass-overlay-tail absolute top-full left-1/2 size-2 -translate-x-1/2 -translate-y-[4px] rotate-45"
                />
              </div>
            )}
          </div>

          <aside aria-label="Note details" className="flex flex-col gap-3.5 self-start">
            <DetailsPanel note={note} project={project} showTags={false} />
          </aside>
        </div>

        {error && (
          <StatusMessage variant="error">Couldn&apos;t save this note: {error}</StatusMessage>
        )}
      </form>
    </NoteFrame>
  );
}

/**
 * A note that does not exist yet. It borrows compose mode's shape rather than
 * inventing a third one: the same measure, the same title, the same filled
 * commit button — the only additions are the two things a new note has no
 * answer for, its project and its name.
 *
 * No rail: it would have nothing to say. The note has no id yet, no captured
 * date on disk, and the one fact it does have — where it will be filed — is
 * being typed two lines up rather than reported.
 */
function CreateNote({ initialProject }: { initialProject: string | null }) {
  const { navigate } = useNavigation();
  const { entries } = useProjects();
  const [project, setProject] = useState(initialProject ?? "");
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const projectSlugs = entries.flatMap((entry) =>
    entry.kind === "project" ? [entry.project.slug] : [],
  );
  const trimmedProject = project.trim();

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!trimmedProject || submitting) return;
    setSubmitting(true);
    setError(null);
    createNote({
      type: "note" satisfies NoteType,
      project: trimmedProject,
      date: todayIsoDate(),
      tags: [],
      source: "manual",
      body,
      title: title.trim() || null,
    })
      .then((created) => {
        // Land in the read view via read_note — the round trip through the
        // on-disk file is the proof the note was written schema-valid. The
        // echoed project is the backend-canonicalized casing, which may
        // differ from what was typed. Every window's lists refresh because the
        // write_note command broadcasts `vault:changed`.
        navigate({
          kind: "noteEditor",
          noteId: created.id,
          project: created.project ?? trimmedProject,
        });
      })
      .catch((err: unknown) => {
        setSubmitting(false);
        setError(String(err));
      });
  };

  return (
    <NoteFrame label="New note">
      <form onSubmit={submit} className={NOTE_COLUMN}>
        <header className="flex items-center justify-between gap-6">
          <p className={PANEL_EYEBROW}>New note</p>
          <Button
            type="submit"
            disabled={!trimmedProject}
            loading={submitting}
            loadingLabel="Creating…"
          >
            Create
          </Button>
        </header>

        <div className="mt-3.5 flex items-center">
          <input
            autoFocus
            value={title}
            onChange={(event) => setTitle(event.target.value)}
            placeholder="Untitled"
            aria-label="Title"
            className={clsx(
              NOTE_TITLE,
              "focus-ring w-full border-none bg-transparent p-0 caret-kodama placeholder:text-ink-faint",
            )}
          />
        </div>

        <div className="mt-2 flex items-center gap-1.5">
          <span className="font-data text-[11px] tabular-nums text-ink-faint">
            {todayIsoDate()}
          </span>
          {/* Free text with suggestions: an unknown name creates the project
              folder on save, which is how a project comes into existence at
              all. A picker would make the vault a closed set. */}
          <input
            list="kodabi-project-slugs"
            value={project}
            onChange={(event) => setProject(event.target.value)}
            placeholder="project"
            aria-label="Project"
            className={clsx(
              TAG_BOX,
              "focus-ring border-dashed border-edge bg-transparent text-ink caret-kodama placeholder:text-ink-faint",
            )}
          />
          <datalist id="kodabi-project-slugs">
            {projectSlugs.map((slug) => (
              <option key={slug} value={slug} />
            ))}
          </datalist>
        </div>

        <textarea
          value={body}
          aria-label="Note body"
          placeholder="Write here"
          onChange={(event) => setBody(event.target.value)}
          className={clsx(
            NOTE_COLUMN,
            "focus-ring mt-8 min-h-[300px] w-full resize-none border-none bg-transparent p-0",
            "caret-kodama placeholder:text-ink-faint",
          )}
        />

        {error && (
          <StatusMessage variant="error">Couldn&apos;t create this note: {error}</StatusMessage>
        )}
      </form>
    </NoteFrame>
  );
}
