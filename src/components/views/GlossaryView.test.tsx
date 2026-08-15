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
import type { GlossaryTerm } from "../../useGlossary";
import { useNavigation } from "../../useNavigation";
import type { Project } from "../../useProjects";
import { useVaultChangedBridge } from "../../useVaultChangedBridge";
import { CapturePipelineProvider } from "../providers/CapturePipelineProvider";
import { MainContent } from "../shell/MainContent";
import { NavigationProvider } from "../providers/NavigationProvider";
import { Dock } from "../shell/Dock";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

/** A `ProjectDto` row as `list_projects` returns it. */
function project(slug: string): Project {
  return {
    id: slug,
    slug,
    display_name: slug,
    parent: null,
    note_count: 0,
    meeting_count: 0,
    last_activity: null,
  };
}

function term(
  name: string,
  definition: string,
  aliases: string[] = [],
): GlossaryTerm {
  return { term: name, definition, aliases };
}

/**
 * The reads the shell makes, plus one glossary per scope. `vaultTerms` is the
 * vault-wide glossary at the knowledge-base root (`project: null`); `projectTerms`
 * is Growth's own.
 */
function serveVault(vaultTerms: GlossaryTerm[], projectTerms: GlossaryTerm[] = []): void {
  onCommand("list_projects", () => ({
    inbox_note_count: 0,
    projects: [project("Growth")],
  }));
  onCommand("list_notes", () => []);
  onCommand("list_failed_sessions", () => []);
  onCommand("capture_phase", () => ({
    phase: "idle",
    sources: { loopback: "off", microphone: "off" },
  }));
  onCommand("get_settings", () => {
    throw "settings not served in this test";
  });
  onCommand("list_glossary_terms", (args) => ({
    project: args?.project ?? null,
    terms: args?.project ? projectTerms : vaultTerms,
  }));
}

/** AppShell's vault:changed relay, so `emitFromBackend(VAULT_CHANGED_EVENT)`
 * reaches `useVaultQuery` the same way the Rust broadcast does. */
function VaultBridge() {
  useVaultChangedBridge();
  return null;
}

/**
 * Stands in for Settings' "Manage" row, which is the vault glossary's real
 * entry point but lives behind a view with its own provider stack. That the
 * row navigates here is pinned in SettingsView.test.tsx; these tests are about
 * what the destination does, so they take the same navigation by the shortest
 * road.
 */
function VaultGlossaryLink() {
  const { navigate } = useNavigation();
  return (
    <button type="button" onClick={() => navigate({ kind: "glossary", slug: null })}>
      Open vault glossary
    </button>
  );
}

function renderShell() {
  return render(
    <NavigationProvider>
      <CapturePipelineProvider>
        <VaultBridge />
        <Dock />
        <VaultGlossaryLink />
        <MainContent />
      </CapturePipelineProvider>
    </NavigationProvider>,
  );
}

/** Renders the shell and lands on the vault-wide glossary. */
async function openVaultGlossary(user: ReturnType<typeof userEvent.setup>) {
  renderShell();
  await user.click(screen.getByRole("button", { name: "Open vault glossary" }));
}

/** Renders the shell and navigates into Growth's own glossary. */
async function openProjectGlossary(user: ReturnType<typeof userEvent.setup>) {
  renderShell();
  await user.click(await screen.findByRole("button", { name: /Growth/ }));
  await user.click(await screen.findByRole("button", { name: "Growth project actions" }));
  await user.click(await screen.findByRole("menuitem", { name: "Glossary" }));
}

describe("GlossaryView listing", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("lists a term with its definition and aliases", async () => {
    const user = userEvent.setup();
    serveVault([], [term("TeeTrack", "Tee-sheet vendor.", ["t-track", "tee-track"])]);

    await openProjectGlossary(user);

    expect(await screen.findByText("TeeTrack")).toBeInTheDocument();
    expect(screen.getByText("Tee-sheet vendor.")).toBeInTheDocument();
    // Aliases read as data, joined rather than one line each.
    expect(screen.getByText("also: t-track · tee-track")).toBeInTheDocument();
    expect(screen.getByText("1 term · Growth")).toBeInTheDocument();
  });

  it("draws no alias line for a term without any", async () => {
    const user = userEvent.setup();
    serveVault([term("MERIDIAN", "A systems-migration project.")]);

    await openVaultGlossary(user);

    expect(await screen.findByText("MERIDIAN")).toBeInTheDocument();
    expect(screen.queryByText(/^also:/)).not.toBeInTheDocument();
  });

  it("says what each scope is for, since only one biases transcription", async () => {
    const user = userEvent.setup();
    serveVault([], []);

    await openVaultGlossary(user);
    expect(
      await screen.findByText(/These terms bias transcription for every capture/),
    ).toBeInTheDocument();

    // The empty states differ too: the advice depends on what the glossary does.
    expect(
      screen.getByText("No terms yet. Add the names transcription keeps getting wrong."),
    ).toBeInTheDocument();
  });

  it("tells a project glossary apart from the vault one", async () => {
    const user = userEvent.setup();
    serveVault([], []);

    await openProjectGlossary(user);

    expect(
      await screen.findByText(/Transcription is biased by the vault glossary/),
    ).toBeInTheDocument();
    expect(
      screen.getByText("No terms yet. Terms added here help route notes to this project."),
    ).toBeInTheDocument();
  });

  it("surfaces a hand-edited file it cannot read, and says what to do", async () => {
    const user = userEvent.setup();
    serveVault([]);
    onCommand("list_glossary_terms", () => {
      throw 'glossary at _glossary.yml has duplicate term "MERIDIAN"';
    });

    await openVaultGlossary(user);

    expect(
      await screen.findByText(
        /Couldn't load the glossary: glossary at _glossary.yml has duplicate term "MERIDIAN"/,
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Fix the file in a text editor, then reopen this view."),
    ).toBeInTheDocument();
  });
});

describe("GlossaryView add", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("adds a term, splitting the aliases, and the row lands on the broadcast", async () => {
    const user = userEvent.setup();
    serveVault([]);
    onCommand("add_glossary_term", () => term("MERIDIAN", "A systems-migration project."));
    await openVaultGlossary(user);

    await user.click(screen.getByRole("button", { name: "Add term" }));
    const dialog = screen.getByRole("dialog", { name: "Add term" });
    await user.type(within(dialog).getByLabelText("Term"), "MERIDIAN");
    await user.type(
      within(dialog).getByLabelText("Definition"),
      "A systems-migration project.",
    );
    await user.type(within(dialog).getByLabelText("Aliases"), "meridian, mer-idian");
    await user.click(within(dialog).getByRole("button", { name: "Add term" }));

    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    // The scope travels inside the input struct, and the comma-separated line
    // arrives as a list.
    expect(invoke).toHaveBeenCalledWith("add_glossary_term", {
      input: {
        project: null,
        term: "MERIDIAN",
        definition: "A systems-migration project.",
        aliases: ["meridian", "mer-idian"],
      },
    });

    // The backend's vault:changed broadcast is what brings the row in.
    onCommand("list_glossary_terms", () => ({
      project: null,
      terms: [term("MERIDIAN", "A systems-migration project.")],
    }));
    act(() => {
      emitFromBackend(VAULT_CHANGED_EVENT);
    });
    expect(await screen.findByText("MERIDIAN")).toBeInTheDocument();
  });

  it("carries the project scope when adding from a project glossary", async () => {
    const user = userEvent.setup();
    serveVault([], []);
    onCommand("add_glossary_term", () => term("TeeTrack", "Tee-sheet vendor."));
    await openProjectGlossary(user);

    await user.click(screen.getByRole("button", { name: "Add term" }));
    const dialog = screen.getByRole("dialog", { name: "Add term" });
    await user.type(within(dialog).getByLabelText("Term"), "TeeTrack");
    await user.type(within(dialog).getByLabelText("Definition"), "Tee-sheet vendor.");
    await user.click(within(dialog).getByRole("button", { name: "Add term" }));

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("add_glossary_term", {
        input: {
          project: "Growth",
          term: "TeeTrack",
          definition: "Tee-sheet vendor.",
          aliases: [],
        },
      });
    });
  });

  it("shows a duplicate on the term field and keeps the dialog open", async () => {
    const user = userEvent.setup();
    serveVault([term("MERIDIAN", "First.")]);
    onCommand("add_glossary_term", () => {
      throw 'glossary term "MERIDIAN" already exists';
    });
    await openVaultGlossary(user);

    await user.click(screen.getByRole("button", { name: "Add term" }));
    const dialog = screen.getByRole("dialog", { name: "Add term" });
    await user.type(within(dialog).getByLabelText("Term"), "MERIDIAN");
    await user.type(within(dialog).getByLabelText("Definition"), "Second.");
    await user.click(within(dialog).getByRole("button", { name: "Add term" }));

    expect(
      await within(dialog).findByText('glossary term "MERIDIAN" already exists'),
    ).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "Add term" })).toBeInTheDocument();
    // The rejection is bound to the field it is about.
    expect(within(dialog).getByLabelText("Term")).toHaveAttribute("aria-invalid", "true");
  });
});

describe("GlossaryView edit", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("opens prefilled and names the entry being replaced when renaming", async () => {
    const user = userEvent.setup();
    serveVault([term("Meridan", "Misspelled on creation.", ["mer"])]);
    onCommand("update_glossary_term", () => term("MERIDIAN", "Fixed.", ["mer"]));
    await openVaultGlossary(user);

    await user.click(await screen.findByRole("button", { name: "Edit" }));
    const dialog = screen.getByRole("dialog", { name: "Edit term" });
    // Prefilled from the row, aliases rejoined into the one line they came from.
    expect(within(dialog).getByLabelText("Term")).toHaveValue("Meridan");
    expect(within(dialog).getByLabelText("Definition")).toHaveValue(
      "Misspelled on creation.",
    );
    expect(within(dialog).getByLabelText("Aliases")).toHaveValue("mer");

    await user.clear(within(dialog).getByLabelText("Term"));
    await user.type(within(dialog).getByLabelText("Term"), "MERIDIAN");
    await user.clear(within(dialog).getByLabelText("Definition"));
    await user.type(within(dialog).getByLabelText("Definition"), "Fixed.");
    await user.click(within(dialog).getByRole("button", { name: "Save term" }));

    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    // `original_term` is the old spelling: without it the backend would have
    // no way to tell a rename from a second term.
    expect(invoke).toHaveBeenCalledWith("update_glossary_term", {
      input: {
        project: null,
        original_term: "Meridan",
        term: "MERIDIAN",
        definition: "Fixed.",
        aliases: ["mer"],
      },
    });
  });
});

describe("GlossaryView delete", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("confirms with the term named, and cancels without deleting", async () => {
    const user = userEvent.setup();
    serveVault([term("MERIDIAN", "A systems-migration project.")]);
    await openVaultGlossary(user);

    await user.click(await screen.findByRole("button", { name: "Delete" }));
    const dialog = screen.getByRole("dialog", { name: "Delete this term?" });
    expect(within(dialog).getByText("MERIDIAN")).toBeInTheDocument();
    expect(
      within(dialog).getByText("Transcription stops recognizing it on future captures."),
    ).toBeInTheDocument();
    // Cancel is the primary control of a confirmation and takes initial focus,
    // which base-ui moves a tick after the dialog mounts.
    await waitFor(() => {
      expect(within(dialog).getByRole("button", { name: "Cancel" })).toHaveFocus();
    });

    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(invokedCommands()).not.toContain("delete_glossary_term");
  });

  it("deletes the confirmed term", async () => {
    const user = userEvent.setup();
    serveVault([term("MERIDIAN", "A systems-migration project.")]);
    onCommand("delete_glossary_term", () => term("MERIDIAN", "A systems-migration project."));
    await openVaultGlossary(user);

    await user.click(await screen.findByRole("button", { name: "Delete" }));
    const dialog = screen.getByRole("dialog", { name: "Delete this term?" });
    await user.click(within(dialog).getByRole("button", { name: "Delete term" }));

    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    expect(invoke).toHaveBeenCalledWith("delete_glossary_term", {
      project: null,
      term: "MERIDIAN",
    });
  });

  it("shows a failed delete inside the dialog and stays open", async () => {
    const user = userEvent.setup();
    serveVault([term("MERIDIAN", "A systems-migration project.")]);
    onCommand("delete_glossary_term", () => {
      throw "file is read-only";
    });
    await openVaultGlossary(user);

    await user.click(await screen.findByRole("button", { name: "Delete" }));
    const dialog = screen.getByRole("dialog", { name: "Delete this term?" });
    await user.click(within(dialog).getByRole("button", { name: "Delete term" }));

    expect(
      await within(dialog).findByText("Couldn't delete the term: file is read-only"),
    ).toBeInTheDocument();
    // And the half that says where to go from here (docs/DESIGN_SYSTEM.md §3).
    expect(
      within(dialog).getByText("The term is still in your glossary. You can try again or cancel."),
    ).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "Delete this term?" })).toBeInTheDocument();
  });

  it("says what a project term's deletion costs, which is not transcription", async () => {
    const user = userEvent.setup();
    serveVault([], [term("TeeTrack", "Tee-sheet vendor.")]);
    await openProjectGlossary(user);

    await user.click(await screen.findByRole("button", { name: "Delete" }));

    expect(
      within(screen.getByRole("dialog", { name: "Delete this term?" })).getByText(
        "Routing and project context stop seeing it.",
      ),
    ).toBeInTheDocument();
  });
});
