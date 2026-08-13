import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { VAULT_CHANGED_EVENT } from "../../events";
import {
  emitFromBackend,
  invoke,
  invokedCommands,
  onCommand,
  resetTauriMocks,
} from "../../test/tauri";
import { todayIsoDate, type NoteSummary } from "../../useNotes";
import type { Project } from "../../useProjects";
import { useVaultChangedBridge } from "../../useVaultChangedBridge";
import { CapturePipelineProvider } from "../providers/CapturePipelineProvider";
import { MainContent } from "../shell/MainContent";
import { NavigationProvider } from "../providers/NavigationProvider";
import { Dock } from "../shell/Dock";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

/** A `ProjectDto` row as `list_projects` returns it. */
function project(slug: string, noteCount = 0): Project {
  const split = slug.lastIndexOf("/");
  return {
    id: slug,
    slug,
    display_name: split === -1 ? slug : slug.slice(split + 1),
    parent: split === -1 ? null : slug.slice(0, split),
    note_count: noteCount,
    meeting_count: 0,
    last_activity: null,
  };
}

/** The reads the dock and the views behind it make. */
function serveVault(projects: Project[]): void {
  onCommand("list_projects", () => ({ inbox_note_count: 0, projects }));
  onCommand("list_notes", () => []);
  onCommand("list_failed_sessions", () => []);
  onCommand("capture_phase", () => ({
    phase: "idle",
    sources: { loopback: "off", microphone: "off" },
  }));
}

/** AppShell's vault:changed relay, so `emitFromBackend(VAULT_CHANGED_EVENT)`
 * reaches `useVaultQuery` the same way the Rust broadcast does. */
function VaultBridge() {
  useVaultChangedBridge();
  return null;
}

function renderShell() {
  return render(
    <NavigationProvider>
      <CapturePipelineProvider>
        <VaultBridge />
        <Dock />
        <MainContent />
      </CapturePipelineProvider>
    </NavigationProvider>,
  );
}

/** Renders the shell and navigates into the Growth project view. */
async function openGrowth(user: ReturnType<typeof userEvent.setup>) {
  serveVault([project("Growth", 2), project("Growth/Q4", 3)]);
  renderShell();
  await user.click(await screen.findByRole("button", { name: /Growth/ }));
  return screen.findByRole("button", { name: "Delete project" });
}

describe("ProjectView delete flow", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("confirms with the slug, the contained-note count, and the subtree", async () => {
    const user = userEvent.setup();
    const deleteButton = await openGrowth(user);

    await user.click(deleteButton);

    const dialog = screen.getByRole("dialog", { name: "Delete this project?" });
    // The title asks the question; the slug is named in its own strip, so the
    // user confirms a thing rather than a pronoun.
    expect(within(dialog).getByText("Growth")).toBeInTheDocument();
    // 2 direct notes + 3 in Growth/Q4: deletion takes the whole subtree.
    expect(
      within(dialog).getByText("Its 5 notes will move back to the Inbox."),
    ).toBeInTheDocument();
    expect(
      within(dialog).getByText("This also deletes 1 project inside it."),
    ).toBeInTheDocument();
    // Cancel is the primary control of a confirmation and holds initial focus.
    expect(within(dialog).getByRole("button", { name: "Cancel" })).toHaveFocus();
  });

  it("cancels without deleting", async () => {
    const user = userEvent.setup();
    const deleteButton = await openGrowth(user);

    await user.click(deleteButton);
    await user.click(screen.getByRole("button", { name: "Cancel" }));

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(invokedCommands()).not.toContain("delete_project");
  });

  it("deletes, lands in the Inbox, and the rows leave on the broadcast", async () => {
    const user = userEvent.setup();
    onCommand("delete_project", () => ({ slug: "Growth", moved_note_count: 5 }));
    const deleteButton = await openGrowth(user);

    await user.click(deleteButton);
    const dialog = screen.getByRole("dialog", { name: "Delete this project?" });
    await user.click(within(dialog).getByRole("button", { name: "Delete project" }));

    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    expect(invoke).toHaveBeenCalledWith("delete_project", { project: "Growth" });

    // The notes went to the Inbox, so that is where the view lands.
    expect(screen.getByRole("button", { name: /Inbox/ })).toHaveAttribute(
      "aria-current",
      "page",
    );

    // The backend's vault:changed broadcast is what drops the sidebar rows.
    onCommand("list_projects", () => ({ inbox_note_count: 5, projects: [] }));
    act(() => {
      emitFromBackend(VAULT_CHANGED_EVENT);
    });
    await waitFor(() => {
      expect(
        screen.queryByRole("button", { name: /Growth/ }),
      ).not.toBeInTheDocument();
    });
  });

  it("shows a failed delete inside the dialog and stays open", async () => {
    const user = userEvent.setup();
    onCommand("delete_project", () => {
      throw "folder is locked";
    });
    const deleteButton = await openGrowth(user);

    await user.click(deleteButton);
    const dialog = screen.getByRole("dialog", { name: "Delete this project?" });
    await user.click(within(dialog).getByRole("button", { name: "Delete project" }));

    expect(
      await within(dialog).findByText(
        "Couldn't delete the project: folder is locked",
      ),
    ).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "Delete this project?" })).toBeInTheDocument();
  });
});

describe("ProjectView rename affordance", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("offers rename beside the other header actions, opening a prefilled dialog", async () => {
    // The header is where the folder's identity lives (hue, name, slug), so
    // editing that identity belongs here too. The rename flow itself is
    // covered by RenameProjectDialog.test.tsx; this pins the wiring.
    const user = userEvent.setup();
    await openGrowth(user);

    const rename = screen.getByRole("button", { name: "Rename" });
    expect(rename).toHaveAttribute("aria-haspopup", "dialog");
    // Creation leads, the destructive action sits last.
    expect(screen.getByRole("button", { name: "New note" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete project" })).toBeInTheDocument();

    await user.click(rename);

    const dialog = screen.getByRole("dialog", { name: "Rename project" });
    expect(within(dialog).getByLabelText("Project name")).toHaveValue("Growth");
    expect(invokedCommands()).not.toContain("rename_project");
  });
});

describe("ProjectView index rows", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  /** A `NoteSummary` row as `list_notes` returns it for a project. */
  function note(overrides: Partial<NoteSummary> & { id: string; title: string }): NoteSummary {
    return {
      path: `Growth/${overrides.id}.md`,
      type: "note",
      project: "Growth",
      date: "2026-07-10T09:15:00-07:00",
      tags: [],
      source: "manual",
      confidence: 0.9,
      snippet: "",
      // A project listing never carries a guess: the note already has a home.
      guess: null,
      ...overrides,
    };
  }

  async function openGrowthWith(notes: NoteSummary[]) {
    const user = userEvent.setup();
    onCommand("list_projects", () => ({
      inbox_note_count: 0,
      projects: [project("Growth", notes.length)],
    }));
    onCommand("list_notes", (args) => (args?.project === "Growth" ? notes : []));
    onCommand("list_failed_sessions", () => []);
    onCommand("capture_phase", () => ({
      phase: "idle",
      sources: { loopback: "off", microphone: "off" },
    }));
    renderShell();
    await user.click(await screen.findByRole("button", { name: /Growth/ }));
  }

  it("leads a row's meta with the kind when it is not a plain note", async () => {
    await openGrowthWith([
      note({ id: "n_g7h8i9", title: "Irrigation contractor comparison", type: "chat" }),
    ]);

    // Kind first, then the day: the row stacks, so its meta line is a caption
    // under a title rather than a column to scan down.
    expect(await screen.findByText("chat · 2026-07-10")).toBeInTheDocument();
  });

  it("says nothing about kind on a plain note's row", async () => {
    await openGrowthWith([note({ id: "n_a1b2c3", title: "Quarterly planning" })]);

    expect(await screen.findByText("2026-07-10")).toBeInTheDocument();
  });

  it("previews the body under the title, and draws nothing when there is none", async () => {
    await openGrowthWith([
      note({
        id: "n_a1b2c3",
        title: "Quarterly planning",
        snippet: "Three fairways need reseeding before the club championship.",
      }),
      note({ id: "n_d4e5f6", title: "Range netting quote" }),
    ]);

    expect(
      await screen.findByText("Three fairways need reseeding before the club championship."),
    ).toBeInTheDocument();
    // The second row carries an empty snippet, so it renders the title and the
    // meta and stops — not a third, blank line holding its height open.
    const bare = screen.getByRole("button", { name: /Range netting quote/ });
    expect(bare.textContent).toBe("Range netting quote2026-07-10");
  });

  it("groups this month's notes under Recent and older ones under their month", async () => {
    // Dated off the real clock rather than a frozen one: the grouping's own
    // boundary cases are unit-tested in noteGroups.test.ts, and what this
    // pins is that the view renders the labels at all.
    await openGrowthWith([
      note({ id: "n_a1b2c3", title: "Quarterly planning", date: todayIsoDate() }),
      note({ id: "n_d4e5f6", title: "Range netting quote", date: "2025-01-15" }),
    ]);

    expect(await screen.findByText("Recent")).toBeInTheDocument();
    expect(screen.getByText("January 2025")).toBeInTheDocument();
  });

  it("heads the view with the project's name and its slug, not the raw path", async () => {
    await openGrowthWith([note({ id: "n_a1b2c3", title: "Quarterly planning" })]);

    expect(
      await screen.findByRole("heading", { name: "Growth", level: 2 }),
    ).toBeInTheDocument();
    // The count and the slug share the header's data line: the folder keeps the
    // address every other surface calls it by.
    expect(screen.getByText("1 note · Growth")).toBeInTheDocument();
  });
});
