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
import type { Project } from "../../useProjects";
import { useVaultChangedBridge } from "../../useVaultChangedBridge";
import { CapturePipelineProvider } from "../providers/CapturePipelineProvider";
import { MainContent } from "../shell/MainContent";
import { NavigationProvider } from "../providers/NavigationProvider";
import { Sidebar } from "../shell/Sidebar";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

/** A `ProjectDto` row as `list_projects` / `create_project` return it. */
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

/** The reads the sidebar and the views behind it make. */
function serveVault(projects: Project[] = []): void {
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
        <Sidebar onOpenPalette={() => {}} />
        <MainContent />
      </CapturePipelineProvider>
    </NavigationProvider>,
  );
}

describe("CreateProjectDialog", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("opens from the sidebar with the name field focused", async () => {
    const user = userEvent.setup();
    serveVault();
    renderShell();

    await user.click(await screen.findByRole("button", { name: "New project" }));

    const dialog = screen.getByRole("dialog", { name: "New project" });
    expect(within(dialog).getByLabelText("Project name")).toHaveFocus();
  });

  it("creates the trimmed name, navigates to it, and the sidebar refreshes on the broadcast", async () => {
    const user = userEvent.setup();
    serveVault();
    onCommand("create_project", () => project("Ops"));
    renderShell();

    await user.click(await screen.findByRole("button", { name: "New project" }));
    await user.type(screen.getByLabelText("Project name"), "  Ops  ");
    await user.click(screen.getByRole("button", { name: "Create" }));

    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    expect(invoke).toHaveBeenCalledWith("create_project", { project: "Ops" });

    // Navigated straight to the echoed canonical slug.
    expect(
      await screen.findByRole("heading", { name: "Ops" }),
    ).toBeInTheDocument();

    // The sidebar row arrives when the backend's vault:changed broadcast
    // triggers a refetch — no frontend wiring of its own.
    onCommand("list_projects", () => ({
      inbox_note_count: 0,
      projects: [project("Ops")],
    }));
    act(() => {
      emitFromBackend(VAULT_CHANGED_EVENT);
    });
    expect(await screen.findByRole("button", { name: /Ops/ })).toBeInTheDocument();
  });

  it("surfaces a backend rejection on the field and stays open", async () => {
    const user = userEvent.setup();
    serveVault();
    onCommand("create_project", () => {
      throw 'project "Ops" already exists';
    });
    renderShell();

    await user.click(await screen.findByRole("button", { name: "New project" }));
    await user.type(screen.getByLabelText("Project name"), "Ops");
    await user.click(screen.getByRole("button", { name: "Create" }));

    expect(
      await screen.findByText('project "Ops" already exists'),
    ).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "New project" })).toBeInTheDocument();
  });

  it("closes on Escape without creating anything", async () => {
    const user = userEvent.setup();
    serveVault();
    renderShell();

    await user.click(await screen.findByRole("button", { name: "New project" }));
    await user.keyboard("{Escape}");

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(invokedCommands()).not.toContain("create_project");
  });
});
