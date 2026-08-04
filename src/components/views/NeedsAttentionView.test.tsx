import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { NeedsAttentionView } from "./NeedsAttentionView";
import { NavigationProvider } from "../providers/NavigationProvider";
import { DISTILL_STATE_EVENT } from "../../events";
import type { FailedSession } from "../../useSessions";
import { notifyVaultChanged } from "../../useVaultQuery";
import { emitFromBackend, invoke, onCommand, resetTauriMocks } from "../../test/tauri";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

/** The device ID every fixture is captured on. Real shape (8 lowercase base36
 * characters, `DeviceId::parse`), because the meta line parses it back out of
 * the filename. */
const DEVICE_ID = "k4m2xp7q";

function makeSession(slug: string, dismissed = false): FailedSession {
  // `{timestamp}-{deviceID}-{slug}.jsonl`, the scheme `kodabi_core::naming`
  // writes and `docs/FILENAME_SCHEME.md` specifies.
  const fileName = `20260701T100000000Z-${DEVICE_ID}-${slug}.jsonl`;
  return {
    path: `sessions/${fileName}`,
    file_name: fileName,
    slug,
    captured_at: "2026-07-01T10:00:00Z",
    dismissed,
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
      // Not `.map(makeSession)`: map's index would land in the `dismissed`
      // parameter and mark every row after the first as dismissed.
      ["team-sync", "vendor-call", "board-prep", "retro"].map((slug) => makeSession(slug)),
    );

    renderView();

    await waitFor(() => {
      expect(screen.getAllByTestId("retry-distill")).toHaveLength(4);
    });
    expect(screen.getByText("retro")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Show \d+ more/ })).not.toBeInTheDocument();
    // And no dismissed shelf when nothing has been dismissed.
    expect(screen.queryByTestId("show-dismissed")).not.toBeInTheDocument();
  });

  it("says what went wrong on the card, in mono meta and plain language", async () => {
    // The badge is marigold, which day mode measures under the reading floor —
    // so it may never be the only thing that says a capture failed. The meta
    // line names the session (the id a person can quote back), and the
    // sentence under it says what happened and that the recording survived.
    serveSessions([makeSession("team-sync")]);

    renderView();

    expect(await screen.findByText("no note created")).toBeInTheDocument();
    expect(screen.getByText(new RegExp(`session ${DEVICE_ID}`))).toBeInTheDocument();
    expect(
      screen.getByText(/Kodabi made no note from this capture/),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/Dismissed captures stay reachable from the command palette/),
    ).toBeInTheDocument();
  });

  it("drops the failure line from a capture already waved off", async () => {
    // A dismissed card is a settled thing: it keeps its title and its meta so
    // it stays identifiable, and stops claiming to need anything.
    const user = userEvent.setup();
    serveSessions([makeSession("team-sync", true)]);

    renderView();
    await user.click(await screen.findByTestId("show-dismissed"));

    const shelf = screen.getByTestId("dismissed-sessions");
    expect(within(shelf).getByText("team sync")).toBeInTheDocument();
    expect(within(shelf).getByText(new RegExp(`session ${DEVICE_ID}`))).toBeInTheDocument();
    expect(screen.queryByText("no note created")).not.toBeInTheDocument();
    expect(
      screen.queryByText(/Kodabi made no note from this capture/),
    ).not.toBeInTheDocument();
  });

  it("counts the work in the header", async () => {
    serveSessions([makeSession("team-sync"), makeSession("retro")]);

    renderView();

    // The count plus when it last went wrong: "2 captures to retry · last
    // failed Jul 19". The date is what turns a number into a situation.
    expect(await screen.findByText(/2 captures to retry/)).toBeInTheDocument();
    expect(await screen.findByText(/last failed/)).toBeInTheDocument();
  });

  it("retries one capture and keeps the other rows out of it", async () => {
    // Each run spends a real headless Claude call and the backend serializes
    // them, so only one may be in flight. The running row stays focusable
    // (aria-disabled via `loading`) while its neighbours go natively disabled.
    const user = userEvent.setup();
    const teamSync = makeSession("team-sync");
    serveSessions([teamSync, makeSession("retro")]);
    onCommand("distill_session", () => null);
    renderView();
    await screen.findByText("team sync");

    const [first, second] = screen.getAllByTestId("retry-distill");
    await user.click(first);

    expect(invoke).toHaveBeenCalledWith("distill_session", {
      sessionPath: teamSync.path,
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
    // The kodama beside it is aria-hidden, so the hint is what carries the
    // state to a screen reader.
    expect(
      screen.getByText(/Failed captures land here so they are never invisible/),
    ).toBeInTheDocument();
    // Nothing has been dismissed either, so there is nothing for the footnote
    // to reassure anyone about.
    expect(
      screen.queryByText(/Dismissed captures stay reachable/),
    ).not.toBeInTheDocument();
  });

  it("holds every marker action while a retry is in flight", async () => {
    // Once a retry is queued the backend's in-flight filter drops the row from
    // the listing, so the refetch that *any* dismiss/restore/delete triggers —
    // not just the running row's own — would remove the running row out from
    // under `pendingPath`: its spinner vanishes and every other Retry stays
    // disabled, with nothing left on screen saying why, until a terminal event
    // that may be minutes away. So the whole verb family waits the retry out.
    const user = userEvent.setup();
    serveSessions([
      makeSession("team-sync"),
      makeSession("retro"),
      makeSession("board-prep", true),
    ]);
    onCommand("distill_session", () => null);
    renderView();
    await user.click(await screen.findByTestId("show-dismissed"));

    const [firstRetry] = screen.getAllByTestId("retry-distill");
    await user.click(firstRetry);

    for (const dismiss of screen.getAllByRole("button", { name: "Dismiss" })) {
      expect(dismiss).toBeDisabled();
    }
    expect(screen.getByTestId("restore-session")).toBeDisabled();
    expect(screen.getByTestId("delete-session")).toBeDisabled();
  });

  it("dismisses a card through the backend and moves it to the dismissed shelf", async () => {
    // Dismiss is a backend write now: the listing itself reports the session
    // dismissed, so the sidebar (which reads the same listing) and this view
    // change together and can never disagree.
    const user = userEvent.setup();
    const teamSync = makeSession("team-sync");
    serveSessions([teamSync]);
    onCommand("dismiss_session", () => {
      // The marker is on disk: every listing from here on reports it.
      serveSessions([makeSession("team-sync", true)]);
      return null;
    });
    renderView();
    await screen.findByText("team sync");

    await user.click(screen.getByRole("button", { name: "Dismiss" }));

    expect(invoke).toHaveBeenCalledWith("dismiss_session", {
      sessionPath: teamSync.path,
    });
    // The card leaves the active list...
    expect(await screen.findByText(/All clear/)).toBeInTheDocument();
    // ...and reappears behind the dismissed shelf, one interaction away.
    const shelf = await screen.findByTestId("show-dismissed");
    expect(shelf).toHaveAttribute("aria-expanded", "false");
    await user.click(shelf);
    expect(shelf).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("team sync")).toBeInTheDocument();
    expect(screen.getByTestId("restore-session")).toBeInTheDocument();
  });

  it("restores a dismissed capture and counts it again", async () => {
    const user = userEvent.setup();
    const teamSync = makeSession("team-sync", true);
    serveSessions([teamSync]);
    onCommand("restore_session", () => {
      serveSessions([makeSession("team-sync")]);
      return null;
    });
    renderView();

    await user.click(await screen.findByTestId("show-dismissed"));
    await user.click(screen.getByTestId("restore-session"));

    expect(invoke).toHaveBeenCalledWith("restore_session", {
      sessionPath: teamSync.path,
    });
    // Back in the active list, and the shelf leaves with its last card.
    await waitFor(() => {
      expect(screen.getByTestId("retry-distill")).toBeInTheDocument();
    });
    expect(screen.queryByTestId("show-dismissed")).not.toBeInTheDocument();
  });

  it("keeps the card and says so when the dismiss call fails", async () => {
    const user = userEvent.setup();
    serveSessions([makeSession("team-sync")]);
    onCommand("dismiss_session", () => {
      throw "marker write failed";
    });
    renderView();
    await screen.findByText("team sync");

    await user.click(screen.getByRole("button", { name: "Dismiss" }));

    expect(
      await screen.findByText(/Dismiss failed: marker write failed/),
    ).toBeInTheDocument();
    // Still listed, still actionable.
    expect(screen.getByTestId("retry-distill")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Dismiss" })).not.toBeDisabled();
  });

  it("keeps dismissed captures reachable when everything is dismissed", async () => {
    // The sidebar row hides at zero undismissed; this view (still reachable
    // via the palette) owns the way back: all clear up top, the shelf below.
    const user = userEvent.setup();
    serveSessions([makeSession("team-sync", true), makeSession("retro", true)]);

    renderView();

    expect(await screen.findByText(/All clear/)).toBeInTheDocument();
    const shelf = screen.getByTestId("show-dismissed");
    expect(shelf).toHaveTextContent("Dismissed");
    expect(shelf).toHaveTextContent("2");
    await user.click(shelf);
    expect(screen.getAllByTestId("restore-session")).toHaveLength(2);
  });

  it("deletes a dismissed capture only after confirming in the modal", async () => {
    const user = userEvent.setup();
    const teamSync = makeSession("team-sync", true);
    serveSessions([teamSync]);
    onCommand("delete_session", () => {
      serveSessions([]);
      return null;
    });
    renderView();

    await user.click(await screen.findByTestId("show-dismissed"));
    await user.click(screen.getByTestId("delete-session"));

    // Delete opens a confirmation modal; nothing has been invoked yet.
    const dialog = screen.getByRole("dialog", { name: "Delete this capture?" });
    expect(invoke).not.toHaveBeenCalledWith("delete_session", expect.anything());

    await user.click(within(dialog).getByRole("button", { name: "Delete capture" }));

    expect(invoke).toHaveBeenCalledWith("delete_session", {
      sessionPath: teamSync.path,
    });
    // Gone for good: no shelf left, just the all-clear.
    await waitFor(() => {
      expect(screen.queryByTestId("show-dismissed")).not.toBeInTheDocument();
    });
    expect(screen.getByText(/All clear/)).toBeInTheDocument();
  });

  it("cancelling the delete modal leaves the capture dismissed and calls nothing", async () => {
    const user = userEvent.setup();
    serveSessions([makeSession("team-sync", true)]);
    renderView();

    await user.click(await screen.findByTestId("show-dismissed"));
    await user.click(screen.getByTestId("delete-session"));

    const dialog = screen.getByRole("dialog", { name: "Delete this capture?" });
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("delete_session", expect.anything());
    // The row's normal actions are back, untouched.
    expect(screen.getByTestId("restore-session")).toBeInTheDocument();
    expect(screen.getByTestId("delete-session")).toBeInTheDocument();
  });

  it("shows a failed capture delete inside the modal and stays open", async () => {
    const user = userEvent.setup();
    serveSessions([makeSession("team-sync", true)]);
    onCommand("delete_session", () => {
      throw "session is locked";
    });
    renderView();

    await user.click(await screen.findByTestId("show-dismissed"));
    await user.click(screen.getByTestId("delete-session"));

    const dialog = screen.getByRole("dialog", { name: "Delete this capture?" });
    await user.click(within(dialog).getByRole("button", { name: "Delete capture" }));

    expect(
      await within(dialog).findByText("Couldn't delete the capture: session is locked"),
    ).toBeInTheDocument();
    // The modal stays open so the delete can be retried or cancelled.
    expect(screen.getByRole("dialog", { name: "Delete this capture?" })).toBeInTheDocument();
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
