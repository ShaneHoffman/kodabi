import { useNavigation } from "../useNavigation";
import { InboxView } from "./views/InboxView";
import { NoteEditorView } from "./views/NoteEditorView";
import { ProjectView } from "./views/ProjectView";
import { SearchView } from "./views/SearchView";

/** Routes the main region to the active destination view. */
export function MainContent() {
  const { view } = useNavigation();

  switch (view.kind) {
    case "inbox":
      return <InboxView />;
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
      return <SearchView query={view.query} />;
    default: {
      // Exhaustiveness: a new View variant fails to compile until routed here.
      const exhausted: never = view;
      return exhausted;
    }
  }
}
