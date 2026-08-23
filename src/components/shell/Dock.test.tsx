import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CapturePipelineProvider } from "../providers/CapturePipelineProvider";
import { Dock } from "./Dock";
import { MainContent } from "./MainContent";
import { NavigationProvider } from "../providers/NavigationProvider";
import { DISTILL_STATE_EVENT } from "../../events";
import type { FailedSession } from "../../useSessions";
import { emitFromBackend, onCommand, resetTauriMocks } from "../../test/tauri";
import { notifyVaultChanged } from "../../useVaultQuery";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

function makeSession(slug: string, dismissed = false): FailedSession {
  return {
    path: `sessions/2026-07-01T10-00-00Z-${slug}.jsonl`,
    file_name: `2026-07-01T10-00-00Z-${slug}.jsonl`,
    slug,
    captured_at: "2026-07-01T10:00:00Z",
    dismissed,
  };
}

/** The reads the dock and the views behind it make. */
function serveVault(sessions: FailedSession[] = []): void {
  onCommand("list_projects", () => ({ inbox_note_count: 0, projects: [] }));
  onCommand("list_notes", () => []);
  onCommand("list_failed_sessions", () => sessions);
  // The capture pipeline around the dock reads this on mount; left unrouted it
  // would reject and put an error beside the row under test.
  onCommand("capture_phase", () => ({
    phase: "idle",
    sources: { loopback: "off", microphone: "off" },
  }));
  // Same reasoning as `capture_phase`: left unrouted this rejects, and the
  // commitments row would read as a quiet zero forever.
  onCommand("count_my_commitments", () => commitmentCount);
}

/** What the next `count_my_commitments` read answers. */
let commitmentCount = 0;

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

describe("Dock needs-attention row", () => {
  beforeEach(() => {
    resetTauriMocks();
    commitmentCount = 0;
  });

  it("stays hidden while nothing has failed", async () => {
    serveVault([]);

    renderShell();

    // Wait for the listing to land, so this asserts "read, and nothing to say"
    // rather than "has not read yet".
    await waitFor(() => {
      expect(screen.getByText(/No projects yet\./)).toBeInTheDocument();
    });
    expect(screen.queryByTestId("needs-attention-nav")).not.toBeInTheDocument();
  });

  it("appears with a count once captures need attention", async () => {
    serveVault([makeSession("team-sync"), makeSession("retro")]);

    renderShell();

    const row = await screen.findByTestId("needs-attention-nav");
    expect(row).toHaveTextContent("Needs attention");
    expect(row).toHaveTextContent("2");
  });

  it("counts only captures that still need attention", async () => {
    // A dismissed capture is cleared: it stays in the listing (the view's
    // dismissed shelf reads it) but must not keep the row's count inflated.
    serveVault([
      makeSession("team-sync"),
      makeSession("retro", true),
      makeSession("board-prep", true),
    ]);

    renderShell();

    const row = await screen.findByTestId("needs-attention-nav");
    expect(row).toHaveTextContent("Needs attention");
    expect(row).toHaveTextContent("1");
    expect(row).not.toHaveTextContent("3");
  });

  it("disappears when every capture is dismissed", async () => {
    // Dismissing the last capture is what collapses the rail: that quiet is
    // the whole point of dismissing. The way back is the palette jump, which
    // keys on the full listing.
    serveVault([makeSession("team-sync", true)]);

    renderShell();

    await waitFor(() => {
      expect(screen.getByText(/No projects yet\./)).toBeInTheDocument();
    });
    expect(screen.queryByTestId("needs-attention-nav")).not.toBeInTheDocument();
  });

  it("still appears when the listing itself failed", async () => {
    // A read that failed is not an empty list, and must not read as all clear.
    serveVault();
    onCommand("list_failed_sessions", () => {
      throw "the sessions folder is unreadable";
    });

    renderShell();

    expect(await screen.findByTestId("needs-attention-nav")).toBeInTheDocument();
  });

  it("surfaces a failure that happens while the user is elsewhere", async () => {
    // The refetch used to be owned by the Inbox, so a distill failing while any
    // other view was open reached nothing. This row is mounted for the whole
    // session, which is what makes the signal unconditional.
    serveVault([]);
    renderShell();
    await waitFor(() => {
      expect(screen.getByText(/No projects yet\./)).toBeInTheDocument();
    });

    onCommand("list_failed_sessions", () => [makeSession("team-sync")]);
    act(() => {
      emitFromBackend(DISTILL_STATE_EVENT, {
        status: "error",
        session_path: "sessions/2026-07-01T10-00-00Z-team-sync.jsonl",
        message: "claude exited 1",
      });
    });

    expect(await screen.findByTestId("needs-attention-nav")).toBeInTheDocument();
  });

  it("navigates to the needs-attention view and marks itself current", async () => {
    const user = userEvent.setup();
    serveVault([makeSession("team-sync")]);
    renderShell();
    const row = await screen.findByTestId("needs-attention-nav");
    expect(row).not.toHaveAttribute("aria-current");

    await user.click(row);

    expect(
      screen.getByRole("heading", { name: "Needs attention", level: 2 }),
    ).toBeInTheDocument();
    expect(row).toHaveAttribute("aria-current", "page");
  });
});

describe("Dock project listing failure", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("says what failed, what happens next, and keeps the vault reachable", async () => {
    serveVault();
    onCommand("list_projects", () => {
      throw "Couldn't read your projects. They are still on disk; restart Kodabi if this keeps happening.";
    });

    renderShell();

    // One sentence, both halves. The dock never unmounts, so "reopen this
    // view" is not the way out here and `list_projects` does not say it
    // (docs/DESIGN_SYSTEM.md §3).
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(
      "Couldn't read your projects. They are still on disk; restart Kodabi if this keeps happening.",
    );
    // Data stays reachable: a failed listing must not read as an empty vault,
    // and the system destinations still work.
    expect(screen.queryByText(/No projects yet\./)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Search" })).toBeInTheDocument();
  });
});

describe("Dock layout", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  // jsdom has no layout, so this pins the STRUCTURE that produces the
  // behaviour: the destinations list is the scroll container, and the tools at
  // the foot sit outside it. Put the scroll on the <aside> instead and the
  // foot stops being a foot — `mt-auto` distributes free space, an overflowing
  // column has none, and a vault past a dozen folders scrolls Chat and
  // Terminal below the fold.
  it("scrolls the destinations, not the pane, so the tools stay pinned", async () => {
    serveVault();
    renderShell();

    const destinations = await screen.findByRole("navigation", {
      name: "Knowledge base",
    });
    const tools = screen.getByRole("navigation", { name: "Tools" });

    expect(destinations).toHaveClass("min-h-0", "flex-1", "overflow-y-auto");
    expect(destinations).not.toContainElement(tools);
    expect(tools.parentElement).not.toHaveClass("overflow-y-auto");
  });
});

describe("Dock commitments row", () => {
  beforeEach(() => {
    resetTauriMocks();
    commitmentCount = 0;
  });

  it("carries the count of what is on you", async () => {
    commitmentCount = 4;
    serveVault();

    renderShell();

    expect(await screen.findByTestId("sidebar-commitments-count")).toHaveTextContent(
      "4",
    );
  });

  it("goes quiet at zero rather than celebrating, and keeps its place", async () => {
    serveVault();

    renderShell();

    // Wait for the read to land, so this asserts "counted, and nothing to say"
    // rather than "has not counted yet".
    await waitFor(() => {
      expect(screen.getByTestId("commitments-nav")).toBeInTheDocument();
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(
      screen.queryByTestId("sidebar-commitments-count"),
    ).not.toBeInTheDocument();
    // The row is a destination, not a notice: it stays whatever the number is.
    expect(screen.getByTestId("commitments-nav")).toBeInTheDocument();
  });

  it("comes down when a commitment settles", async () => {
    commitmentCount = 2;
    serveVault();

    renderShell();
    expect(await screen.findByTestId("sidebar-commitments-count")).toHaveTextContent(
      "2",
    );

    // Both `ledger:changed` and `vault:changed` are relayed onto this one bus
    // at the shell root, which is outside this harness, so it is driven
    // directly here.
    commitmentCount = 1;
    act(() => {
      notifyVaultChanged();
    });

    await waitFor(() => {
      expect(screen.getByTestId("sidebar-commitments-count")).toHaveTextContent(
        "1",
      );
    });
  });
});
