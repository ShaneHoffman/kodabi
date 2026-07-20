import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CommandPalette } from "./CommandPalette";
import { NavigationProvider } from "./NavigationProvider";
import { onCommand, resetTauriMocks } from "../test/tauri";

vi.mock("@tauri-apps/api/core", () => import("../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../test/tauri"));

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

  it("drops the headings when filtering and lets the rows speak for themselves", async () => {
    // A filtered list is a set of matches, not a table of contents, so the
    // per-row hint has to take the heading's place.
    const user = userEvent.setup();
    await renderPalette();

    await user.type(input(), "briarwood");

    expect(screen.queryByRole("group", { name: "Jump to" })).not.toBeInTheDocument();
    const [match] = screen.getAllByRole("option");
    expect(match).toHaveTextContent("briarwood-golf");
    expect(match).toHaveTextContent("Jump to");
  });

  it("walks the arrow keys straight across the section boundary", async () => {
    // The sections are visual only: the option ids stay one sequence over the
    // whole list, so the highlight must not stall or skip where they meet.
    const user = userEvent.setup();
    await renderPalette();
    const options = screen.getAllByRole("option");
    // Inbox, briarwood-golf | New note, Quick capture, Search notes, Settings
    expect(options).toHaveLength(6);
    expect(input()).toHaveAttribute("aria-activedescendant", options[0].id);

    await user.keyboard("{ArrowDown}{ArrowDown}");

    // The third row is the first of the next section, reached with no extra
    // keypress for the heading in between.
    expect(input()).toHaveAttribute("aria-activedescendant", options[2].id);
    expect(options[2]).toHaveTextContent("New note");
    expect(options[2]).toHaveAttribute("aria-selected", "true");
  });
});
