import { useNavigation } from "../useNavigation";
import { InboxView } from "./views/InboxView";
import { NeedsAttentionView } from "./views/NeedsAttentionView";
import { NoteEditorView } from "./views/NoteEditorView";
import { ProjectView } from "./views/ProjectView";
import { SearchView } from "./views/SearchView";
import { SettingsView } from "./views/SettingsView";

/** Routes the main region to the active destination view. */
export function MainContent() {
  const { view } = useNavigation();

  switch (view.kind) {
    case "inbox":
      return <InboxView />;
    case "needsAttention":
      return <NeedsAttentionView />;
    case "project":
      return <ProjectView slug={view.slug} />;
    case "noteEditor":
      // Keyed by the navigation target so a noteEditor→noteEditor jump
      // remounts: without this, a create form open for one project would keep
      // its form state when e.g. the palette's "New note" navigates here anew.
      return (
        <NoteEditorView
          key={`${view.noteId ?? "new"}:${view.project ?? ""}`}
          noteId={view.noteId}
          project={view.project}
        />
      );
    case "search":
      // Keyed for the same reason as noteEditor: SearchView seeds its editable
      // draft from this prop once, so a second search (the palette's
      // `Search for "…"` row) has to remount to be seen at all.
      return <SearchView key={view.query} query={view.query} />;
    case "settings":
      return <SettingsView />;
    default: {
      // Exhaustiveness: a new View variant fails to compile until routed here.
      const exhausted: never = view;
      return exhausted;
    }
  }
}
