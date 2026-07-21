import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { InboxView } from "./InboxView";
import { NavigationProvider } from "../NavigationProvider";
import type { NoteSummary } from "../../useNotes";
import type { Project } from "../../useProjects";
import { notifyVaultChanged } from "../../useVaultQuery";
import { invoke, onCommand, resetTauriMocks } from "../../test/tauri";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

function makeNote(overrides: Partial<NoteSummary> & { id: string; title: string }): NoteSummary {
  return {
    path: `Inbox/${overrides.id}.md`,
    type: "note",
    project: null,
    date: "2026-07-01T09:00:00Z",
    tags: [],
    source: "manual",
    confidence: 0.41,
    snippet: "",
    ...overrides,
  };
}

function makeProject(slug: string): Project {
  return {
    id: slug,
    slug,
    display_name: slug,
    parent: null,
    note_count: 0,
    meeting_count: 0,
  };
}

const PLANNING = makeNote({ id: "n_a1b2c3", title: "Quarterly planning" });
const VENDOR = makeNote({ id: "n_d4e5f6", title: "Vendor follow-up" });

/** Serve the two reads the view makes. Failed captures are deliberately not
 * among them: they moved to NeedsAttentionView, and this view's empty state no
 * longer depends on anything but its own list. */
function serveVault(
  notes: NoteSummary[],
  projects = ["briarwood-golf", "kodabi"],
): void {
  onCommand("list_notes", (args) => (args?.project === "Inbox" ? notes : []));
  onCommand("list_projects", () => ({
    inbox_note_count: notes.length,
    projects: projects.map(makeProject),
  }));
}

function renderInbox() {
  return render(
    <NavigationProvider>
      <InboxView />
    </NavigationProvider>,
  );
}

/** Matches a row's picker by its accessible name, which is the sr-only label
 * followed by the trigger's own text ("… File to…"). A predicate, not a
 * `RegExp`: the note title is interpolated, and a title carrying `(` or `?`
 * would make a pattern that throws or quietly matches the wrong row. */
function pickerFor(title: string) {
  const label = `File "${title}" to project`;
  return (accessibleName: string) => accessibleName.startsWith(label);
}

/** Open a row's project picker and choose `project`. */
async function fileNote(
  user: ReturnType<typeof userEvent.setup>,
  title: string,
  project: string,
): Promise<void> {
  await user.click(screen.getByRole("combobox", { name: pickerFor(title) }));
  await user.click(screen.getByRole("option", { name: project }));
}

describe("InboxView", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("lists the notes waiting to be filed", async () => {
    serveVault([PLANNING, VENDOR]);

    renderInbox();

    expect(await screen.findByText("Quarterly planning")).toBeInTheDocument();
    expect(screen.getByText("Vendor follow-up")).toBeInTheDocument();
  });

  it("re-routes a note to the chosen project and drops the row once the vault refetches", async () => {
    const user = userEvent.setup();
    serveVault([PLANNING, VENDOR]);
    onCommand("file_note_to_project", () => ({
      note: { ...PLANNING, project: "briarwood-golf", path: "Projects/briarwood-golf/planning.md" },
      previous: { path: PLANNING.path, project: null },
      moved: true,
    }));
    renderInbox();
    await screen.findByText("Quarterly planning");

    await fileNote(user, "Quarterly planning", "briarwood-golf");

    // The correction itself: note id in, project slug out, nested under the
    // `input` key the Rust DTO expects.
    expect(invoke).toHaveBeenCalledWith("file_note_to_project", {
      input: { id: "n_a1b2c3", project: "briarwood-golf" },
    });

    // The backend broadcasts `vault:changed` after a re-route; the bridge turns
    // that into the DOM event this hook fans out on. Once the list refetches
    // without the filed note, its row is gone.
    serveVault([VENDOR]);
    act(() => {
      notifyVaultChanged();
    });

    await waitFor(() => {
      expect(screen.queryByText("Quarterly planning")).not.toBeInTheDocument();
    });
    expect(screen.getByText("Vendor follow-up")).toBeInTheDocument();
  });

  it("keeps the row and surfaces the message when the re-route fails", async () => {
    const user = userEvent.setup();
    serveVault([PLANNING]);
    onCommand("file_note_to_project", () => {
      throw "no such project: briarwood-golf";
    });
    renderInbox();
    await screen.findByText("Quarterly planning");

    await fileNote(user, "Quarterly planning", "briarwood-golf");

    // Substring, not exact: the message is rendered behind a prefix naming what
    // failed, so the raw backend string is never the whole line
    // (docs/DESIGN_SYSTEM.md §3).
    expect(await screen.findByText(/no such project: briarwood-golf/)).toBeInTheDocument();
    // The note is still unfiled, so it must still be actionable: row present
    // and the picker back (not stuck on "Filing…").
    expect(screen.getByText("Quarterly planning")).toBeInTheDocument();
    expect(
      screen.getByRole("combobox", { name: pickerFor("Quarterly planning") }),
    ).toBeInTheDocument();
  });

  it("offers no picker when there is no project to file into", async () => {
    serveVault([PLANNING], []);

    renderInbox();

    expect(await screen.findByText("Create a project to file notes.")).toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  });

  it("says nothing is waiting when the inbox is empty", async () => {
    serveVault([]);

    renderInbox();

    expect(await screen.findByText(/Nothing waiting/)).toBeInTheDocument();
  });

  it("states the workload in the header, and only when there is one", async () => {
    // The header says what the view is FOR before the list is read. At zero it
    // says nothing: the empty state below is already that sentence, and a
    // header that also says "nothing" says it worse.
    serveVault([PLANNING, VENDOR]);

    const { unmount } = renderInbox();

    // The count folds into the masthead sentence ("Inbox · 2 to file"), so the
    // noun lives in the view's name rather than being repeated in the count.
    expect(await screen.findByText("· 2 to file")).toBeInTheDocument();
    unmount();

    resetTauriMocks();
    serveVault([PLANNING]);
    renderInbox();

    expect(await screen.findByText("· 1 to file")).toBeInTheDocument();
  });

  it("holds no needs-attention queue of its own", async () => {
    // The Inbox has one job. Failed captures are a different job with a
    // different verb, and they now live in their own view; even served one,
    // this view must not grow a second queue back.
    serveVault([PLANNING]);
    onCommand("list_failed_sessions", () => [
      {
        path: "sessions/2026-07-01T10-00-00Z-team-sync.jsonl",
        file_name: "2026-07-01T10-00-00Z-team-sync.jsonl",
        slug: "team-sync",
        captured_at: "2026-07-01T10:00:00Z",
      },
    ]);

    renderInbox();
    await screen.findByText("Quarterly planning");

    expect(screen.queryByTestId("needs-attention")).not.toBeInTheDocument();
    expect(screen.queryByTestId("retry-distill")).not.toBeInTheDocument();
  });

  it("surfaces a failed listing instead of claiming an empty inbox", async () => {
    onCommand("list_notes", () => {
      throw "the vault is unreadable";
    });
    onCommand("list_projects", () => ({ inbox_note_count: 0, projects: [] }));

    renderInbox();

    expect(await screen.findByText(/the vault is unreadable/)).toBeInTheDocument();
    expect(screen.queryByText(/Nothing waiting/)).not.toBeInTheDocument();
  });
});
