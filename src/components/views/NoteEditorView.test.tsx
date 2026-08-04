import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { NoteEditorView } from "./NoteEditorView";
import { INITIAL_VIEW, NavigationContext, type View } from "../../useNavigation";
import type { NoteDetail } from "../../useNotes";
import { invoke, invokedCommands, onCommand, resetTauriMocks } from "../../test/tauri";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

function makeNote(
  overrides: Partial<NoteDetail> & {
    id: string;
    title: string;
    project: string | null;
  },
): NoteDetail {
  return {
    path: `${overrides.project ?? "Inbox"}/${overrides.id}.md`,
    type: "note",
    date: "2026-07-01T09:00:00Z",
    tags: [],
    source: "manual",
    confidence: null,
    snippet: "",
    guess: null,
    body_markdown: "Some body text.",
    ...overrides,
  };
}

/** Renders one opened note in read mode with a recording `navigate`, so a test
 * can assert both the `delete_note` invoke and where a successful delete lands.
 * `project` is the folder the note was opened from ("Inbox" for an unfiled
 * note). */
function renderNote(project: string, note: NoteDetail): (view: View) => void {
  const navigate = vi.fn();
  onCommand("read_note", () => note);
  // The edit form's project picker reads this; harmless in read mode.
  onCommand("list_projects", () => ({ inbox_note_count: 0, projects: [] }));
  render(
    <NavigationContext value={{ view: INITIAL_VIEW, navigate }}>
      <NoteEditorView noteId={note.id} project={project} />
    </NavigationContext>,
  );
  return navigate;
}

describe("NoteEditorView delete flow", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("deletes a filed note by id and returns to its project", async () => {
    const user = userEvent.setup();
    const note = makeNote({
      id: "n_a1b2c3",
      title: "Quarterly planning",
      project: "briarwood-golf",
    });
    onCommand("delete_note", () => ({
      id: "n_a1b2c3",
      title: "Quarterly planning",
      project: "briarwood-golf",
      session_deleted: false,
    }));
    const navigate = renderNote("briarwood-golf", note);
    await screen.findByRole("heading", { name: "Quarterly planning" });

    await user.click(screen.getByRole("button", { name: "Delete note" }));
    const dialog = screen.getByRole("dialog", { name: "Delete this note?" });
    await user.click(within(dialog).getByRole("button", { name: "Delete note" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("delete_note", { id: "n_a1b2c3" });
    });
    // A filed note's "after delete" is the project it was filed in.
    expect(navigate).toHaveBeenCalledWith({ kind: "project", slug: "briarwood-golf" });
  });

  it("deletes an unfiled note and returns to the Inbox", async () => {
    const user = userEvent.setup();
    const note = makeNote({ id: "n_d4e5f6", title: "Loose thought", project: null });
    onCommand("delete_note", () => ({
      id: "n_d4e5f6",
      title: "Loose thought",
      project: null,
      session_deleted: false,
    }));
    const navigate = renderNote("Inbox", note);
    await screen.findByRole("heading", { name: "Loose thought" });

    await user.click(screen.getByRole("button", { name: "Delete note" }));
    await user.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: "Delete note" }),
    );

    await waitFor(() => {
      expect(navigate).toHaveBeenCalledWith({ kind: "inbox" });
    });
  });

  it("cancels without deleting or navigating", async () => {
    const user = userEvent.setup();
    const note = makeNote({
      id: "n_a1b2c3",
      title: "Quarterly planning",
      project: "briarwood-golf",
    });
    const navigate = renderNote("briarwood-golf", note);
    await screen.findByRole("heading", { name: "Quarterly planning" });

    await user.click(screen.getByRole("button", { name: "Delete note" }));
    await user.click(screen.getByRole("button", { name: "Cancel" }));

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(invokedCommands()).not.toContain("delete_note");
    expect(navigate).not.toHaveBeenCalled();
  });

  it("keeps the dialog open and shows the error when the delete fails", async () => {
    const user = userEvent.setup();
    const note = makeNote({
      id: "n_a1b2c3",
      title: "Quarterly planning",
      project: "briarwood-golf",
    });
    onCommand("delete_note", () => {
      throw "disk is full";
    });
    const navigate = renderNote("briarwood-golf", note);
    await screen.findByRole("heading", { name: "Quarterly planning" });

    await user.click(screen.getByRole("button", { name: "Delete note" }));
    const dialog = screen.getByRole("dialog", { name: "Delete this note?" });
    await user.click(within(dialog).getByRole("button", { name: "Delete note" }));

    expect(
      await within(dialog).findByText("Couldn't delete the note: disk is full"),
    ).toBeInTheDocument();
    expect(navigate).not.toHaveBeenCalled();
  });
});
