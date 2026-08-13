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
import { Dock } from "../shell/Dock";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

/** A `ProjectDto` row as `list_projects` / `rename_project` return it. */
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

/** Renders the shell and opens the rename dialog on the Growth project. */
async function openRename(user: ReturnType<typeof userEvent.setup>) {
  serveVault([project("Growth", 2)]);
  render(
    <NavigationProvider>
      <CapturePipelineProvider>
        <VaultBridge />
        <Dock />
        <MainContent />
      </CapturePipelineProvider>
    </NavigationProvider>,
  );
  await user.click(await screen.findByRole("button", { name: /Growth/ }));
  await user.click(await screen.findByRole("button", { name: "Rename" }));
  return screen.getByRole("dialog", { name: "Rename project" });
}

describe("RenameProjectDialog", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("opens on the field, prefilled with the current slug", async () => {
    const user = userEvent.setup();
    const dialog = await openRename(user);

    const field = within(dialog).getByLabelText("Project name");
    // The full slug, not the leaf: the slug is the project's identity, and
    // editing its parent part is how a project moves.
    expect(field).toHaveValue("Growth");
    // base-ui hands focus off after the popup is in the document, so this is
    // awaited rather than read on the same tick as the click.
    await waitFor(() => {
      expect(field).toHaveFocus();
    });
  });

  it("cannot submit an unchanged or blank name", async () => {
    const user = userEvent.setup();
    const dialog = await openRename(user);

    const submit = within(dialog).getByRole("button", { name: "Rename" });
    expect(submit).toBeDisabled();

    await user.clear(within(dialog).getByLabelText("Project name"));
    expect(submit).toBeDisabled();

    await user.type(within(dialog).getByLabelText("Project name"), "Acme");
    expect(submit).toBeEnabled();
  });

  it("stays submittable for a case-only change", async () => {
    // Backends that canonicalize casing would fold this into a no-op; ours
    // honours it, so the frontend must not be the thing that blocks it. The
    // comparison is exact rather than case-insensitive for exactly this.
    const user = userEvent.setup();
    const dialog = await openRename(user);
    const field = within(dialog).getByLabelText("Project name");

    await user.clear(field);
    await user.type(field, "growth");

    expect(within(dialog).getByRole("button", { name: "Rename" })).toBeEnabled();
  });

  it("renames the trimmed name and navigates to the echoed canonical slug", async () => {
    const user = userEvent.setup();
    // The echo differs from what was typed: the backend adopts an existing
    // parent's on-disk casing, and the view must follow the slug that exists
    // rather than the one the user guessed.
    onCommand("rename_project", () => project("Archive/2026", 2));
    const dialog = await openRename(user);

    const field = within(dialog).getByLabelText("Project name");
    await user.clear(field);
    await user.type(field, "  archive/2026  ");
    await user.click(within(dialog).getByRole("button", { name: "Rename" }));

    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    expect(invoke).toHaveBeenCalledWith("rename_project", {
      project: "Growth",
      newProject: "archive/2026",
    });

    // Landed on the canonical slug, not the typed one. Without this the view
    // would still name a folder that no longer exists.
    expect(await screen.findByRole("heading", { name: "2026" })).toBeInTheDocument();

    // The sidebar follows on the backend's broadcast, with no wiring here.
    onCommand("list_projects", () => ({
      inbox_note_count: 0,
      projects: [project("Archive"), project("Archive/2026", 2)],
    }));
    act(() => {
      emitFromBackend(VAULT_CHANGED_EVENT);
    });
    expect(await screen.findByRole("button", { name: /2026/ })).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: /Growth/ })).not.toBeInTheDocument();
    });
  });

  it("surfaces a backend rejection on the field and stays open", async () => {
    const user = userEvent.setup();
    onCommand("rename_project", () => {
      throw 'project "Acme" already exists';
    });
    const dialog = await openRename(user);

    const field = within(dialog).getByLabelText("Project name");
    await user.clear(field);
    await user.type(field, "Acme");
    await user.click(within(dialog).getByRole("button", { name: "Rename" }));

    expect(
      await within(dialog).findByText('project "Acme" already exists'),
    ).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "Rename project" })).toBeInTheDocument();
    // The rejected name is still in the field, so it can be corrected rather
    // than retyped.
    expect(within(dialog).getByLabelText("Project name")).toHaveValue("Acme");
  });

  it("closes on Escape without renaming anything", async () => {
    const user = userEvent.setup();
    await openRename(user);

    await user.keyboard("{Escape}");

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(invokedCommands()).not.toContain("rename_project");
  });

  it("closes on a scrim press without renaming anything", async () => {
    const user = userEvent.setup();
    await openRename(user);

    await user.click(screen.getByTestId("dialog-scrim"));

    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    expect(invokedCommands()).not.toContain("rename_project");
  });
});
