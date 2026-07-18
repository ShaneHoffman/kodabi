import { useState, type FormEvent, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { useNavigation, type View } from "../../useNavigation";
import {
  createNote,
  INBOX_PROJECT,
  notifyVaultChanged,
  parseTagsInput,
  saveNote,
  todayIsoDate,
  useNote,
  type NoteDetail,
  type NoteType,
} from "../../useNotes";
import { formatSlug, useProjects } from "../../useProjects";
import "./NoteEditorView.css";

type Props = {
  noteId: string | null;
  project: string | null;
};

const NOTE_TYPES: NoteType[] = ["note", "meeting", "chat"];

/** Value-shift field surface (bg-sink on bg), never a border. */
const FIELD_CLASS =
  "w-full rounded-md bg-bg-sink p-xs text-body text-text focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent";

const ACTION_CLASS =
  "flex-none text-body text-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent disabled:text-text-faint";

/**
 * The note screen's three lives: a create form (no id yet), the serif reading
 * view, and an in-place editor. All writes go through the backend's Markdown
 * writer, so the on-disk frontmatter stays schema-valid; on edit the backend
 * preserves `id`, `source`, and routing verbatim.
 */
export function NoteEditorView({ noteId, project }: Props) {
  if (noteId === null) {
    return <CreateNote initialProject={project} />;
  }
  if (project === null) {
    // Unreachable via current navigation: every opened note arrives with its
    // project. A quiet dead-end beats a crash if a future caller slips.
    return (
      <Frame>
        <p className="text-body text-text-soft">
          This note arrived without its project. Open it from a project list.
        </p>
      </Frame>
    );
  }
  return <OpenedNote key={noteId} noteId={noteId} project={project} />;
}

/** The shared page scaffold: centered content column inside a padded page. */
function Frame({ children }: { children: ReactNode }) {
  return (
    <section className="flex min-h-full flex-col p-xl">
      <div className="mx-auto flex w-full max-w-content flex-col gap-lg">
        {children}
      </div>
    </section>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="flex flex-1 flex-col gap-3xs">
      <span className="text-eyebrow uppercase tracking-wide text-text-faint">
        {label}
      </span>
      {children}
    </label>
  );
}

/** The date/type/tags row shared by the create and edit forms. */
function MetaFields(props: {
  noteType: NoteType;
  onNoteType: (t: NoteType) => void;
  date: string;
  onDate: (d: string) => void;
  tagsRaw: string;
  onTagsRaw: (t: string) => void;
}) {
  return (
    <div className="flex flex-wrap gap-md">
      <Field label="Type">
        <select
          value={props.noteType}
          onChange={(e) => props.onNoteType(e.target.value as NoteType)}
          className={FIELD_CLASS}
        >
          {NOTE_TYPES.map((t) => (
            <option key={t} value={t}>
              {t}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Date">
        {/* A plain text input, not type="date": an RFC 3339 timestamp must
            stay editable verbatim (the backend stores dates as written). */}
        <input
          value={props.date}
          onChange={(e) => props.onDate(e.target.value)}
          placeholder="2026-07-17"
          className={FIELD_CLASS}
        />
      </Field>
      <Field label="Tags">
        <input
          value={props.tagsRaw}
          onChange={(e) => props.onTagsRaw(e.target.value)}
          placeholder="comma or space separated"
          className={FIELD_CLASS}
        />
      </Field>
    </div>
  );
}

function CreateNote({ initialProject }: { initialProject: string | null }) {
  const { navigate } = useNavigation();
  const { entries } = useProjects();
  const [project, setProject] = useState(initialProject ?? "");
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [noteType, setNoteType] = useState<NoteType>("note");
  const [date, setDate] = useState(todayIsoDate());
  const [tagsRaw, setTagsRaw] = useState("");
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
      type: noteType,
      project: trimmedProject,
      date: date.trim(),
      tags: parseTagsInput(tagsRaw),
      source: "manual",
      body,
      title: title.trim() || null,
    })
      .then((created) => {
        notifyVaultChanged();
        // Land in the read view via read_note — the round trip through the
        // on-disk file is the proof the note was written schema-valid. The
        // echoed project is the backend-canonicalized casing, which may
        // differ from what was typed.
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
    <Frame>
      <form onSubmit={submit} className="flex flex-col gap-md">
        <header className="flex items-baseline justify-between gap-md">
          <h2 className="font-serif text-h2 text-text">New note</h2>
          <button
            type="submit"
            disabled={!trimmedProject || submitting}
            className={ACTION_CLASS}
          >
            Create note
          </button>
        </header>

        <div className="flex flex-wrap gap-md">
          <Field label="Project">
            {/* Native free-text + suggestions: an unknown name creates the
                project folder on save. */}
            <input
              list="kodabi-project-slugs"
              value={project}
              onChange={(e) => setProject(e.target.value)}
              placeholder="e.g. Growth/Q3"
              className={FIELD_CLASS}
            />
            <datalist id="kodabi-project-slugs">
              {projectSlugs.map((slug) => (
                <option key={slug} value={slug} />
              ))}
            </datalist>
          </Field>
          <Field label="Title">
            <input
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="names the file"
              className={FIELD_CLASS}
            />
          </Field>
        </div>

        <Field label="Body">
          <textarea
            value={body}
            onChange={(e) => setBody(e.target.value)}
            placeholder="Markdown"
            className={`${FIELD_CLASS} note-editor__body font-mono`}
          />
        </Field>

        <MetaFields
          noteType={noteType}
          onNoteType={setNoteType}
          date={date}
          onDate={setDate}
          tagsRaw={tagsRaw}
          onTagsRaw={setTagsRaw}
        />

        {error && <p className="text-body text-text-soft">{error}</p>}
      </form>
    </Frame>
  );
}

function OpenedNote({ noteId, project }: { noteId: string; project: string }) {
  const { note, error, setNote } = useNote(project, noteId);
  const [editing, setEditing] = useState(false);

  if (error) {
    return (
      <Frame>
        <p className="text-body text-text-soft">{error}</p>
      </Frame>
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
      onCancel={() => setEditing(false)}
      onSaved={(saved) => {
        setNote(saved);
        setEditing(false);
        notifyVaultChanged();
      }}
    />
  ) : (
    <ReadNote note={note} project={project} onEdit={() => setEditing(true)} />
  );
}

function ReadNote({
  note,
  project,
  onEdit,
}: {
  note: NoteDetail;
  project: string;
  onEdit: () => void;
}) {
  const { navigate } = useNavigation();
  const meta = [note.date.slice(0, 10), note.type, ...note.tags].join(" · ");

  // An unfiled note came from the Inbox, not a project — send "back" there
  // (navigating to a `project` view for the Inbox sentinel is a dead end).
  const isInbox = project === INBOX_PROJECT;
  const backView: View = isInbox
    ? { kind: "inbox" }
    : { kind: "project", slug: project };
  const backLabel = isInbox ? "Inbox" : formatSlug(project);

  return (
    <Frame>
      <article className="flex flex-col gap-lg">
        <header className="flex flex-col gap-3xs">
          <button
            type="button"
            onClick={() => navigate(backView)}
            className="flex items-center gap-2xs self-start text-cap text-text-faint hover:text-text-soft focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
          >
            <span aria-hidden="true">←</span>
            <span>{backLabel}</span>
          </button>
          <div className="flex items-baseline justify-between gap-md">
            <h2 className="font-serif text-h2 text-text">{note.title}</h2>
            <button type="button" onClick={onEdit} className={ACTION_CLASS}>
              Edit
            </button>
          </div>
          <p className="text-cap text-text-faint">{meta}</p>
        </header>

        {note.body_markdown ? (
          <div className="note-reading max-w-measure font-serif text-read text-text">
            <ReactMarkdown remarkPlugins={[remarkGfm]}>
              {note.body_markdown}
            </ReactMarkdown>
          </div>
        ) : (
          <p className="text-body text-text-soft">This note has no body yet.</p>
        )}
      </article>
    </Frame>
  );
}

function EditNote({
  note,
  project,
  onCancel,
  onSaved,
}: {
  note: NoteDetail;
  project: string;
  onCancel: () => void;
  onSaved: (saved: NoteDetail) => void;
}) {
  const [body, setBody] = useState(note.body_markdown);
  const [noteType, setNoteType] = useState<NoteType>(note.type);
  const [date, setDate] = useState(note.date);
  const [tagsRaw, setTagsRaw] = useState(note.tags.join(", "));
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (saving) return;
    setSaving(true);
    setError(null);
    saveNote({
      id: note.id,
      project,
      type: noteType,
      date: date.trim(),
      tags: parseTagsInput(tagsRaw),
      body,
    })
      .then(onSaved)
      .catch((err: unknown) => {
        setSaving(false);
        setError(String(err));
      });
  };

  return (
    <Frame>
      <form onSubmit={submit} className="flex flex-col gap-md">
        <header className="flex items-baseline justify-between gap-md">
          {/* Title and project are read-only here: the filename never changes
              on edit, and moving a note is the re-route flow, not an edit. */}
          <h2 className="font-serif text-h2 text-text">{note.title}</h2>
          <div className="flex flex-none items-baseline gap-md">
            <button
              type="button"
              onClick={onCancel}
              className="text-body text-text-soft hover:text-text focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
            >
              Cancel
            </button>
            <button type="submit" disabled={saving} className={ACTION_CLASS}>
              Save
            </button>
          </div>
        </header>

        <Field label="Body">
          <textarea
            value={body}
            onChange={(e) => setBody(e.target.value)}
            className={`${FIELD_CLASS} note-editor__body font-mono`}
          />
        </Field>

        <MetaFields
          noteType={noteType}
          onNoteType={setNoteType}
          date={date}
          onDate={setDate}
          tagsRaw={tagsRaw}
          onTagsRaw={setTagsRaw}
        />

        {error && <p className="text-body text-text-soft">{error}</p>}
      </form>
    </Frame>
  );
}
