import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CommandPalette } from "./CommandPalette";
import { NavigationProvider } from "../providers/NavigationProvider";
import { onCommand, resetTauriMocks } from "../../test/tauri";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

function serveVault(): void {
  onCommand("list_projects", () => ({
    inbox_note_count: 0,
    projects: [
      {
        id: "briarwood-golf",
        slug: "briarwood-golf",
        display_name: "Briarwood Golf",
        parent: null,
        note_count: 2,
        meeting_count: 0,
      },
    ],
  }));
  onCommand("list_failed_sessions", () => []);
}

async function renderPalette() {
  const result = render(
    <NavigationProvider>
      <CommandPalette onClose={() => {}} />
    </NavigationProvider>,
  );
  await screen.findByRole("option", { name: "briarwood-golf" });
  return result;
}

function input(): HTMLElement {
  return screen.getByRole("combobox");
}

describe("CommandPalette sections", () => {
  beforeEach(() => {
    resetTauriMocks();
    serveVault();
  });

  it("heads the two halves of an unfiltered list", async () => {
    // Somewhere to go and something to do are different questions; flat, the
    // only thing marking the boundary was a hint repeated on every row.
    await renderPalette();

    expect(screen.getByRole("group", { name: "Jump to" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Actions" })).toBeInTheDocument();
  });

  it("says nothing per row that its own heading already says", async () => {
    await renderPalette();

    expect(
      screen.getByRole("option", { name: "briarwood-golf" }),
    ).toBeInTheDocument();
  });

  it("drops the headings when filtering and marks the row Enter would run", async () => {
    // A filtered list is a set of matches, not a table of contents. What
    // replaces the heading is not a restatement of it — it is the one thing
    // worth saying about the top row, which is that Enter runs it.
    const user = userEvent.setup();
    await renderPalette();

    await user.type(input(), "briarwood");

    expect(screen.queryByRole("group", { name: "Jump to" })).not.toBeInTheDocument();
    const [match] = screen.getAllByRole("option");
    expect(match).toHaveTextContent("briarwood-golf");
    expect(match).toHaveTextContent("↵");
  });

  it("teaches the global shortcut of a command that has one, and invents none", async () => {
    // The palette is where people learn the bindings, so a hint here has to be
    // a real accelerator the backend registered. Every other row shows nothing
    // rather than a label dressed up as a key.
    await renderPalette();

    expect(screen.getByRole("option", { name: /Quick capture/ })).toHaveTextContent(
      "Ctrl+Alt+Space",
    );
    expect(screen.getByRole("option", { name: /New note/ })).not.toHaveTextContent(
      "Ctrl",
    );
  });

  it("offers the needs-attention jump while only dismissed captures remain", async () => {
    // The sidebar row hides once everything is dismissed, so this jump is the
    // one way back to the dismissed shelf — it keys on the full listing,
    // dismissed included, or dismissal would stop being reversible.
    serveVault();
    onCommand("list_failed_sessions", () => [
      {
        path: "sessions/2026-07-01T10-00-00Z-team-sync.jsonl",
        file_name: "2026-07-01T10-00-00Z-team-sync.jsonl",
        slug: "team-sync",
        captured_at: "2026-07-01T10:00:00Z",
        dismissed: true,
      },
    ]);

    await renderPalette();

    expect(
      await screen.findByRole("option", { name: "Needs attention" }),
    ).toBeInTheDocument();
  });

  it("walks the arrow keys straight across the section boundary", async () => {
    // The sections are visual only: the option ids stay one sequence over the
    // whole list, so the highlight must not stall or skip where they meet.
    const user = userEvent.setup();
    await renderPalette();
    const options = screen.getAllByRole("option");
    // Inbox, briarwood-golf | New note, Quick capture, Search notes,
    // Open chat, Open terminal, Settings
    expect(options).toHaveLength(8);
    expect(input()).toHaveAttribute("aria-activedescendant", options[0].id);

    await user.keyboard("{ArrowDown}{ArrowDown}");

    // The third row is the first of the next section, reached with no extra
    // keypress for the heading in between.
    expect(input()).toHaveAttribute("aria-activedescendant", options[2].id);
    expect(options[2]).toHaveTextContent("New note");
    expect(options[2]).toHaveAttribute("aria-selected", "true");
  });
});
