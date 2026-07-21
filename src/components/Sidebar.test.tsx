import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { Sidebar } from "./Sidebar";
import { MainContent } from "./MainContent";
import { NavigationProvider } from "./NavigationProvider";
import { DISTILL_STATE_EVENT } from "../events";
import type { FailedSession } from "../useSessions";
import { emitFromBackend, onCommand, resetTauriMocks } from "../test/tauri";

vi.mock("@tauri-apps/api/core", () => import("../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../test/tauri"));

function makeSession(slug: string): FailedSession {
  return {
    path: `sessions/2026-07-01T10-00-00Z-${slug}.jsonl`,
    file_name: `2026-07-01T10-00-00Z-${slug}.jsonl`,
    slug,
    captured_at: "2026-07-01T10:00:00Z",
  };
}

/** The reads the sidebar and the views behind it make. */
function serveVault(sessions: FailedSession[] = []): void {
  onCommand("list_projects", () => ({ inbox_note_count: 0, projects: [] }));
  onCommand("list_notes", () => []);
  onCommand("list_failed_sessions", () => sessions);
  // The listening indicator in the footer reads this on mount; left unrouted it
  // would reject and put an error beside the row under test.
  onCommand("capture_phase", () => ({
    phase: "idle",
    sources: { loopback: "off", microphone: "off" },
  }));
}

function renderShell() {
  return render(
    <NavigationProvider>
      <Sidebar onOpenPalette={() => {}} />
      <MainContent />
    </NavigationProvider>,
  );
}

describe("Sidebar needs-attention row", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("stays hidden while nothing has failed", async () => {
    serveVault([]);

    renderShell();

    // Wait for the listing to land, so this asserts "read, and nothing to say"
    // rather than "has not read yet".
    await waitFor(() => {
      expect(screen.getByText("No projects yet.")).toBeInTheDocument();
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
      expect(screen.getByText("No projects yet.")).toBeInTheDocument();
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
