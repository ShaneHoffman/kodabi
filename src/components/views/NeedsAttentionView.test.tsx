import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { NeedsAttentionView } from "./NeedsAttentionView";
import { NavigationProvider } from "../NavigationProvider";
import { DISTILL_STATE_EVENT } from "../../events";
import type { FailedSession } from "../../useSessions";
import { notifyVaultChanged } from "../../useVaultQuery";
import { emitFromBackend, invoke, onCommand, resetTauriMocks } from "../../test/tauri";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

function makeSession(slug: string): FailedSession {
  return {
    path: `sessions/2026-07-01T10-00-00Z-${slug}.jsonl`,
    file_name: `2026-07-01T10-00-00Z-${slug}.jsonl`,
    slug,
    captured_at: "2026-07-01T10:00:00Z",
  };
}

function serveSessions(sessions: FailedSession[]): void {
  onCommand("list_failed_sessions", () => sessions);
}

function renderView() {
  return render(
    <NavigationProvider>
      <NeedsAttentionView />
    </NavigationProvider>,
  );
}

describe("NeedsAttentionView", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("lists every failed capture, with no cap and no expander", async () => {
    // As a section inside the Inbox this list was capped at three rows so it
    // could not bury the notes it sat above. Here the list is the subject of
    // the view, so holding any of it back would hide the only thing on screen.
    serveSessions(
      ["team-sync", "vendor-call", "board-prep", "retro"].map(makeSession),
    );

    renderView();

    await waitFor(() => {
      expect(screen.getAllByTestId("retry-distill")).toHaveLength(4);
    });
    expect(screen.getByText("retro")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Show \d+ more/ })).not.toBeInTheDocument();
  });

  it("counts the work in the header", async () => {
    serveSessions([makeSession("team-sync"), makeSession("retro")]);

    renderView();

    expect(await screen.findByText("2 captures to retry")).toBeInTheDocument();
  });

  it("retries one capture and keeps the other rows out of it", async () => {
    // Each run spends a real headless Claude call and the backend serializes
    // them, so only one may be in flight. The running row stays focusable
    // (aria-disabled via `loading`) while its neighbours go natively disabled.
    const user = userEvent.setup();
    serveSessions([makeSession("team-sync"), makeSession("retro")]);
    onCommand("distill_session", () => null);
    renderView();
    await screen.findByText("team sync");

    const [first, second] = screen.getAllByTestId("retry-distill");
    await user.click(first);

    expect(invoke).toHaveBeenCalledWith("distill_session", {
      sessionPath: "sessions/2026-07-01T10-00-00Z-team-sync.jsonl",
    });
    expect(first).toHaveAccessibleName("Retrying…");
    expect(first).not.toBeDisabled();
    expect(second).toBeDisabled();
  });

  it("re-arms only the row whose own run finished", async () => {
    const user = userEvent.setup();
    const [teamSync, retro] = [makeSession("team-sync"), makeSession("retro")];
    serveSessions([teamSync, retro]);
    onCommand("distill_session", () => null);
    renderView();
    await screen.findByText("team sync");

    const [first] = screen.getAllByTestId("retry-distill");
    await user.click(first);

    // A different session finishing must not re-arm this one: it would let the
    // user queue a second run, and a second note, for a distill still going.
    act(() => {
      emitFromBackend(DISTILL_STATE_EVENT, {
        status: "saved",
        session_path: retro.path,
        path: "Projects/briarwood-golf/retro.md",
      });
    });
    expect(first).toHaveAccessibleName("Retrying…");

    act(() => {
      emitFromBackend(DISTILL_STATE_EVENT, {
        status: "saved",
        session_path: teamSync.path,
        path: "Projects/briarwood-golf/team-sync.md",
      });
    });
    await waitFor(() => {
      expect(first).toHaveAccessibleName("Retry");
    });
  });

  it("surfaces a failed distill under the row it belongs to, then prunes it", async () => {
    const teamSync = makeSession("team-sync");
    serveSessions([teamSync, makeSession("retro")]);
    renderView();
    await screen.findByText("team sync");

    act(() => {
      emitFromBackend(DISTILL_STATE_EVENT, {
        status: "error",
        session_path: teamSync.path,
        message: "claude exited 1",
      });
    });
    expect(await screen.findByText(/claude exited 1/)).toBeInTheDocument();

    // Once the session leaves the list the message must go with it, or it comes
    // back under whatever row later recycles that position.
    serveSessions([makeSession("retro")]);
    act(() => {
      notifyVaultChanged();
    });

    await waitFor(() => {
      expect(screen.queryByText(/claude exited 1/)).not.toBeInTheDocument();
    });
  });

  it("clears the pending row and says so when the retry call itself fails", async () => {
    const user = userEvent.setup();
    serveSessions([makeSession("team-sync")]);
    onCommand("distill_session", () => {
      throw "no capture session at that path";
    });
    renderView();
    await screen.findByText("team sync");

    await user.click(screen.getByTestId("retry-distill"));

    expect(await screen.findByText(/no capture session at that path/)).toBeInTheDocument();
    // Still actionable: the button is back, not stuck on "Retrying…".
    expect(screen.getByTestId("retry-distill")).toHaveAccessibleName("Retry");
  });

  it("says all clear rather than emptying out when the last capture is handled", async () => {
    // The sidebar row that leads here disappears at zero. Without this the user
    // standing on the view would watch it go blank with nothing to say it went
    // well (docs/DESIGN_SYSTEM.md §3).
    serveSessions([]);

    renderView();

    expect(await screen.findByText(/All clear/)).toBeInTheDocument();
  });

  it("surfaces a failed listing instead of claiming all clear", async () => {
    onCommand("list_failed_sessions", () => {
      throw "the sessions folder is unreadable";
    });

    renderView();

    expect(
      await screen.findByText(/the sessions folder is unreadable/),
    ).toBeInTheDocument();
    expect(screen.queryByText(/All clear/)).not.toBeInTheDocument();
  });
});
