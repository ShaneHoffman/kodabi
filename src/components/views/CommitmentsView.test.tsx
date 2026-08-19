import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { LEDGER_CHANGED_EVENT, VAULT_CHANGED_EVENT } from "../../events";
import {
  emitFromBackend,
  invoke,
  onCommand,
  resetTauriMocks,
} from "../../test/tauri";
import type {
  Commitment,
  CommitmentItem,
  CommitmentsPayload,
} from "../../useCommitments";
import { useNavigation, type View } from "../../useNavigation";
import { useLedgerChangedBridge } from "../../useLedgerChangedBridge";
import { useVaultChangedBridge } from "../../useVaultChangedBridge";
import { CapturePipelineProvider } from "../providers/CapturePipelineProvider";
import { NavigationProvider } from "../providers/NavigationProvider";
import { MainContent } from "../shell/MainContent";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

function commitment(
  overrides: Partial<Commitment> & Pick<Commitment, "entry_id">,
): Commitment {
  return {
    state: "open",
    direction: "mine",
    owner: "You",
    description: "book the venue",
    project: "Briarwood Golf",
    created_at: "2026-08-01T00:00:00Z",
    updated_at: "2026-08-01T00:00:00Z",
    last_mention: "2026-08-01T00:00:00Z",
    last_evidence_check: null,
    tier: "fresh",
    snoozed_until: null,
    snooze_lapsed: false,
    closed_via: null,
    review_reason: null,
    item: null,
    source: {
      note_id: "n_a1b2c3",
      title: "Kickoff",
      project: "Briarwood Golf",
      path: "Briarwood Golf/kickoff.md",
    },
    evidence: [],
    ...overrides,
  };
}

function item(overrides: Partial<CommitmentItem> = {}): CommitmentItem {
  return {
    note_id: "n_a1b2c3",
    item_id: "a_111111",
    description: "book the venue",
    owner: "You",
    due_date: null,
    done: false,
    status: "open",
    ...overrides,
  };
}

/** The shell's own reads, plus one ledger payload. */
function serve(payload: Partial<CommitmentsPayload>): void {
  onCommand("list_projects", () => ({
    inbox_note_count: 0,
    projects: [],
  }));
  onCommand("list_notes", () => []);
  onCommand("list_failed_sessions", () => []);
  onCommand("capture_phase", () => ({
    phase: "idle",
    sources: { loopback: "off", microphone: "off" },
  }));
  onCommand("list_commitments", () => ({
    entries: payload.entries ?? [],
    settled: payload.settled ?? [],
  }));
}

/** The two relays AppShell mounts. The ledger one lives at the shell root
 * rather than inside useCommitments, because the note view's enrollment panel
 * is a second consumer of the same event. */
function VaultBridge() {
  useVaultChangedBridge();
  useLedgerChangedBridge();
  return null;
}

let lastNavigation: View | null = null;

/** Lands the shell on the Commitments view, and records where a click-through
 * would send it. */
function CommitmentsLink({ slug }: { slug: string | null }) {
  const { view, navigate } = useNavigation();
  lastNavigation = view.kind === "noteEditor" ? view : lastNavigation;
  return (
    <button type="button" onClick={() => navigate({ kind: "commitments", slug })}>
      Open commitments
    </button>
  );
}

function renderShell(slug: string | null = null) {
  return render(
    <NavigationProvider>
      <CapturePipelineProvider>
        <VaultBridge />
        <CommitmentsLink slug={slug} />
        <MainContent />
      </CapturePipelineProvider>
    </NavigationProvider>,
  );
}

async function openCommitments(
  user: ReturnType<typeof userEvent.setup>,
  slug: string | null = null,
) {
  renderShell(slug);
  await user.click(screen.getByRole("button", { name: "Open commitments" }));
}

describe("CommitmentsView", () => {
  beforeEach(() => {
    resetTauriMocks();
    lastNavigation = null;
  });

  it("separates mine from waiting on them, in that order", async () => {
    const user = userEvent.setup();
    serve({
      entries: [
        commitment({
          entry_id: "le_theirs",
          direction: "theirs",
          owner: "Priya",
          description: "send the revised deck",
          item: item({ owner: "Priya", description: "send the revised deck" }),
        }),
        commitment({ entry_id: "le_mine", item: item() }),
      ],
    });

    await openCommitments(user);

    const mine = await screen.findByTestId("commitments-mine");
    const theirs = screen.getByTestId("commitments-theirs");
    expect(within(mine).getByText("book the venue")).toBeInTheDocument();
    expect(within(theirs).getByText("send the revised deck")).toBeInTheDocument();
    // The owner is printed on the them half, where it is the thing you want.
    expect(within(theirs).getByText("Priya")).toBeInTheDocument();
    // Mine leads: it is the half you can act on.
    expect(mine.compareDocumentPosition(theirs)).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    );
    expect(screen.getByText(/1 on you/)).toBeInTheDocument();
  });

  it("renders nothing while the first read is in flight", async () => {
    const user = userEvent.setup();
    serve({});
    // A read that never resolves: the view must not claim the ledger is empty
    // before it has heard back.
    onCommand("list_commitments", () => new Promise(() => {}));

    await openCommitments(user);

    expect(screen.queryByText(/Nothing promised yet/)).not.toBeInTheDocument();
  });

  it("teaches what the ledger is when it is empty", async () => {
    const user = userEvent.setup();
    serve({});

    await openCommitments(user);

    expect(await screen.findByText("Nothing promised yet.")).toBeInTheDocument();
    expect(
      screen.getByText(/what you owe people, and what they owe you/),
    ).toBeInTheDocument();
  });

  it("says the shelves hold everything when nothing is open", async () => {
    const user = userEvent.setup();
    serve({
      settled: [
        commitment({
          entry_id: "le_done",
          state: "closed",
          closed_via: "manual",
        }),
      ],
    });

    await openCommitments(user);

    expect(
      await screen.findByText(/Nothing open right now/),
    ).toBeInTheDocument();
    expect(screen.getByTestId("show-settled-commitments")).toBeInTheDocument();
  });

  it("names what failed when the read fails", async () => {
    const user = userEvent.setup();
    serve({});
    onCommand("list_commitments", () => {
      throw "The commitment ledger isn't available this session.";
    });

    await openCommitments(user);

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/Couldn't load the commitment ledger/);
    expect(alert).toHaveTextContent(/notes on disk are untouched/);
  });

  it("ticks the box through the note and lands the row on the settled shelf", async () => {
    const user = userEvent.setup();
    serve({ entries: [commitment({ entry_id: "le_mine", item: item() })] });
    onCommand("set_commitment_done", () => ({
      entry: {
        entry_id: "le_mine",
        state: "closed",
        snoozed_until: null,
        closed_via: "manual",
        review_reason: null,
        updated_at: "2026-08-17T12:00:00Z",
      },
      note_updated: true,
    }));

    await openCommitments(user);
    await user.click(
      await screen.findByRole("checkbox", { name: 'Mark "book the venue" done' }),
    );

    const call = invoke.mock.calls.find(
      ([command]) => command === "set_commitment_done",
    );
    expect(call?.[1]).toEqual({
      input: {
        entry_id: "le_mine",
        note_id: "n_a1b2c3",
        item_id: "a_111111",
        done: true,
      },
    });

    // The backend's echo is what moves the row, not an optimistic flip.
    serve({
      settled: [
        commitment({
          entry_id: "le_mine",
          state: "closed",
          closed_via: "manual",
          item: item({ done: true }),
        }),
      ],
    });
    emitFromBackend(VAULT_CHANGED_EVENT, undefined);

    await user.click(await screen.findByTestId("show-settled-commitments"));
    const shelf = await screen.findByTestId("settled-commitments");
    expect(within(shelf).getByText(/closed by you/)).toBeInTheDocument();
    expect(
      within(shelf).getByRole("button", { name: "Reopen" }),
    ).toBeInTheDocument();
  });

  it("refetches when a ledger mutation announces itself", async () => {
    const user = userEvent.setup();
    serve({ entries: [commitment({ entry_id: "le_mine", item: item() })] });

    await openCommitments(user);
    expect(await screen.findByText("book the venue")).toBeInTheDocument();

    serve({
      entries: [
        commitment({
          entry_id: "le_other",
          description: "call the club",
          item: item({ description: "call the club" }),
        }),
      ],
    });
    emitFromBackend(LEDGER_CHANGED_EVENT, undefined);

    expect(await screen.findByText("call the club")).toBeInTheDocument();
  });

  it("snoozes from the row menu without touching the note", async () => {
    const user = userEvent.setup();
    serve({ entries: [commitment({ entry_id: "le_mine", item: item() })] });
    onCommand("snooze_commitment", () => ({
      entry_id: "le_mine",
      state: "snoozed",
      snoozed_until: "2026-12-01",
      closed_via: null,
      review_reason: null,
      updated_at: "2026-08-17T12:00:00Z",
    }));

    await openCommitments(user);
    await user.click(
      await screen.findByRole("button", { name: 'Actions for "book the venue"' }),
    );
    await user.click(
      await screen.findByRole("menuitem", { name: "Snooze until tomorrow" }),
    );

    const call = invoke.mock.calls.find(
      ([command]) => command === "snooze_commitment",
    );
    const until = (call?.[1] as { input: { until: string } }).input.until;
    expect(until).toMatch(/^\d{4}-\d{2}-\d{2}$/);
    // The note is never written by a snooze.
    expect(
      invoke.mock.calls.some(([command]) => command === "set_commitment_done"),
    ).toBe(false);
  });

  it("wakes a snoozed row from its shelf", async () => {
    const user = userEvent.setup();
    serve({
      entries: [
        commitment({
          entry_id: "le_sleeping",
          state: "snoozed",
          snoozed_until: "2026-12-01",
          item: item(),
        }),
      ],
    });
    onCommand("reopen_commitment", () => ({
      entry_id: "le_sleeping",
      state: "open",
      snoozed_until: null,
      closed_via: null,
      review_reason: null,
      updated_at: "2026-08-17T12:00:00Z",
    }));

    await openCommitments(user);
    await user.click(await screen.findByTestId("show-snoozed-commitments"));
    const shelf = await screen.findByTestId("snoozed-commitments");
    expect(within(shelf).getByText(/snoozed until/)).toBeInTheDocument();
    await user.click(within(shelf).getByRole("button", { name: "Wake" }));

    expect(
      invoke.mock.calls.some(([command]) => command === "reopen_commitment"),
    ).toBe(true);
  });

  it("waives without a confirmation, because reopening takes it back", async () => {
    const user = userEvent.setup();
    serve({ entries: [commitment({ entry_id: "le_mine", item: item() })] });
    onCommand("waive_commitment", () => ({
      entry_id: "le_mine",
      state: "waived",
      snoozed_until: null,
      closed_via: null,
      review_reason: null,
      updated_at: "2026-08-17T12:00:00Z",
    }));

    await openCommitments(user);
    await user.click(
      await screen.findByRole("button", { name: 'Actions for "book the venue"' }),
    );
    await user.click(await screen.findByRole("menuitem", { name: "Waive" }));

    await waitFor(() =>
      expect(
        invoke.mock.calls.some(([command]) => command === "waive_commitment"),
      ).toBe(true),
    );
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("untracks from the row menu, beside waive", async () => {
    const user = userEvent.setup();
    serve({ entries: [commitment({ entry_id: "le_mine", item: item() })] });
    onCommand("untrack_commitment", () => ({
      entry_id: "le_mine",
      state: "untracked",
      snoozed_until: null,
      closed_via: null,
      review_reason: null,
      updated_at: "2026-08-17T12:00:00Z",
      untracked_via: "manual",
    }));

    await openCommitments(user);
    await user.click(
      await screen.findByRole("button", { name: 'Actions for "book the venue"' }),
    );
    // The two ways out of the working set sit together: the choice between them
    // is the one the reader is making.
    expect(await screen.findByRole("menuitem", { name: "Waive" })).toBeInTheDocument();
    await user.click(await screen.findByRole("menuitem", { name: "Untrack" }));

    await waitFor(() =>
      expect(
        invoke.mock.calls.some(([command]) => command === "untrack_commitment"),
      ).toBe(true),
    );
    // No confirmation, for the same reason waive has none: one ledger row, no
    // note touched, and the shelf below takes it straight back.
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("files an untracked entry on the settled shelf, saying so, with its undo", async () => {
    const user = userEvent.setup();
    serve({
      settled: [
        commitment({
          entry_id: "le_untracked",
          state: "untracked",
          direction: "theirs",
          owner: "Priya",
          description: "send the revised deck",
        }),
      ],
    });

    await openCommitments(user);
    await user.click(await screen.findByRole("button", { name: /Settled/ }));

    expect(await screen.findByText("send the revised deck")).toBeInTheDocument();
    // It must not read as a closure: untracked says this was never your
    // business, where waived says it was and stopped mattering.
    expect(screen.getByText(/untracked/)).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /Reopen|It is still open/ }),
    ).toBeInTheDocument();
  });

  it("shows a parked claim with its evidence, and answers it either way", async () => {
    const user = userEvent.setup();
    serve({
      entries: [
        commitment({
          entry_id: "le_review",
          state: "needs_review",
          review_reason: "The source line was removed from n_a1b2c3.",
          evidence: [
            {
              evidence_id: "ev_1",
              source: "github",
              reference: "https://example.com/pull/42",
              confidence: 0.92,
              observed_at: "2026-08-15T00:00:00Z",
            },
          ],
        }),
      ],
    });
    onCommand("confirm_commitment_evidence", () => ({
      entry: {
        entry_id: "le_review",
        state: "closed",
        snoozed_until: null,
        closed_via: "github",
        review_reason: null,
        updated_at: "2026-08-17T12:00:00Z",
      },
      note_updated: true,
      note_annotated: true,
    }));

    await openCommitments(user);

    expect(
      await screen.findByText("The source line was removed from n_a1b2c3."),
    ).toBeInTheDocument();
    expect(screen.getByText(/92% confident/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "evidence" })).toHaveAttribute(
      "href",
      "https://example.com/pull/42",
    );
    // A row that is asking a question offers no checkbox: it asks a different
    // thing, and the two affordances beside it are the answer.
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Mark done" }));
    const call = invoke.mock.calls.find(
      ([command]) => command === "confirm_commitment_evidence",
    );
    expect(call?.[1]).toEqual({
      input: { entry_id: "le_review", evidence_id: "ev_1" },
    });
  });

  it("keeps a claim's entry open when it is dismissed", async () => {
    const user = userEvent.setup();
    serve({
      entries: [
        commitment({
          entry_id: "le_review",
          state: "needs_review",
          evidence: [
            {
              evidence_id: "ev_1",
              source: "conversation",
              reference: null,
              confidence: 0.5,
              observed_at: "2026-08-15T00:00:00Z",
            },
          ],
        }),
      ],
    });
    onCommand("dismiss_commitment_evidence", () => ({
      entry_id: "le_review",
      state: "open",
      snoozed_until: null,
      closed_via: null,
      review_reason: null,
      updated_at: "2026-08-17T12:00:00Z",
    }));

    await openCommitments(user);
    await user.click(await screen.findByRole("button", { name: "Keep open" }));

    expect(
      invoke.mock.calls.some(
        ([command]) => command === "dismiss_commitment_evidence",
      ),
    ).toBe(true);
  });

  it("annotates an auto-closed row and offers the undo", async () => {
    const user = userEvent.setup();
    serve({
      settled: [
        commitment({
          entry_id: "le_auto",
          state: "closed",
          closed_via: "github",
          item: item({ done: true }),
          evidence: [
            {
              evidence_id: "ev_1",
              source: "github",
              reference: "https://example.com/pull/42",
              confidence: 0.97,
              observed_at: "2026-08-16T00:00:00Z",
            },
          ],
        }),
      ],
    });
    onCommand("set_commitment_done", () => ({
      entry: {
        entry_id: "le_auto",
        state: "open",
        snoozed_until: null,
        closed_via: null,
        review_reason: null,
        updated_at: "2026-08-17T12:00:00Z",
      },
      note_updated: true,
    }));

    await openCommitments(user);
    await user.click(await screen.findByTestId("show-settled-commitments"));
    const shelf = await screen.findByTestId("settled-commitments");

    // Never silent: it says who closed it, and links what it saw.
    expect(within(shelf).getByText(/closed from GitHub/)).toBeInTheDocument();
    expect(within(shelf).getByRole("link", { name: "evidence" })).toHaveAttribute(
      "href",
      "https://example.com/pull/42",
    );

    await user.click(within(shelf).getByRole("button", { name: "Reopen" }));
    // The note owns done, so undoing a closure unticks the box rather than
    // leaving a ticked line over an open commitment.
    const call = invoke.mock.calls.find(
      ([command]) => command === "set_commitment_done",
    );
    expect(call?.[1]).toEqual({
      input: {
        entry_id: "le_auto",
        note_id: "n_a1b2c3",
        item_id: "a_111111",
        done: false,
      },
    });
  });

  it("surfaces a failed write under its own row and keeps the list", async () => {
    const user = userEvent.setup();
    serve({ entries: [commitment({ entry_id: "le_mine", item: item() })] });
    onCommand("set_commitment_done", () => {
      throw "The commitment ledger isn't available this session, so this change wasn't saved.";
    });

    await openCommitments(user);
    await user.click(
      await screen.findByRole("checkbox", { name: 'Mark "book the venue" done' }),
    );

    const alert = await screen.findByRole("alert");
    // The backend's finished copy, verbatim.
    expect(alert).toHaveTextContent(/wasn't saved/);
    expect(screen.getByText("book the venue")).toBeInTheDocument();
  });

  it("opens the source note, remembering where the reader came from", async () => {
    const user = userEvent.setup();
    serve({ entries: [commitment({ entry_id: "le_mine", item: item() })] });

    await openCommitments(user);
    await user.click(await screen.findByText("book the venue"));

    await waitFor(() => expect(lastNavigation).not.toBeNull());
    expect(lastNavigation).toMatchObject({
      kind: "noteEditor",
      noteId: "n_a1b2c3",
      project: "Briarwood Golf",
      origin: { kind: "commitments", slug: null },
    });
  });

  it("scopes to one project and says so in the summary", async () => {
    const user = userEvent.setup();
    serve({ entries: [commitment({ entry_id: "le_mine", item: item() })] });

    await openCommitments(user, "Briarwood Golf");

    await screen.findByText("book the venue");
    const call = invoke.mock.calls.find(
      ([command]) => command === "list_commitments",
    );
    expect(call?.[1]).toEqual({ project: "Briarwood Golf" });
    expect(screen.getByText(/Briarwood Golf/)).toBeInTheDocument();
  });

  it("lets a review with no claim behind it say the commitment still stands", async () => {
    // Sync parks an entry in review when its source line vanishes, and that
    // review carries no evidence to confirm or dismiss. Without this the entry
    // would sit in review forever.
    const user = userEvent.setup();
    serve({
      entries: [
        commitment({
          entry_id: "le_orphaned",
          state: "needs_review",
          review_reason: "The source line was removed from n_a1b2c3.",
        }),
      ],
    });
    onCommand("reopen_commitment", () => ({
      entry_id: "le_orphaned",
      state: "open",
      snoozed_until: null,
      closed_via: null,
      review_reason: null,
      updated_at: "2026-08-17T12:00:00Z",
    }));

    await openCommitments(user);
    await user.click(
      await screen.findByRole("button", { name: 'Actions for "book the venue"' }),
    );
    await user.click(
      await screen.findByRole("menuitem", { name: "It is still open" }),
    );

    await waitFor(() =>
      expect(
        invoke.mock.calls.some(([command]) => command === "reopen_commitment"),
      ).toBe(true),
    );
  });

  it("marks a commitment whose source line is gone as untickable", async () => {
    const user = userEvent.setup();
    serve({
      entries: [
        // No `item`: the line was edited away, and the entry survives it. This
        // is what the ledger is for, so the row still renders.
        commitment({ entry_id: "le_orphan", item: null, source: null }),
      ],
    });

    await openCommitments(user);

    const box = await screen.findByRole("checkbox", {
      name: 'Mark "book the venue" done',
    });
    expect(box).toBeDisabled();
  });

  it("names a commitment's tier once it stops being fresh", async () => {
    const user = userEvent.setup();
    serve({
      entries: [
        commitment({
          entry_id: "le_stale",
          description: "send the deck",
          tier: "stale",
          item: item({ description: "send the deck" }),
        }),
        commitment({
          entry_id: "le_aging",
          description: "chase the invoice",
          tier: "aging",
          item: item({ description: "chase the invoice" }),
        }),
        commitment({
          entry_id: "le_fresh",
          description: "book the venue",
          item: item(),
        }),
      ],
    });

    await openCommitments(user);

    const mine = await screen.findByTestId("commitments-mine");
    expect(within(mine).getByText(/stale · heard/)).toBeInTheDocument();
    expect(within(mine).getByText(/aging · heard/)).toBeInTheDocument();
    // Fresh says nothing: it is the absence of a problem, so a fresh row
    // spends no line telling you everything is fine.
    expect(within(mine).queryByText(/fresh/)).not.toBeInTheDocument();
  });

  it("promotes a stale row's age out of the faint run", async () => {
    const user = userEvent.setup();
    serve({
      entries: [
        commitment({
          entry_id: "le_stale",
          description: "send the deck",
          tier: "stale",
          item: item({ description: "send the deck" }),
        }),
      ],
    });

    await openCommitments(user);

    // Age reads as weight and position, never as a hue: the promotion is one
    // step up the ink ladder, the same treatment overdue gets.
    const mine = await screen.findByTestId("commitments-mine");
    expect(within(mine).getByText(/^stale · heard/)).toHaveClass("text-ink-dim");
  });

  it("leaves the promoted slot to overdue when a row is both", async () => {
    const user = userEvent.setup();
    serve({
      entries: [
        commitment({
          entry_id: "le_both",
          description: "chase the invoice",
          tier: "stale",
          item: item({
            description: "chase the invoice",
            due_date: "2026-08-10",
            status: "overdue",
          }),
        }),
      ],
    });

    await openCommitments(user);

    // One promoted segment per row. An overdue row spends it on overdue, so
    // the tier stays in the faint run rather than competing with it.
    const mine = await screen.findByTestId("commitments-mine");
    expect(within(mine).getByText(/^overdue · due/)).toHaveClass("text-ink-dim");
    expect(within(mine).getByText(/stale · heard/)).not.toHaveClass("text-ink-dim");
  });
});
