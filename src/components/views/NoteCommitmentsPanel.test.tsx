import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { invoke, onCommand, resetTauriMocks } from "../../test/tauri";
import type {
  NoteCommitmentItem,
  NoteCommitmentsPayload,
} from "../../useNoteCommitments";
import { NoteCommitmentsPanel } from "./NoteCommitmentsPanel";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

function line(
  overrides: Partial<NoteCommitmentItem> & Pick<NoteCommitmentItem, "item_id">,
): NoteCommitmentItem {
  return {
    description: "send the revised deck",
    owner: "Priya",
    due_date: null,
    done: false,
    direction: "theirs",
    tracking: "tracked",
    untracked_via: null,
    entry_id: "le_aaaaaaaaaaaa",
    entry_state: "open",
    ...overrides,
  };
}

function serve(payload: Partial<NoteCommitmentsPayload>): void {
  onCommand("list_note_commitments", () => ({
    context_only: payload.context_only ?? false,
    items: payload.items ?? [],
  }));
}

function renderPanel() {
  render(<NoteCommitmentsPanel noteId="n_a1b2c3" />);
}

describe("NoteCommitmentsPanel", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("stays silent when the ledger cannot be read", async () => {
    onCommand("list_note_commitments", () => {
      throw "The commitment ledger isn't available this session.";
    });
    renderPanel();

    // The one departure from the four-states rule, argued in the component: a
    // read fails structurally, so an error here would repeat one problem on
    // every note screen. The Commitments view states it once, in full.
    await waitFor(() =>
      expect(
        invoke.mock.calls.some(
          ([command]) => command === "list_note_commitments",
        ),
      ).toBe(true),
    );
    expect(screen.queryByText("Commitments")).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("says nothing for a note that extracted no commitments", async () => {
    serve({});
    renderPanel();

    // Most notes are this, and a panel announcing "no commitments" on a
    // shopping list is noise rather than an empty state.
    await waitFor(() =>
      expect(
        invoke.mock.calls.some(
          ([command]) => command === "list_note_commitments",
        ),
      ).toBe(true),
    );
    expect(screen.queryByText("Commitments")).not.toBeInTheDocument();
  });

  it("distinguishes tracked, untracked and never-enrolled lines", async () => {
    serve({
      context_only: true,
      items: [
        line({ item_id: "a_111111", description: "book the venue", direction: "mine" }),
        line({
          item_id: "a_222222",
          description: "chase the caterer",
          tracking: "not_enrolled",
          entry_id: null,
          entry_state: null,
        }),
        line({
          item_id: "a_333333",
          description: "send the revised deck",
          tracking: "untracked",
          untracked_via: "manual",
          entry_state: "untracked",
        }),
      ],
    });
    renderPanel();

    expect(await screen.findByText("Commitments")).toBeInTheDocument();
    expect(screen.getByText("book the venue")).toBeInTheDocument();
    expect(screen.getByText(/not tracked/)).toBeInTheDocument();
    expect(screen.getByText(/· untracked/)).toBeInTheDocument();
    // Track is offered on the two that are out and not on the one that is in:
    // untracking a tracked line belongs to the Commitments view.
    expect(screen.getAllByRole("button", { name: "Track" })).toHaveLength(2);
  });

  it("flips the meeting's tracking through the switch", async () => {
    const user = userEvent.setup();
    serve({ items: [line({ item_id: "a_111111" })] });
    onCommand("set_meeting_tracking", () => ({
      context_only: true,
      untracked: 1,
      retracked: 0,
    }));
    renderPanel();

    const toggle = await screen.findByRole("switch", { name: "Context only" });
    expect(toggle).toHaveAttribute("aria-checked", "false");
    await user.click(toggle);

    await waitFor(() =>
      expect(
        invoke.mock.calls.some(
          ([command, args]) =>
            command === "set_meeting_tracking" &&
            (args as { input: { note_id: string; context_only: boolean } }).input
              .context_only === true,
        ),
      ).toBe(true),
    );
  });

  it("promotes one line by hand", async () => {
    const user = userEvent.setup();
    serve({
      context_only: true,
      items: [
        line({
          item_id: "a_222222",
          tracking: "not_enrolled",
          entry_id: null,
          entry_state: null,
        }),
      ],
    });
    onCommand("track_commitment_item", () => ({
      entry_id: "le_bbbbbbbbbbbb",
      state: "open",
      snoozed_until: null,
      closed_via: null,
      review_reason: null,
      updated_at: "2026-08-18T09:00:00Z",
      untracked_via: null,
    }));
    renderPanel();

    await user.click(await screen.findByRole("button", { name: "Track" }));

    await waitFor(() =>
      expect(
        invoke.mock.calls.some(
          ([command, args]) =>
            command === "track_commitment_item" &&
            (args as { input: { item_id: string } }).input.item_id === "a_222222",
        ),
      ).toBe(true),
    );
  });

  it("names what failed and leaves the note reachable", async () => {
    const user = userEvent.setup();
    serve({ items: [line({ item_id: "a_111111" })] });
    onCommand("set_meeting_tracking", () => {
      throw "The commitment ledger isn't available this session, so this change wasn't saved.";
    });
    renderPanel();

    await user.click(await screen.findByRole("switch", { name: "Context only" }));

    expect(
      await screen.findByText(/isn't available this session/),
    ).toBeInTheDocument();
    // The lines are still on screen: an error never takes the data with it.
    expect(screen.getByText("send the revised deck")).toBeInTheDocument();
  });
});
