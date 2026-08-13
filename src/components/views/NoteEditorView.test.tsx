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
function renderNote(
  project: string,
  note: NoteDetail,
  origin?: View,
): (view: View) => void {
  const navigate = vi.fn();
  onCommand("read_note", () => note);
  // The edit form's project picker reads this; harmless in read mode.
  onCommand("list_projects", () => ({ inbox_note_count: 0, projects: [] }));
  render(
    <NavigationContext value={{ view: INITIAL_VIEW, navigate }}>
      <NoteEditorView noteId={note.id} project={project} origin={origin} />
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

describe("NoteEditorView back link", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("returns to where the note was opened from, not to where it is filed", async () => {
    // The case the origin exists for: a search spans projects, so the folder a
    // hit happens to live in is not the way back from it.
    const user = userEvent.setup();
    const note = makeNote({
      id: "n_a1b2c3",
      title: "Quarterly planning",
      project: "briarwood-golf",
    });
    const navigate = renderNote("briarwood-golf", note, {
      kind: "search",
      query: "budget",
    });
    await screen.findByRole("heading", { name: "Quarterly planning" });

    await user.click(screen.getByRole("button", { name: "Search" }));

    expect(navigate).toHaveBeenCalledWith({ kind: "search", query: "budget" });
  });

  it("names a project origin by its slug", async () => {
    const note = makeNote({ id: "n_a1b2c3", title: "Quarterly planning", project: null });
    renderNote("Inbox", note, { kind: "project", slug: "briarwood-golf" });
    await screen.findByRole("heading", { name: "Quarterly planning" });

    // The label is the destination, and the destination is the origin even
    // though the note itself is unfiled.
    expect(screen.getByRole("button", { name: "briarwood-golf" })).toBeInTheDocument();
  });

  it("falls back to the filing location when nothing said where you came from", async () => {
    // The palette's New note, and the note just created, arrive without one.
    const user = userEvent.setup();
    const note = makeNote({
      id: "n_a1b2c3",
      title: "Quarterly planning",
      project: "briarwood-golf",
    });
    const navigate = renderNote("briarwood-golf", note);
    await screen.findByRole("heading", { name: "Quarterly planning" });

    await user.click(screen.getByRole("button", { name: "briarwood-golf" }));

    expect(navigate).toHaveBeenCalledWith({ kind: "project", slug: "briarwood-golf" });
  });

  it("sends a deleted note back the same way it would have walked out", async () => {
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
    const navigate = renderNote("briarwood-golf", note, { kind: "inbox" });
    await screen.findByRole("heading", { name: "Quarterly planning" });

    await user.click(screen.getByRole("button", { name: "Delete note" }));
    await user.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: "Delete note" }),
    );

    await waitFor(() => {
      expect(navigate).toHaveBeenCalledWith({ kind: "inbox" });
    });
  });
});

describe("NoteEditorView details rail", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("reports what the note is, beside what it says", async () => {
    const note = makeNote({
      id: "n_a1b2c3",
      title: "Quarterly planning",
      project: "briarwood-golf",
      type: "meeting",
      tags: ["budget", "q3"],
    });
    renderNote("briarwood-golf", note);
    await screen.findByRole("heading", { name: "Quarterly planning" });

    const rail = screen.getByRole("complementary", { name: "Note details" });
    // Each fact gets its own named row, where they used to be one `·`-joined
    // line at metadata weight under the title.
    expect(within(rail).getByText("Filed in")).toBeInTheDocument();
    expect(within(rail).getByText("briarwood-golf")).toBeInTheDocument();
    expect(within(rail).getByText("meeting")).toBeInTheDocument();
    // The day, not the instant: a frontmatter date is either a local calendar
    // day or an offset-bearing timestamp, and the day is the part being read.
    expect(within(rail).getByText("2026-07-01")).toBeInTheDocument();
    // Two facts the note screen never showed at all before the rail existed.
    expect(within(rail).getByText("n_a1b2c3")).toBeInTheDocument();
    expect(within(rail).getByText("budget · q3")).toBeInTheDocument();
  });

  it("lets the rail's open-ended rows wrap rather than clip", async () => {
    // jsdom has no layout, so what this locks in is the class contract, the
    // same way ChatView's bubble test does. It matters because `getByText`
    // above passes identically whether the value span carries `truncate` (the
    // spelling this replaced, which clips to one nowrap line) or the wrapping
    // pair — so without this the regression returns green. `min-w-0` is
    // load-bearing next to `break-words`, not tidiness: `overflow-wrap:
    // break-word` does not lower a span's min-content width, so a flex item
    // left at `min-width: auto` cannot shrink and the long word paints outside
    // the rail. Measured in a browser at the rail's real geometry (a 272px
    // track, less `glass-card`'s px-4, is a 240px content box): with both
    // classes a 58-character unbroken tag wraps to 3 lines and overflows by
    // 0px; drop either one and it paints past the card's edge.
    //
    // The Tags row is the one that cannot afford clipping: compose mode drops
    // it for the editable chips, so read mode is the only full listing a note's
    // tags ever get.
    const note = makeNote({
      id: "n_a1b2c3",
      title: "Quarterly planning",
      project: "briarwood-golf-course-renovation",
      tags: ["quarterly-budget-review", "stakeholders"],
    });
    renderNote("briarwood-golf-course-renovation", note);
    await screen.findByRole("heading", { name: "Quarterly planning" });

    const rail = screen.getByRole("complementary", { name: "Note details" });
    for (const value of [
      within(rail).getByText("briarwood-golf-course-renovation"),
      within(rail).getByText("quarterly-budget-review · stakeholders"),
    ]) {
      expect(value).toHaveClass("break-words");
      expect(value).toHaveClass("min-w-0");
      // The `wrap` row's other half: baseline, not centre, so the label stays
      // on the value's first line once the value is taller than one.
      expect(value.closest("dd")).toHaveClass("items-baseline");
    }

    // The fixed-width rows keep centring — `wrap` is opt-in, not the default.
    expect(within(rail).getByText("n_a1b2c3").closest("dd")).toHaveClass("items-center");
  });

  it("drops the Tags row in compose mode, where the tags themselves are", async () => {
    // Two copies of one list, and this one reads the SAVED note — so adding a
    // tag would leave the rail contradicting the chips two lines above it.
    const user = userEvent.setup();
    const note = makeNote({
      id: "n_a1b2c3",
      title: "Quarterly planning",
      project: "briarwood-golf",
      tags: ["budget"],
    });
    renderNote("briarwood-golf", note);
    await screen.findByRole("heading", { name: "Quarterly planning" });
    expect(
      within(screen.getByRole("complementary", { name: "Note details" })).getByText("Tags"),
    ).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Edit" }));

    const rail = screen.getByRole("complementary", { name: "Note details" });
    expect(within(rail).queryByText("Tags")).toBeNull();
    // The editable list is still there, and it is the only one.
    expect(screen.getByRole("list", { name: "Tags" })).toBeInTheDocument();
    // ...and the rest of the rail survives the mode change.
    expect(within(rail).getByText("Note id")).toBeInTheDocument();
  });

  it("names the Inbox rather than inventing a folder for an unfiled note", async () => {
    const note = makeNote({ id: "n_d4e5f6", title: "Loose thought", project: null });
    renderNote("Inbox", note);
    await screen.findByRole("heading", { name: "Loose thought" });

    const rail = screen.getByRole("complementary", { name: "Note details" });
    expect(within(rail).getByText("Inbox")).toBeInTheDocument();
    // No tags, no row: an empty value is worse than an absent one.
    expect(within(rail).queryByText("Tags")).toBeNull();
  });
});

describe("NoteEditorView action items", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("renders a GFM task list as checkboxes that state status without offering to change it", async () => {
    const note = makeNote({
      id: "n_a1b2c3",
      title: "Quarterly planning",
      project: "briarwood-golf",
      body_markdown: "## Action items\n\n- [x] Send the install records\n- [ ] Confirm warranty terms",
    });
    renderNote("briarwood-golf", note);
    await screen.findByRole("heading", { name: "Quarterly planning" });

    const boxes = screen.getAllByRole("checkbox");
    expect(boxes).toHaveLength(2);
    expect(boxes[0]).toBeChecked();
    expect(boxes[1]).not.toBeChecked();
    // Disabled by mdast-util-to-hast, and honestly so: there is no write path
    // behind a click, so the box must not advertise that it takes one.
    expect(boxes[0]).toBeDisabled();
    expect(boxes[1]).toBeDisabled();

    // The done row recedes: faint and struck, driven off the real `:checked`
    // rather than off a parallel piece of state.
    const doneRow = boxes[0].closest("li");
    expect(doneRow).toHaveClass("has-[:checked]:line-through");
    expect(doneRow).toHaveTextContent("Send the install records");

    // A section head inside a note keeps its semantics while dropping to the
    // caps register — the note's own title is the only document-sized heading.
    expect(
      screen.getByRole("heading", { name: "Action items", level: 2 }),
    ).toBeInTheDocument();
  });
});
