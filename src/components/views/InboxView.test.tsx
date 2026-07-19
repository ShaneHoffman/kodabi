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

/** Serve `list_notes` from `notes`, plus the two other reads the view makes. */
function serveVault(notes: NoteSummary[], projects = ["briarwood-golf", "kodabi"]): void {
  onCommand("list_notes", (args) => (args?.project === "Inbox" ? notes : []));
  onCommand("list_projects", () => ({
    inbox_note_count: notes.length,
    projects: projects.map(makeProject),
  }));
  // The needs-attention section is a separate seam; an empty list is the
  // normal case and renders nothing.
  onCommand("list_failed_sessions", () => []);
}

function renderInbox() {
  return render(
    <NavigationProvider>
      <InboxView />
    </NavigationProvider>,
  );
}

/** Open a row's project picker and choose `project`. The trigger's accessible
 * name is its sr-only label followed by the button's own text, hence the
 * regex rather than an exact string. */
async function fileNote(
  user: ReturnType<typeof userEvent.setup>,
  title: string,
  project: string,
): Promise<void> {
  await user.click(
    screen.getByRole("combobox", { name: new RegExp(`File "${title}" to project`, "i") }),
  );
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

    expect(await screen.findByText("no such project: briarwood-golf")).toBeInTheDocument();
    // The note is still unfiled, so it must still be actionable: row present
    // and the picker back (not stuck on "Filing…").
    expect(screen.getByText("Quarterly planning")).toBeInTheDocument();
    expect(
      screen.getByRole("combobox", { name: /File "Quarterly planning" to project/i }),
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

  it("surfaces a failed listing instead of claiming an empty inbox", async () => {
    onCommand("list_notes", () => {
      throw "the vault is unreadable";
    });
    onCommand("list_projects", () => ({ inbox_note_count: 0, projects: [] }));
    onCommand("list_failed_sessions", () => []);

    renderInbox();

    expect(await screen.findByText("the vault is unreadable")).toBeInTheDocument();
    expect(screen.queryByText(/Nothing waiting/)).not.toBeInTheDocument();
  });
});
