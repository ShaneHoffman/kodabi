import { useState, type FormEvent } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { useNavigation, type View } from "../../useNavigation";
import { noteMeta } from "../../noteMeta";
import {
  createNote,
  INBOX_PROJECT,
  parseTagsInput,
  saveNote,
  todayIsoDate,
  useNote,
  type NoteDetail,
  type NoteType,
} from "../../useNotes";
import { formatSlug, useProjects } from "../../useProjects";
import { Button } from "../ui/Button";
import { Select } from "../ui/Select";
import { StatusMessage } from "../ui/StatusMessage";
import { Textarea } from "../ui/Textarea";
import { TextField } from "../ui/TextField";
import { ViewFrame } from "../ui/ViewFrame";
import "./NoteEditorView.css";

type Props = {
  noteId: string | null;
  project: string | null;
};

const NOTE_TYPE_OPTIONS = (["note", "meeting", "chat"] satisfies NoteType[]).map(
  (value) => ({ value, label: value }),
);

/**
 * The note screen's three lives: a create form (no id yet), the serif reading
 * view, and an in-place editor. All writes go through the backend's Markdown
 * writer, so the on-disk frontmatter stays schema-valid; on edit the backend
 * preserves `id`, `source`, and routing verbatim.
 *
 * This screen used to run a parallel design system — its own FIELD_CLASS and
 * ACTION_CLASS strings, a local `Field` label wrapper whose labels were eyebrows
 * rather than field labels, a native <select> that ignored the token theme
 * entirely, and five hand-rolled buttons. All of it now comes from the shared
 * primitives (docs/UI_CONVENTIONS.md).
 */
export function NoteEditorView({ noteId, project }: Props) {
  if (noteId === null) {
    return <CreateNote initialProject={project} />;
  }
  if (project === null) {
    // Unreachable via current navigation: every opened note arrives with its
    // project. A quiet dead-end beats a crash if a future caller slips.
    return (
      <ViewFrame>
        <StatusMessage variant="empty">
          This note arrived without its project. Open it from a project list.
        </StatusMessage>
      </ViewFrame>
    );
  }
  return <OpenedNote key={noteId} noteId={noteId} project={project} />;
}

/** The date/type/tags row shared by the create and edit forms. */
function MetaFields(props: {
  noteType: NoteType;
  onNoteType: (t: NoteType) => void;
  date: string;
  onDate: (d: string) => void;
  tagsRaw: string;
  onTagsRaw: (t: string) => void;
  disabled?: boolean;
}) {
  return (
    <div className="flex flex-wrap gap-md">
      <div className="min-w-48 flex-1">
        <Select
          label="Type"
          value={props.noteType}
          onChange={(value) => props.onNoteType(value as NoteType)}
          options={NOTE_TYPE_OPTIONS}
          disabled={props.disabled}
        />
      </div>
      <div className="min-w-48 flex-1">
        {/* A plain text input, not type="date": an RFC 3339 timestamp must
            stay editable verbatim (the backend stores dates as written). */}
        <TextField
          label="Date"
          value={props.date}
          onChange={(e) => props.onDate(e.target.value)}
          placeholder="2026-07-17"
          disabled={props.disabled}
        />
      </div>
      <div className="min-w-48 flex-1">
        <TextField
          label="Tags"
          value={props.tagsRaw}
          onChange={(e) => props.onTagsRaw(e.target.value)}
          placeholder="comma or space separated"
          disabled={props.disabled}
        />
      </div>
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
        // Land in the read view via read_note — the round trip through the
        // on-disk file is the proof the note was written schema-valid. The
        // echoed project is the backend-canonicalized casing, which may
        // differ from what was typed. Every window's lists refresh because the
        // write_note command broadcasts `vault:changed` (the watcher is a
        // fallback for external edits).
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
    <ViewFrame>
      <form onSubmit={submit} className="flex flex-col gap-md">
        <header className="flex items-baseline justify-between gap-md">
          <h2 className="font-serif text-h2 text-text">New note</h2>
          <Button
            type="submit"
            variant="quiet"
            disabled={!trimmedProject}
            loading={submitting}
            loadingLabel="Creating…"
            className="-mr-xs flex-none text-body text-accent"
          >
            Create note
          </Button>
        </header>

        <div className="flex flex-wrap gap-md">
          <div className="min-w-48 flex-1">
            {/* Native free-text + suggestions: an unknown name creates the
                project folder on save. */}
            <TextField
              label="Project"
              list="kodabi-project-slugs"
              value={project}
              onChange={(e) => setProject(e.target.value)}
              placeholder="e.g. Growth/Q3"
              disabled={submitting}
            />
            <datalist id="kodabi-project-slugs">
              {projectSlugs.map((slug) => (
                <option key={slug} value={slug} />
              ))}
            </datalist>
          </div>
          <div className="min-w-48 flex-1">
            <TextField
              label="Title"
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="names the file"
              disabled={submitting}
            />
          </div>
        </div>

        <Textarea
          label="Body"
          value={body}
          onChange={(e) => setBody(e.target.value)}
          placeholder="Markdown"
          disabled={submitting}
          className="note-editor__body font-mono"
        />

        <MetaFields
          noteType={noteType}
          onNoteType={setNoteType}
          date={date}
          onDate={setDate}
          tagsRaw={tagsRaw}
          onTagsRaw={setTagsRaw}
          disabled={submitting}
        />

        {error && (
          <StatusMessage variant="error">Couldn&apos;t create this note: {error}</StatusMessage>
        )}
      </form>
    </ViewFrame>
  );
}

function OpenedNote({ noteId, project }: { noteId: string; project: string }) {
  const { note, error, setNote } = useNote(project, noteId);
  const [editing, setEditing] = useState(false);

  if (error) {
    return (
      <ViewFrame>
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
      onCancel={() => setEditing(false)}
      onSaved={(saved) => {
        // The editor shows the save's echo immediately; other windows' lists
        // refresh because the save_note command broadcasts `vault:changed`.
        setNote(saved);
        setEditing(false);
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

  // An unfiled note came from the Inbox, not a project — send "back" there
  // (navigating to a `project` view for the Inbox sentinel is a dead end).
  const isInbox = project === INBOX_PROJECT;
  const backView: View = isInbox
    ? { kind: "inbox" }
    : { kind: "project", slug: project };
  const backLabel = isInbox ? "Inbox" : formatSlug(project);

  return (
    <ViewFrame>
      <article className="flex flex-col gap-lg">
        <header className="flex flex-col gap-3xs">
          <Button
            variant="quiet"
            onClick={() => navigate(backView)}
            className="-ml-xs flex items-center gap-2xs self-start text-cap text-text-faint"
          >
            <span aria-hidden="true">←</span>
            <span>{backLabel}</span>
          </Button>
          <div className="flex items-baseline justify-between gap-md">
            <h2 className="font-serif text-h2 text-text">{note.title}</h2>
            <Button
              variant="quiet"
              onClick={onEdit}
              className="-mr-xs flex-none text-body text-accent"
            >
              Edit
            </Button>
          </div>
          <p className="text-cap text-text-faint">{noteMeta(note, note.type)}</p>
        </header>

        {note.body_markdown ? (
          <div className="note-reading max-w-measure font-serif text-read text-text">
            <ReactMarkdown remarkPlugins={[remarkGfm]}>
              {note.body_markdown}
            </ReactMarkdown>
          </div>
        ) : (
          <StatusMessage variant="empty">This note has no body yet.</StatusMessage>
        )}
      </article>
    </ViewFrame>
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
    <ViewFrame>
      <form onSubmit={submit} className="flex flex-col gap-md">
        <header className="flex items-baseline justify-between gap-md">
          {/* Title and project are read-only here: the filename never changes
              on edit, and moving a note is the re-route flow, not an edit. */}
          <h2 className="font-serif text-h2 text-text">{note.title}</h2>
          <div className="flex flex-none items-baseline gap-md">
            <Button
              variant="quiet"
              onClick={onCancel}
              disabled={saving}
              className="text-body text-text-soft"
            >
              Cancel
            </Button>
            <Button
              type="submit"
              variant="quiet"
              loading={saving}
              loadingLabel="Saving…"
              className="-mr-xs flex-none text-body text-accent"
            >
              Save
            </Button>
          </div>
        </header>

        <Textarea
          label="Body"
          value={body}
          onChange={(e) => setBody(e.target.value)}
          disabled={saving}
          className="note-editor__body font-mono"
        />

        <MetaFields
          noteType={noteType}
          onNoteType={setNoteType}
          date={date}
          onDate={setDate}
          tagsRaw={tagsRaw}
          onTagsRaw={setTagsRaw}
          disabled={saving}
        />

        {error && (
          <StatusMessage variant="error">Couldn&apos;t save this note: {error}</StatusMessage>
        )}
      </form>
    </ViewFrame>
  );
}
