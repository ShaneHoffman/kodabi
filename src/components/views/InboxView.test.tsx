import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { InboxView } from "./InboxView";
import { NavigationProvider } from "../NavigationProvider";
import type { NoteSummary } from "../../useNotes";
import type { Project } from "../../useProjects";
import type { FailedSession } from "../../useSessions";
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

function makeSession(slug: string): FailedSession {
  return {
    path: `sessions/2026-07-01T10-00-00Z-${slug}.jsonl`,
    file_name: `2026-07-01T10-00-00Z-${slug}.jsonl`,
    slug,
    captured_at: "2026-07-01T10:00:00Z",
  };
}

const PLANNING = makeNote({ id: "n_a1b2c3", title: "Quarterly planning" });
const VENDOR = makeNote({ id: "n_d4e5f6", title: "Vendor follow-up" });

/** Serve the three reads the view makes. `sessions` defaults to empty — a
 * meeting that never became a note is the exception, not the norm — but the
 * empty state depends on it, so it is a parameter rather than a constant. */
function serveVault(
  notes: NoteSummary[],
  projects = ["paradise-golf", "kodabi"],
  sessions: FailedSession[] = [],
): void {
  onCommand("list_notes", (args) => (args?.project === "Inbox" ? notes : []));
  onCommand("list_projects", () => ({
    inbox_note_count: notes.length,
    projects: projects.map(makeProject),
  }));
  onCommand("list_failed_sessions", () => sessions);
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
      note: { ...PLANNING, project: "paradise-golf", path: "Projects/paradise-golf/planning.md" },
      previous: { path: PLANNING.path, project: null },
      moved: true,
    }));
    renderInbox();
    await screen.findByText("Quarterly planning");

    await fileNote(user, "Quarterly planning", "paradise-golf");

    // The correction itself: note id in, project slug out, nested under the
    // `input` key the Rust DTO expects.
    expect(invoke).toHaveBeenCalledWith("file_note_to_project", {
      input: { id: "n_a1b2c3", project: "paradise-golf" },
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
      throw "no such project: paradise-golf";
    });
    renderInbox();
    await screen.findByText("Quarterly planning");

    await fileNote(user, "Quarterly planning", "paradise-golf");

    // Substring, not exact: the message is rendered behind a prefix naming what
    // failed, so the raw backend string is never the whole line
    // (docs/DESIGN_SYSTEM.md §3).
    expect(await screen.findByText(/no such project: paradise-golf/)).toBeInTheDocument();
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

  it("never claims nothing is waiting above a meeting that needs a retry", async () => {
    // No unfiled notes, but a captured meeting that never became a note. The
    // empty state speaks for the whole view, so "Nothing waiting" here would
    // tell the user there is nothing to do while showing them a Retry button.
    serveVault([], ["paradise-golf"], [makeSession("team-sync")]);

    renderInbox();

    expect(await screen.findByTestId("needs-attention")).toBeInTheDocument();
    expect(screen.queryByText(/Nothing waiting/)).not.toBeInTheDocument();
  });

  it("caps the needs-attention list so it cannot bury the notes below it", async () => {
    // The section is an exception list inside a view named for something else;
    // unbounded it pushes the Inbox's own subject off the screen
    // (docs/DESIGN_SYSTEM.md §1). It caps, and says how much it is holding back.
    const user = userEvent.setup();
    serveVault(
      [PLANNING],
      ["paradise-golf"],
      ["team-sync", "vendor-call", "board-prep", "retro"].map(makeSession),
    );

    renderInbox();

    expect(await screen.findByTestId("needs-attention")).toBeInTheDocument();
    expect(screen.getAllByTestId("retry-distill")).toHaveLength(3);
    expect(screen.queryByText("retro")).not.toBeInTheDocument();
    // The note the view is actually named for is still on screen.
    expect(screen.getByText("Quarterly planning")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Show 1 more" }));

    expect(screen.getAllByTestId("retry-distill")).toHaveLength(4);
    expect(screen.getByText("retro")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Show fewer" }));

    expect(screen.getAllByTestId("retry-distill")).toHaveLength(3);
  });

  it("never claims nothing is waiting when the session listing failed", async () => {
    // Same rule for the other unknown: a failed read is not an empty list.
    serveVault([], ["paradise-golf"]);
    onCommand("list_failed_sessions", () => {
      throw "the sessions folder is unreadable";
    });

    renderInbox();

    expect(
      await screen.findByText(/the sessions folder is unreadable/),
    ).toBeInTheDocument();
    expect(screen.queryByText(/Nothing waiting/)).not.toBeInTheDocument();
  });

  it("surfaces a failed listing instead of claiming an empty inbox", async () => {
    onCommand("list_notes", () => {
      throw "the vault is unreadable";
    });
    onCommand("list_projects", () => ({ inbox_note_count: 0, projects: [] }));
    onCommand("list_failed_sessions", () => []);

    renderInbox();

    expect(await screen.findByText(/the vault is unreadable/)).toBeInTheDocument();
    expect(screen.queryByText(/Nothing waiting/)).not.toBeInTheDocument();
  });
});
