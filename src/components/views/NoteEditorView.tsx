import { PlaceholderView } from "./PlaceholderView";

type Props = {
  noteId: string | null;
  project: string | null;
};

export function NoteEditorView({ noteId, project }: Props) {
  return (
    <PlaceholderView
      title={noteId ? "Edit note" : "New note"}
      caption={
        project
          ? `Writing in ${project} — the serif reading view and schema-valid frontmatter arrive with the editor.`
          : "A fresh note — it routes on save, resting in the Inbox until then."
      }
      detail="Arrives with #46"
    />
  );
}
