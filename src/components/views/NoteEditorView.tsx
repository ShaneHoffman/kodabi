import { useRef, useState, type FormEvent, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { useNavigation, type View } from "../../useNavigation";
import { noteMeta } from "../../noteMeta";
import {
  createNote,
  INBOX_PROJECT,
  saveNote,
  todayIsoDate,
  useNote,
  type NoteDetail,
  type NoteType,
} from "../../useNotes";
import { useProjects } from "../../useProjects";
import { applyMarkup, selectionAnchor } from "../../textareaCaret";
import { Button } from "../ui/Button";
import { StatusMessage } from "../ui/StatusMessage";
import { ViewFrame } from "../ui/ViewFrame";
import "./NoteEditorView.css";

type Props = {
  noteId: string | null;
  project: string | null;
};

/**
 * The note screen — the app's READING ROOM, and the composing mode that
 * shares its measure. All writes go through the backend's Markdown writer, so
 * the on-disk frontmatter stays schema-valid; on edit the backend preserves
 * `id`, `source`, and routing verbatim.
 */
export function NoteEditorView({ noteId, project }: Props) {
  if (noteId === null) {
    return <CreateNote initialProject={project} />;
  }
  if (project === null) {
    // Unreachable via current navigation: every opened note arrives with its
    // project. A quiet dead-end beats a crash if a future caller slips.
    return (
      <ViewFrame variant="doc">
        <StatusMessage variant="empty">
          This note arrived without its project. Open it from a project list.
        </StatusMessage>
      </ViewFrame>
    );
  }
  return <OpenedNote key={noteId} noteId={noteId} project={project} />;
}

/** Where "back" goes from a note, and what it is called. An unfiled note came
 * from the Inbox, not a project — navigating to a `project` view for the Inbox
 * sentinel is a dead end. */
function backTarget(project: string): { view: View; label: string } {
  return project === INBOX_PROJECT
    ? { view: { kind: "inbox" }, label: "Inbox" }
    : { view: { kind: "project", slug: project }, label: project };
}

/** The quiet way out, at the top of every note. */
function BackLink({ project }: { project: string }) {
  const { navigate } = useNavigation();
  const { view, label } = backTarget(project);
  return (
    <Button
      variant="quiet"
      onClick={() => navigate(view)}
      className="flex items-center gap-2xs self-start py-3xs text-label text-text-faint"
    >
      <span aria-hidden="true">←</span>
      <span>{label}</span>
    </Button>
  );
}

function OpenedNote({ noteId, project }: { noteId: string; project: string }) {
  const { note, error, setNote } = useNote(project, noteId);
  const [editing, setEditing] = useState(false);

  if (error) {
    return (
      <ViewFrame variant="doc">
        <StatusMessage variant="error">Couldn&apos;t open this note: {error}</StatusMessage>
      </ViewFrame>
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
    <ReadNote note={note} project={project} onEdit={() => setEditing(true)} />
  );
}

/**
 * Read mode. A left-pinned document on its own measure, in the full serif
 * reading ramp — the one screen in the app with no list on it, and the only
 * one whose type is sized for reading rather than for scanning.
 *
 * The chrome is two quiet text buttons and a mono meta line. Nothing else,
 * because everything else would be competing with the note.
 */
function ReadNote({
  note,
  project,
  onEdit,
}: {
  note: NoteDetail;
  project: string;
  onEdit: () => void;
}) {
  return (
    <ViewFrame variant="doc">
      <article>
        <BackLink project={project} />

        <header className="note__title-row">
          <div className="flex items-start justify-between gap-md">
            <h2 className="font-serif text-title-doc font-semibold leading-title-doc tracking-title text-text">
              {note.title}
            </h2>
            <Button
              variant="quiet"
              onClick={onEdit}
              className="flex-none py-3xs text-label text-text-soft"
            >
              Edit
            </Button>
          </div>
          <p className="mt-xs font-mono text-meta text-text-faint">
            {noteMeta(note, note.type)}
          </p>
        </header>

        {note.body_markdown ? (
          // The checkboxes GFM emits are interactive here rather than
          // disabled: an action item you cannot tick in the view you read it
          // in is a to-do list you have to open an editor to use.
          <div className="note-reading note__body font-serif">
            <ReactMarkdown remarkPlugins={[remarkGfm]}>
              {note.body_markdown}
            </ReactMarkdown>
          </div>
        ) : (
          <div className="note__body">
            <StatusMessage variant="empty">This note has no body yet.</StatusMessage>
          </div>
        )}
      </article>
    </ViewFrame>
  );
}

/** One button in the floating format toolbar. */
function Tool({
  label,
  onApply,
  className = "",
}: {
  label: ReactNode;
  onApply: () => void;
  className?: string;
}) {
  return (
    <button
      type="button"
      // pointerdown, not click: clicking would blur the textarea first, which
      // collapses the very selection the button is about to act on.
      onPointerDown={(event) => {
        event.preventDefault();
        onApply();
      }}
      className={`note-edit__tool text-label text-text ${className}`}
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
 * The format toolbar is chrome on demand: it exists only while text is
 * selected, anchored just above the selection rather than parked in a bar at
 * the top of the screen.
 */
function EditNote({
  note,
  project,
  onDone,
}: {
  note: NoteDetail;
  project: string;
  onDone: (saved: NoteDetail | null) => void;
}) {
  const [title] = useState(note.title);
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

  // Derived, never tracked. A `saved` flag could only ever be false here — a
  // successful save unmounts this component — so the line it fed claimed
  // "Unsaved changes" over a note nobody had touched yet.
  const dirty =
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
    })
      .then((result) => onDone(result))
      .catch((err: unknown) => {
        setSaving(false);
        setError(String(err));
      });
  };

  return (
    <ViewFrame variant="doc">
      <form onSubmit={submit}>
        {/* The mode bar. It is the whole reason compose mode is legible: the
            document below it is identical to read mode, so this line is what
            says which one you are in. */}
        <header className="flex items-center justify-between gap-md">
          <p className="font-mono text-eyebrow uppercase tracking-eyebrow text-text-faint">
            Editing
          </p>
          <div className="flex items-center gap-sm">
            <span className="font-mono text-cap text-text-faint">
              {saving ? "Saving…" : dirty ? "Unsaved changes" : "No changes"}
            </span>
            {/* The way out that does not write. Without it the only exit from
                compose mode was Done, so changing your mind still rewrote the
                file on disk. */}
            <Button
              variant="quiet"
              onClick={() => onDone(null)}
              disabled={saving}
              className="py-3xs text-label text-text-soft"
            >
              Cancel
            </Button>
            <Button
              type="submit"
              variant="filled"
              loading={saving}
              loadingLabel="Saving…"
              className="text-label"
            >
              Done
            </Button>
          </div>
        </header>

        {/* Read-only: the filename never changes on edit, and moving a note is
            the filing flow, not an edit. The caret still sits beside it,
            because it marks where composing is happening. */}
        <div className="note__title-row flex items-center">
          <h2 className="font-serif text-title-doc font-semibold leading-title-doc tracking-title text-text">
            {title}
          </h2>
        </div>

        <div className="mt-xs flex flex-wrap items-center gap-2xs">
          <span className="mr-3xs font-mono text-meta text-text-faint">{note.date}</span>
          {tags.map((tag) => (
            <span key={tag} className="note-edit__tag font-mono text-cap text-text-soft">
              {tag}
              <button
                type="button"
                aria-label={`Remove tag ${tag}`}
                onClick={() => setTags(tags.filter((each) => each !== tag))}
                className="ui-focus-ring text-text-faint"
              >
                ×
              </button>
            </span>
          ))}
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
              className="note-edit__tag-add ui-focus-ring w-24 font-mono text-cap text-text"
            />
          ) : (
            <button
              type="button"
              onClick={() => setAddingTag(true)}
              className="note-edit__tag-add ui-focus-ring font-mono text-cap text-text-faint"
            >
              + tag
            </button>
          )}
        </div>

        <div className="note__body relative">
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
            onBlur={() => setAnchor(null)}
            className="note-edit__body ui-writing"
          />
          {anchor && (
            <div
              className="note-edit__toolbar"
              style={{ left: anchor.left, top: anchor.top - 11 }}
            >
              <Tool label="B" onApply={() => format({ wrap: "**" })} className="font-bold" />
              <Tool label="I" onApply={() => format({ wrap: "*" })} className="italic" />
              <Tool
                label="H"
                onApply={() => format({ prefix: "## " })}
                className="font-serif font-semibold"
              />
              <span className="note-edit__tool-divider" />
              <Tool label="List" onApply={() => format({ prefix: "- " })} />
              <Tool label="Link" onApply={() => format({ link: true })} />
              <span className="note-edit__tail" aria-hidden="true" />
            </div>
          )}
        </div>

        {error && (
          <StatusMessage variant="error">Couldn&apos;t save this note: {error}</StatusMessage>
        )}
      </form>
    </ViewFrame>
  );
}

/**
 * A note that does not exist yet. It borrows compose mode's shape rather than
 * inventing a third one: the same measure, the same serif title, the same
 * filled commit button — the only additions are the two things a new note has
 * no answer for, its project and its name.
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
    <ViewFrame variant="doc">
      <form onSubmit={submit}>
        <header className="flex items-center justify-between gap-md">
          <p className="font-mono text-eyebrow uppercase tracking-eyebrow text-text-faint">
            New note
          </p>
          <Button
            type="submit"
            variant="filled"
            disabled={!trimmedProject}
            loading={submitting}
            loadingLabel="Creating…"
            className="text-label"
          >
            Create
          </Button>
        </header>

        <div className="note__title-row flex items-center">
          <input
            autoFocus
            value={title}
            onChange={(event) => setTitle(event.target.value)}
            placeholder="Untitled"
            aria-label="Title"
            className="note-edit__title ui-writing font-serif text-title-doc font-semibold leading-title-doc tracking-title text-text placeholder:text-text-faint"
          />
        </div>

        <div className="mt-xs flex items-center gap-2xs">
          <span className="font-mono text-meta text-text-faint">{todayIsoDate()}</span>
          {/* Free text with suggestions: an unknown name creates the project
              folder on save, which is how a project comes into existence at
              all. A picker would make the vault a closed set. */}
          <input
            list="kodabi-project-slugs"
            value={project}
            onChange={(event) => setProject(event.target.value)}
            placeholder="project"
            aria-label="Project"
            className="note-edit__tag-add ui-focus-ring font-mono text-cap text-text placeholder:text-text-faint"
          />
          <datalist id="kodabi-project-slugs">
            {projectSlugs.map((slug) => (
              <option key={slug} value={slug} />
            ))}
          </datalist>
        </div>

        <div className="note__body">
          <textarea
            value={body}
            aria-label="Note body"
            placeholder="Write here."
            onChange={(event) => setBody(event.target.value)}
            className="note-edit__body ui-writing placeholder:text-text-faint"
          />
        </div>

        {error && (
          <StatusMessage variant="error">Couldn&apos;t create this note: {error}</StatusMessage>
        )}
      </form>
    </ViewFrame>
  );
}
