import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CapturePipelineProvider } from "./CapturePipelineProvider";
import { Sidebar } from "./Sidebar";
import { MainContent } from "./MainContent";
import { NavigationProvider } from "./NavigationProvider";
import { DISTILL_STATE_EVENT } from "../events";
import type { FailedSession } from "../useSessions";
import { emitFromBackend, onCommand, resetTauriMocks } from "../test/tauri";

vi.mock("@tauri-apps/api/core", () => import("../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../test/tauri"));

function makeSession(slug: string, dismissed = false): FailedSession {
  return {
    path: `sessions/2026-07-01T10-00-00Z-${slug}.jsonl`,
    file_name: `2026-07-01T10-00-00Z-${slug}.jsonl`,
    slug,
    captured_at: "2026-07-01T10:00:00Z",
    dismissed,
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
      <CapturePipelineProvider>
        <Sidebar onOpenPalette={() => {}} />
        <MainContent />
      </CapturePipelineProvider>
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
