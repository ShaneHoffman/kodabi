import { useNavigation } from "../useNavigation";
import { CaptureView } from "./views/CaptureView";
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
      return <NoteEditorView noteId={view.noteId} project={view.project} />;
    case "search":
      return <SearchView query={view.query} />;
    case "capture":
      return <CaptureView />;
    default: {
      // Exhaustiveness: a new View variant fails to compile until routed here.
      const exhausted: never = view;
      return exhausted;
    }
  }
}
