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
import { CreateProjectDialog } from "./CreateProjectDialog";
import { CapturePipelineProvider } from "../providers/CapturePipelineProvider";
import { MainContent } from "../shell/MainContent";
import { NavigationProvider } from "../providers/NavigationProvider";
import { Dock } from "../shell/Dock";

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

/** The reads the dock and the views behind it make. */
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
        <Dock />
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
    // base-ui hands focus off after the popup is in the document, so this is
    // awaited rather than read on the same tick as the click.
    await waitFor(() => {
      expect(within(dialog).getByLabelText("Project name")).toHaveFocus();
    });
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
      throw 'A project named "Ops" already exists. Pick a different name.';
    });
    renderShell();

    await user.click(await screen.findByRole("button", { name: "New project" }));
    await user.type(screen.getByLabelText("Project name"), "Ops");
    await user.click(screen.getByRole("button", { name: "Create" }));

    expect(
      await screen.findByText('A project named "Ops" already exists. Pick a different name.'),
    ).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "New project" })).toBeInTheDocument();
  });

  it("shows the validation rule a typed name broke, verbatim", async () => {
    // Half of the leak pin for a dialog. Validation detail is the ONE thing the
    // boundary passes through unchanged (`user_errors::note_error` →
    // `user_sentence`), because it describes what the user typed: no generic
    // sentence could tell them which rule they broke. A change that started
    // swallowing these in favour of a house sentence would fail here.
    const user = userEvent.setup();
    serveVault();
    onCommand("create_project", () => {
      throw 'Project segment "aux" is a reserved Windows device name.';
    });
    renderShell();

    await user.click(await screen.findByRole("button", { name: "New project" }));
    await user.type(screen.getByLabelText("Project name"), "aux");
    await user.click(screen.getByRole("button", { name: "Create" }));

    expect(
      await screen.findByText('Project segment "aux" is a reserved Windows device name.'),
    ).toBeInTheDocument();
  });

  it("shows its own sentence, not the exception, when the rejection is not backend copy", async () => {
    // The other half. A non-string rejection did not come from the command
    // boundary, so it carries developer text (a stack, a class name) that
    // docs/DESIGN_SYSTEM.md §3 forbids on screen: `backendCopy` logs it and the
    // dialog's own sentence renders instead.
    const user = userEvent.setup();
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    serveVault();
    onCommand("create_project", () => {
      throw new TypeError("projects.map is not a function");
    });
    renderShell();

    await user.click(await screen.findByRole("button", { name: "New project" }));
    await user.type(screen.getByLabelText("Project name"), "Ops");
    await user.click(screen.getByRole("button", { name: "Create" }));

    expect(
      await screen.findByText(
        "Couldn't finish creating the project. Your notes are untouched; try again.",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/TypeError|is not a function/)).toBeNull();
    // Not swallowed: the console is the only record there is.
    expect(logged).toHaveBeenCalled();
    logged.mockRestore();
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

  it("closes on a scrim press without creating anything", async () => {
    const user = userEvent.setup();
    serveVault();
    renderShell();

    await user.click(await screen.findByRole("button", { name: "New project" }));
    await user.click(screen.getByTestId("dialog-scrim"));

    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    expect(invokedCommands()).not.toContain("create_project");
  });

  it("hands the created project to onCreated instead of navigating", async () => {
    // The Inbox's File menu opens this dialog mid-errand: the user is filing
    // a note, and navigating away would answer half the request and leave the
    // note where it was. The caller that knows the errand takes over.
    const user = userEvent.setup();
    const onCreated = vi.fn();
    serveVault();
    onCommand("create_project", () => project("Ops"));
    render(
      <NavigationProvider>
        <CreateProjectDialog onClose={() => {}} onCreated={onCreated} />
      </NavigationProvider>,
    );

    await user.type(screen.getByLabelText("Project name"), "Ops");
    await user.click(screen.getByRole("button", { name: "Create" }));

    await waitFor(() => {
      expect(onCreated).toHaveBeenCalledWith(project("Ops"));
    });
    // Nothing moved: the dialog closed onto the surface it was opened from.
    expect(screen.queryByRole("heading", { name: "Ops" })).not.toBeInTheDocument();
  });
});
