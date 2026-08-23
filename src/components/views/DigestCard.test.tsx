import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { VAULT_CHANGED_EVENT } from "../../events";
import {
  emitFromBackend,
  invokedCommands,
  onCommand,
  resetTauriMocks,
} from "../../test/tauri";
import type { DigestItem, DigestPayload } from "../../useDigest";
import { useLedgerChangedBridge } from "../../useLedgerChangedBridge";
import { useNavigation, viewKey } from "../../useNavigation";
import { useVaultChangedBridge } from "../../useVaultChangedBridge";
import { NavigationProvider } from "../providers/NavigationProvider";
import { DigestCard } from "./DigestCard";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

function item(
  overrides: Partial<DigestItem> & Pick<DigestItem, "entry_id" | "kind">,
): DigestItem {
  return {
    description: "Send the revised quote",
    owner: "You",
    project: "Briarwood",
    note_id: "n_a1b2c3",
    note_title: "Briarwood kickoff",
    due_date: null,
    last_mention: null,
    quiet_days: null,
    review_reason: null,
    ...overrides,
  };
}

function serve(payload: Partial<DigestPayload>): void {
  onCommand(
    "daily_digest",
    (): DigestPayload => ({
      date: payload.date ?? "2026-08-21",
      since: payload.since ?? "2026-08-20",
      items: payload.items ?? [],
      more: payload.more ?? 0,
    }),
  );
}

/** Reports where the shell would be, so a navigation is observable without
 * mounting the whole shell around one card. */
function ViewProbe() {
  const { view } = useNavigation();
  return <p data-testid="view-probe">{viewKey(view)}</p>;
}

/** The two relays AppShell mounts, which are what put a backend event onto the
 * bus `useVaultQuery` listens to. The digest's refetch depends on that path,
 * so the test exercises it rather than poking the bus directly. */
function VaultBridge() {
  useVaultChangedBridge();
  useLedgerChangedBridge();
  return null;
}

function renderCard() {
  render(
    <NavigationProvider>
      <VaultBridge />
      <DigestCard />
      <ViewProbe />
    </NavigationProvider>,
  );
}

describe("DigestCard", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("lists what changed, with the day it measures from", async () => {
    serve({
      since: "2026-08-20",
      items: [
        item({
          entry_id: "le_overdue",
          kind: "newly_overdue",
          due_date: "2026-08-20",
        }),
      ],
    });
    renderCard();

    const card = await screen.findByTestId("digest-card");
    expect(card).toHaveTextContent("1 change since Aug 20");
    expect(card).toHaveTextContent("Send the revised quote");
    expect(card).toHaveTextContent("Now overdue");
    expect(card).toHaveTextContent("was due Aug 20");
  });

  it("names each kind of change, because the label is the news", async () => {
    serve({
      items: [
        item({ entry_id: "le_1", kind: "newly_overdue", due_date: "2026-08-20" }),
        item({
          entry_id: "le_2",
          kind: "parked_in_review",
          review_reason: "a conversation reported this done",
        }),
        item({
          entry_id: "le_3",
          kind: "went_stale",
          last_mention: "2026-07-12T09:00:00Z",
        }),
        item({
          entry_id: "le_4",
          kind: "theirs_quiet",
          owner: "Priya",
          quiet_days: 12,
        }),
      ],
    });
    renderCard();

    const card = await screen.findByTestId("digest-card");
    expect(card).toHaveTextContent("Now overdue");
    expect(card).toHaveTextContent(
      "Needs review · a conversation reported this done",
    );
    expect(card).toHaveTextContent("Went stale · last mentioned Jul 12");
    expect(card).toHaveTextContent("Waiting on Priya · quiet 12 days");
  });

  it("counts what the cap dropped rather than hiding it", async () => {
    serve({
      items: [item({ entry_id: "le_1", kind: "went_stale" })],
      more: 3,
    });
    renderCard();

    expect(await screen.findByTestId("digest-card")).toHaveTextContent(
      "3 more not shown",
    );
    // The lead line counts everything that qualified, not just the rows.
    expect(screen.getByTestId("digest-card")).toHaveTextContent("4 changes");
  });

  it("renders nothing on a day with no news", async () => {
    serve({ items: [] });
    renderCard();

    await waitFor(() => {
      expect(invokedCommands()).toContain("daily_digest");
    });
    expect(screen.queryByTestId("digest-card")).not.toBeInTheDocument();
  });

  it("stays silent when the ledger cannot answer", async () => {
    // No handler registered: the command rejects. A digest that cannot be
    // computed and a day with nothing to report read the same on screen, and
    // neither may take the Inbox down with it.
    renderCard();

    await waitFor(() => {
      expect(invokedCommands()).toContain("daily_digest");
    });
    expect(screen.queryByTestId("digest-card")).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("picks up a new day's digest on the vault bus", async () => {
    serve({ items: [] });
    renderCard();

    await waitFor(() => {
      expect(invokedCommands()).toContain("daily_digest");
    });
    expect(screen.queryByTestId("digest-card")).not.toBeInTheDocument();

    // The command is compute-if-due, so a refetch after midnight is the whole
    // trigger: no timer watches the clock.
    serve({
      since: "2026-08-21",
      items: [item({ entry_id: "le_1", kind: "went_stale" })],
    });
    await act(async () => {
      emitFromBackend(VAULT_CHANGED_EVENT);
    });

    expect(await screen.findByTestId("digest-card")).toHaveTextContent(
      "1 change since Aug 21",
    );
  });

  it("offers the way to the commitments view and no per-row verbs", async () => {
    serve({ items: [item({ entry_id: "le_1", kind: "went_stale" })] });
    renderCard();

    await screen.findByTestId("digest-card");
    // One control on the whole card: the digest reports, the Commitments view
    // acts. A per-row verb here would be that view wearing a smaller hat.
    const buttons = screen.getAllByRole("button");
    expect(buttons).toHaveLength(1);
    expect(buttons[0]).toHaveTextContent("Open commitments");

    await userEvent.click(buttons[0]);
    expect(screen.getByTestId("view-probe")).toHaveTextContent(
      viewKey({ kind: "commitments", slug: null }),
    );
  });
});
