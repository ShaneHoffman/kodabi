import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import { invokedCommands, onCommand, resetTauriMocks } from "../../test/tauri";
import type { Project } from "../../useProjects";
import type { SearchHit, SearchResults } from "../../useSearch";
import { CapturePipelineProvider } from "../providers/CapturePipelineProvider";
import { NavigationProvider } from "../providers/NavigationProvider";
import { MainContent } from "../shell/MainContent";
import { Dock } from "../shell/Dock";
import { vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

/** The mark sentinels `search_notes` wraps matches in (index_cmds.rs). Spelled
 * as escapes here for the same reason they are in useSearch.ts: a literal
 * private-use character is invisible in a diff. */
const MARK_OPEN = "\uE000";
const MARK_CLOSE = "\uE001";

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

function hit(overrides: Partial<SearchHit> = {}): SearchHit {
  return {
    id: "n_a1b2c3",
    path: "Briarwood Golf/tournament.md",
    title: "Fall tournament sponsor list",
    type: "meeting",
    project: "Briarwood Golf",
    date: "2026-07-29",
    tags: [],
    source: "manual",
    confidence: null,
    score: 0.5,
    rank: 1,
    snippet: `the ${MARK_OPEN}tournament${MARK_CLOSE} sponsors are set`,
    ...overrides,
  };
}

function results(hits: SearchHit[], totalEstimate: number | null = hits.length): SearchResults {
  return {
    hits,
    page: { has_more: false, next_cursor: null, total_estimate: totalEstimate },
  };
}

/** The reads the dock and the views behind it make. */
function serveVault(): void {
  onCommand("list_projects", () => ({
    inbox_note_count: 0,
    projects: [project("Briarwood Golf")],
  }));
  onCommand("list_notes", () => []);
  onCommand("list_failed_sessions", () => []);
  onCommand("capture_phase", () => ({
    phase: "idle",
    sources: { loopback: "off", microphone: "off" },
  }));
}

function renderShell() {
  return render(
    <NavigationProvider>
      <CapturePipelineProvider>
        <Dock />
        <MainContent />
      </CapturePipelineProvider>
    </NavigationProvider>,
  );
}

/** Opens the Search view from the dock and returns its field. */
async function openSearch(user: ReturnType<typeof userEvent.setup>) {
  serveVault();
  renderShell();
  await user.click(await screen.findByRole("button", { name: "Search" }));
  return screen.findByRole("combobox", { name: "Search notes" });
}

describe("SearchView", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("shows the idle hint and searches nothing under two characters", async () => {
    const user = userEvent.setup();
    onCommand("search_notes", () => results([]));
    const field = await openSearch(user);

    expect(
      screen.getByText("Searches every note in full, title and body."),
    ).toBeInTheDocument();

    await user.type(field, "t");
    // The debounce has to be allowed to fire, or this would pass on timing.
    await new Promise((resolve) => setTimeout(resolve, 400));
    expect(invokedCommands()).not.toContain("search_notes");
  });

  it("asks the index for one page and renders the hits it returns", async () => {
    const user = userEvent.setup();
    const calls: unknown[] = [];
    onCommand("search_notes", (args) => {
      calls.push(args);
      return results([hit(), hit({ id: "n_d4e5f6", path: "Inbox/call.md", title: "Call with Dana" })]);
    });
    const field = await openSearch(user);

    await user.type(field, "tournament");

    const options = await screen.findAllByRole("option");
    expect(options).toHaveLength(2);
    expect(calls[calls.length - 1]).toEqual({ params: { query: "tournament", limit: 50 } });
    // `textContent`, not a text matcher: the marked run splits the title
    // across elements, which is the whole point of the row.
    expect(options[0].textContent).toContain("Fall tournament sponsor list");
    expect(screen.getByText("2 hits")).toBeInTheDocument();
  });

  it("marks the backend's matches in the snippet and the query's terms in the title", async () => {
    const user = userEvent.setup();
    onCommand("search_notes", () => results([hit()]));
    const field = await openSearch(user);

    await user.type(field, "tournament");
    const option = await screen.findByRole("option");

    // Exactly the delimited run is marked, and the sentinels never reach the DOM.
    const marks = within(option).getAllByText("tournament", {
      selector: "mark",
    });
    expect(marks).toHaveLength(2); // the title (frontend) and the snippet (backend)
    expect(option.textContent).not.toContain(MARK_OPEN);
    expect(option.textContent).toContain("the tournament sponsors are set");
  });

  it("leads the meta line with the folder in its hue, and inbox untinted", async () => {
    const user = userEvent.setup();
    onCommand("search_notes", () =>
      results([hit(), hit({ id: "n_d4e5f6", path: "Inbox/call.md", project: null })]),
    );
    const field = await openSearch(user);

    await user.type(field, "tournament");
    const options = await screen.findAllByRole("option");

    // The hue is a token class, not a literal — which one is `folderHue`'s call.
    const folder = within(options[0]).getByText("Briarwood Golf");
    expect(folder.className).toMatch(/^text-(coral|cobalt|teal|plum)$/);
    expect(within(options[1]).getByText("inbox").className).toBe("");
  });

  it("says what did not match, quoting the query it actually searched", async () => {
    const user = userEvent.setup();
    onCommand("search_notes", () => results([]));
    const field = await openSearch(user);

    await user.type(field, "zzzz");

    expect(await screen.findByText('Nothing matches “zzzz”.')).toBeInTheDocument();
    expect(screen.queryByRole("option")).not.toBeInTheDocument();
  });

  it("walks the results with the arrow keys and opens the walked one with Enter", async () => {
    const user = userEvent.setup();
    onCommand("search_notes", () =>
      results([hit(), hit({ id: "n_d4e5f6", path: "Inbox/call.md", title: "Call with Dana" })]),
    );
    onCommand("read_note", () => {
      throw new Error("stop the editor here: the navigation is what this asserts");
    });
    const field = await openSearch(user);

    await user.type(field, "tournament");
    await screen.findAllByRole("option");

    // Down twice lands on the second row; the field keeps focus throughout.
    await user.keyboard("{ArrowDown}{ArrowDown}");
    await waitFor(() => {
      expect(field).toHaveAttribute("aria-activedescendant");
    });
    const secondOption = screen.getAllByRole("option")[1];
    expect(field.getAttribute("aria-activedescendant")).toBe(secondOption.id);
    expect(secondOption).toHaveAttribute("aria-selected", "true");
    expect(field).toHaveFocus();

    // Up walks back, so the row lit is the first one again.
    await user.keyboard("{ArrowUp}");
    expect(field.getAttribute("aria-activedescendant")).toBe(
      screen.getAllByRole("option")[0].id,
    );

    await user.keyboard("{Enter}");
    await waitFor(() => {
      expect(invokedCommands()).toContain("read_note");
    });
  });

  it("opens the top hit when Enter is pressed without walking", async () => {
    const user = userEvent.setup();
    onCommand("search_notes", () => results([hit()]));
    const calls: unknown[] = [];
    onCommand("read_note", (args) => {
      calls.push(args);
      throw new Error("stop the editor here");
    });
    const field = await openSearch(user);

    await user.type(field, "tournament");
    await screen.findAllByRole("option");
    await user.keyboard("{Enter}");

    await waitFor(() => {
      expect(calls[calls.length - 1]).toEqual({ id: "n_a1b2c3", project: "Briarwood Golf" });
    });
  });

  it("surfaces a failed search, which is also how an unavailable index reads", async () => {
    const user = userEvent.setup();
    onCommand("search_notes", () => {
      throw new Error("the note index is unavailable this session");
    });
    const field = await openSearch(user);

    await user.type(field, "tournament");

    expect(
      await screen.findByText(/the note index is unavailable this session/),
    ).toBeInTheDocument();
  });
});
