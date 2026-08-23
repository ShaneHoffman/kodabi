import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { InboxView } from "./InboxView";
import { CapturePipelineProvider } from "../providers/CapturePipelineProvider";
import { NavigationProvider } from "../providers/NavigationProvider";
import { INITIAL_VIEW, NavigationContext, type View } from "../../useNavigation";
import { DISTILL_STATE_EVENT } from "../../events";
import type { NoteSummary } from "../../useNotes";
import type { Project } from "../../useProjects";
import { notifyVaultChanged } from "../../useVaultQuery";
import { applyReduceMotion } from "../../reduceMotion";
import {
  emitFromBackend,
  invoke,
  invokedCommands,
  onCommand,
  resetTauriMocks,
} from "../../test/tauri";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

/* The capture/transcription event names the hooks keep private, spelled out
 * here for the same reason `CaptureToast.test.tsx` does: they are the
 * contract with Rust (`audio_cmds`, `transcribe_cmds`), not a test
 * convenience. */
const CAPTURE_STATE_EVENT = "capture:state";
const TRANSCRIPTION_STATE_EVENT = "transcription:state";

/** A run's opening progress event: the denominator is known from the start
 * (the backend has the recording's duration), the numerator is not yet. */
const TRANSCRIBING_AT_START = {
  status: "transcribing",
  seconds_processed: 0,
  total_seconds: 60,
};

const IDLE = { phase: "idle", sources: { loopback: "off", microphone: "off" } };
const STARTING = { phase: "starting", sources: { loopback: "off", microphone: "off" } };
const LISTENING = {
  phase: "listening",
  sources: { loopback: "live", microphone: "live" },
};

function makeNote(overrides: Partial<NoteSummary> & { id: string; title: string }): NoteSummary {
  return {
    path: `Inbox/${overrides.id}.md`,
    type: "note",
    project: null,
    date: "2026-07-01T09:00:00Z",
    tags: [],
    source: "manual",
    confidence: 0.41,
    category: null,
    category_confidence: null,
    tracking: null,
    snippet: "",
    // No suggestion by default: the router having nothing to say is the case
    // most of these tests are not about.
    guess: null,
    ...overrides,
  };
}

function makeProject(slug: string, note_count = 0): Project {
  return {
    id: slug,
    slug,
    display_name: slug,
    parent: null,
    note_count,
    meeting_count: 0,
  };
}

const PLANNING = makeNote({ id: "n_a1b2c3", title: "Quarterly planning" });
const VENDOR = makeNote({ id: "n_d4e5f6", title: "Vendor follow-up" });

/** Serve the two reads the view makes. Failed captures are deliberately not
 * among them: they moved to NeedsAttentionView, and this view's empty state no
 * longer depends on anything but its own list. */
function serveVault(
  notes: NoteSummary[],
  projects = ["briarwood-golf", "kodabi"],
): void {
  onCommand("list_notes", (args) => (args?.project === "Inbox" ? notes : []));
  onCommand("list_projects", () => ({
    inbox_note_count: notes.length,
    projects: projects.map(makeProject),
  }));
}

function renderInbox() {
  // The pipeline provider seeds from this on mount; left unrouted every test
  // would reject and log noise, and the placeholder would never settle on
  // "idle" for the not-showing assertions.
  onCommand("capture_phase", () => IDLE);
  return render(
    <NavigationProvider>
      <CapturePipelineProvider>
        <InboxView />
      </CapturePipelineProvider>
    </NavigationProvider>,
  );
}

/** Renders with a recording `navigate` in place of the real
 * `NavigationProvider`, for tests that only need to prove which `View` a
 * click resolves to — no `MainContent`/`NoteEditorView` required. */
function renderInboxWithNavigateSpy(): (view: View) => void {
  const navigate = vi.fn();
  onCommand("capture_phase", () => IDLE);
  render(
    <NavigationContext value={{ view: INITIAL_VIEW, navigate }}>
      <CapturePipelineProvider>
        <InboxView />
      </CapturePipelineProvider>
    </NavigationContext>,
  );
  return navigate;
}

/**
 * Wait for something the app has dropped from state to actually leave the DOM.
 *
 * The filed toast is the one element here whose departure `AnimatePresence`
 * owns, so it stays mounted until its exit finishes: "gone from the state" and
 * "gone from the document" are two moments. Even with animations skipped
 * (src/test/setup.ts) the completion that unmounts lands on a later tick of
 * motion's frame loop, not inside the commit that dropped it.
 *
 * Plain `waitFor` cannot close that gap — it polls, but it does not drive the
 * frame loop — and neither can a single fixed nudge, because `shouldAdvanceTime`
 * is moving the same clock underneath and any one advance is a race it
 * sometimes loses. This drives the loop explicitly instead, a frame at a time,
 * until the element is gone. The step stays far below every timer this view
 * runs (280 / 3500 / 10000), so settling an exit can never advance the state
 * machine along with it.
 */
async function waitForRemoval(find: () => HTMLElement | null): Promise<void> {
  const FRAME_MS = 20;
  const MAX_FRAMES = 20;
  for (let frame = 0; frame < MAX_FRAMES && find() !== null; frame += 1) {
    await act(async () => {
      await vi.advanceTimersByTimeAsync(FRAME_MS);
    });
  }
  expect(find()).toBeNull();
}

/**
 * The clock every `userEvent.setup` in this file hands to user-event. Async on
 * purpose.
 *
 * `advanceTimersByTime` fires timer and rAF callbacks back to back without
 * draining microtasks between them, so base-ui's menu-open sequence — a portal
 * mount, then floating-ui positioning, each waiting on a promise the next tick
 * resolves — cannot make progress inside one of user-event's advances. It got
 * there anyway on `shouldAdvanceTime`'s real-interval ticks, since those are
 * native and each is followed by a natural drain. Under full-suite load those
 * arrive late and sparse, and the menu misses its window: every `findBy*` here
 * is budgeted on the *fake* clock (Testing Library detects fake timers only
 * through a `jest` global, which vitest never defines whatever `globals` is set
 * to, so its waits fall through to the global `setTimeout` — the faked one),
 * and user-event's own advances spend that budget without the menu ever
 * mounting.
 *
 * The async advance drains microtasks between the timers it fires, so the
 * popup mounts inside those advances rather than in the gaps between them.
 */
async function advanceTimersForUserEvent(delay: number): Promise<void> {
  await vi.advanceTimersByTimeAsync(delay);
}

/** A row's File trigger — the whole rail hangs off it. `aria-label` carries
 * the note title, so one exact string identifies one card. */
function fileTrigger(title: string): HTMLElement {
  return screen.getByRole("button", { name: `File "${title}" to project` });
}

/** Open a row's File menu and choose `project`. */
async function fileNote(
  user: ReturnType<typeof userEvent.setup>,
  title: string,
  project: string,
): Promise<void> {
  await user.click(fileTrigger(title));
  await user.click(await screen.findByRole("menuitem", { name: new RegExp(`^${project}`) }));
}

/** The status span currently shown inside the placeholder — the crossfade
 * renders both texts of a pair always (one at zero opacity), so a plain
 * substring match can't tell which one is actually showing. */
function activeStatusText(): string | null {
  const placeholder = screen.getByTestId("pipeline-placeholder");
  return placeholder.querySelector("[data-visible]")?.textContent ?? null;
}

/** The meta (sub-)line's currently shown text — the second crossfading pair in
 * the card, below the status line `activeStatusText` reads. */
function activeMetaText(): string | null {
  const placeholder = screen.getByTestId("pipeline-placeholder");
  return placeholder.querySelectorAll("[data-visible]")[1]?.textContent ?? null;
}

/** What the placeholder actually announces: the sr-only line inside its
 * `role="status"` region. The visual crossfade is aria-hidden and always
 * carries both stage texts, so only this line's rewrite is an announcement. */
function announcedStatusText(): string | null {
  const placeholder = screen.getByTestId("pipeline-placeholder");
  return placeholder.querySelector(".sr-only")?.textContent ?? null;
}

/** The progress bar's fill width, as the inline style sets it. */
function progressFillWidth(): string | undefined {
  return screen.getByTestId("placeholder-progress-fill").style.width;
}

describe("InboxView", () => {
  beforeEach(() => {
    resetTauriMocks();
    vi.useFakeTimers({ shouldAdvanceTime: true });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("lists the notes waiting to be filed", async () => {
    serveVault([PLANNING, VENDOR]);

    renderInbox();

    expect(await screen.findByText("Quarterly planning")).toBeInTheDocument();
    expect(screen.getByText("Vendor follow-up")).toBeInTheDocument();
  });

  it("opens with the ledger's digest, above the queue", async () => {
    // The whole reason the card lives here: the Inbox is where the app opens,
    // so the ledger's news reaches the user without them going to look for it.
    serveVault([PLANNING]);
    onCommand("daily_digest", () => ({
      date: "2026-08-21",
      since: "2026-08-20",
      items: [
        {
          entry_id: "le_1",
          kind: "went_stale",
          description: "Draft the Q4 brief",
          owner: "You",
          project: "Briarwood",
          note_id: "n_a1b2c3",
          note_title: "Briarwood kickoff",
          due_date: null,
          last_mention: "2026-07-12T09:00:00Z",
          quiet_days: null,
          review_reason: null,
        },
      ],
      more: 0,
    }));

    renderInbox();

    const digest = await screen.findByTestId("digest-card");
    expect(digest).toHaveTextContent("Draft the Q4 brief");
    // Above the queue in document order, not merely present.
    const list = await screen.findByTestId("inbox-list");
    expect(digest.compareDocumentPosition(list)).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    );
  });

  it("shows no digest card on a day with nothing to report", async () => {
    serveVault([PLANNING]);
    onCommand("daily_digest", () => ({
      date: "2026-08-21",
      since: "2026-08-20",
      items: [],
      more: 0,
    }));

    renderInbox();

    await screen.findByText("Quarterly planning");
    expect(screen.queryByTestId("digest-card")).not.toBeInTheDocument();
  });

  it("says what failed and what happens next when the listing errors", async () => {
    serveVault([PLANNING]);
    onCommand("list_notes", () => {
      throw "the vault is unreadable";
    });

    renderInbox();

    // Both halves in one sentence, supplied by `useProjectNotes` rather than
    // assembled here: the listing's failures read the same to a reader whatever
    // the backend hit, so the hook fixes the copy and the rejection is dropped.
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(
      "Couldn't read your notes. They are still on disk; reopen this view to try again.",
    );
    // A failed read is not an empty inbox: the first-run copy must stay away.
    expect(screen.queryByTestId("inbox-list")).not.toBeInTheDocument();
  });

  /* The 14px between rows is not a `gap` on the list: it is an animated margin
   * on each slot, so it can collapse with the row that owns it. That makes it
   * the one piece of pure LAYOUT living inside a motion variant table, and a
   * table is chosen per reduced-motion setting — so the spacing of this list
   * now depends on a preference that should not touch spacing at all. Pinned
   * on both grounds because the failure is silent: leave the margin out of one
   * table and the whole Inbox goes flush, with nothing else to notice. */
  describe("the space between rows", () => {
    afterEach(() => {
      applyReduceMotion(false);
    });

    async function rowMargins(): Promise<(string | null)[]> {
      serveVault([PLANNING, VENDOR]);
      renderInbox();
      await screen.findByText("Quarterly planning");
      return [...screen.getByTestId("inbox-list").querySelectorAll("li")].map(
        (slot) => slot.style.marginBottom || null,
      );
    }

    it("is 14px on every row", async () => {
      expect(await rowMargins()).toEqual(["14px", "14px"]);
    });

    it("is still 14px when the user has asked us to hold still", async () => {
      applyReduceMotion(true);
      expect(await rowMargins()).toEqual(["14px", "14px"]);
    });
  });

  it("names a row's kind when it is not a plain note", async () => {
    serveVault([
      makeNote({
        id: "n_g7h8i9",
        title: "Irrigation contractor comparison",
        type: "chat",
        date: "2026-07-10T09:15:00-07:00",
        confidence: 0.62,
      }),
    ]);

    renderInbox();

    expect(await screen.findByText("chat · 2026-07-10")).toBeInTheDocument();
    // The gauge is restated in words beside it: colour and length alone are
    // not a reading (docs/DESIGN_SYSTEM.md §6).
    expect(screen.getByText("62% match")).toBeInTheDocument();
    expect(screen.getByTestId("inbox-row-gauge")).toHaveStyle({ width: "62%" });
  });

  it("says nothing about kind on a plain note's row", async () => {
    serveVault([PLANNING]);

    renderInbox();

    // The majority case: a `note` segment on every row would be noise.
    expect(await screen.findByText("2026-07-01")).toBeInTheDocument();
  });


  describe("the meeting kind", () => {
    const MEETING = makeNote({
      id: "n_m11111",
      title: "Weekly sync",
      type: "meeting",
      category: "standup",
    });

    function fileMenu(title: string): HTMLElement {
      return screen.getByRole("button", { name: `File "${title}" to project` });
    }

    it("names a classified meeting's genre on its row", async () => {
      serveVault([MEETING]);

      renderInbox();

      expect(await screen.findByText("meeting · Stand-up · 2026-07-01")).toBeInTheDocument();
    });

    it("says nothing extra for a meeting nothing has classified", async () => {
      serveVault([makeNote({ id: "n_m22222", title: "Weekly sync", type: "meeting" })]);

      renderInbox();

      expect(await screen.findByText("meeting · 2026-07-01")).toBeInTheDocument();
    });

    it("folds the kinds into the row's existing File menu", async () => {
      const user = userEvent.setup();
      serveVault([MEETING]);
      renderInbox();
      await screen.findByText("Weekly sync");

      // The row's two affordances are spent on File and Delete, so a third
      // control would break the ceiling; the kinds ride the menu instead.
      await user.click(fileMenu("Weekly sync"));

      expect(await screen.findByRole("menuitem", { name: /Working session/ })).toBeInTheDocument();
      expect(screen.getByRole("menuitem", { name: /briarwood-golf/ })).toBeInTheDocument();
    });

    it("offers no kinds on a row that is not a meeting", async () => {
      const user = userEvent.setup();
      serveVault([PLANNING]);
      renderInbox();
      await screen.findByText("Quarterly planning");

      await user.click(fileMenu("Quarterly planning"));

      await screen.findByRole("menuitem", { name: /briarwood-golf/ });
      expect(screen.queryByRole("menuitem", { name: /Working session/ })).toBeNull();
    });

    it("recategorizes without filing the note away", async () => {
      const user = userEvent.setup();
      serveVault([MEETING]);
      onCommand("set_note_category", () => ({ ...MEETING, category: "review" }));
      renderInbox();
      await screen.findByText("Weekly sync");

      await user.click(fileMenu("Weekly sync"));
      await user.click(await screen.findByRole("menuitem", { name: /Review/ }));

      await waitFor(() => {
        expect(invokedCommands()).toContain("set_note_category");
      });
      // A kind is not a filing: the row stays put rather than playing its
      // departure, and nothing was filed.
      expect(invokedCommands()).not.toContain("file_note_to_project");
      expect(screen.getByText("Weekly sync")).toBeInTheDocument();
    });

    it("surfaces a refused recategorization on the row", async () => {
      const user = userEvent.setup();
      serveVault([MEETING]);
      onCommand("set_note_category", () => {
        throw "Couldn't change this meeting's kind. The note is unchanged; try again.";
      });
      renderInbox();
      await screen.findByText("Weekly sync");

      await user.click(fileMenu("Weekly sync"));
      await user.click(await screen.findByRole("menuitem", { name: /Review/ }));

      expect(await screen.findByRole("alert")).toHaveTextContent(
        "Couldn't change this meeting's kind. The note is unchanged; try again.",
      );
    });
  });

  describe("the router's guess", () => {
    it("wears the guessed folder's hue on the edge and names it in words", async () => {
      serveVault([
        makeNote({
          id: "n_a1b2c3",
          title: "Sprinkler quotes",
          guess: { project: "briarwood-golf", confidence: 0.41 },
        }),
      ]);

      renderInbox();
      await screen.findByText("Sprinkler quotes");

      // `folderHue("briarwood-golf")` is pinned by useProjects.test.ts.
      expect(screen.getByTestId("inbox-card")).toHaveClass("border-l-coral");
      const guess = screen.getByTestId("inbox-row-guess");
      expect(guess).toHaveTextContent("→ briarwood-golf");
      // Same hue in the words as on the edge: one project, one colour.
      expect(guess).toHaveClass("text-coral");
      // And it keeps the hue under the pointer. `hover:glass-card-lift`
      // brightens the card's whole `border-color` from a `:hover` rule, which
      // outranks a bare `border-l-coral` on specificity — so without the
      // restatement the guess colour would drop off the edge exactly while
      // the card is being pointed at.
      expect(screen.getByTestId("inbox-card")).toHaveClass("hover:border-l-coral");
    });

    it("keeps its neutral edge under the pointer too", async () => {
      serveVault([makeNote({ id: "n_a1b2c3", title: "Ideas from the drive home" })]);

      renderInbox();
      await screen.findByText("Ideas from the drive home");

      expect(screen.getByTestId("inbox-card")).toHaveClass(
        "border-l-ink-faint",
        "hover:border-l-ink-faint",
      );
    });

    it("says so plainly when there is no confident guess", async () => {
      serveVault([makeNote({ id: "n_a1b2c3", title: "Ideas from the drive home" })]);

      renderInbox();
      await screen.findByText("Ideas from the drive home");

      expect(screen.getByTestId("inbox-card")).toHaveClass("border-l-ink-faint");
      expect(screen.getByTestId("inbox-row-guess")).toHaveTextContent("no confident guess");
    });

    it("ignores a guess too weak to be a claim", async () => {
      // A dead tie scores 0.0: the router still names a winner, but offering
      // it as a destination would dress a coin flip as an answer.
      serveVault([
        makeNote({
          id: "n_a1b2c3",
          title: "Ideas from the drive home",
          guess: { project: "briarwood-golf", confidence: 0.05 },
        }),
      ]);

      renderInbox();
      await screen.findByText("Ideas from the drive home");

      expect(screen.getByTestId("inbox-card")).toHaveClass("border-l-ink-faint");
      expect(screen.getByTestId("inbox-row-guess")).toHaveTextContent("no confident guess");
    });

    it("offers the guess first in the File menu, with the key that fires it", async () => {
      const user = userEvent.setup({ advanceTimers: advanceTimersForUserEvent });
      serveVault(
        [
          makeNote({
            id: "n_a1b2c3",
            title: "Sprinkler quotes",
            guess: { project: "briarwood-golf", confidence: 0.41 },
          }),
        ],
        ["kodabi", "briarwood-golf"],
      );

      renderInbox();
      await screen.findByText("Sprinkler quotes");
      await user.click(fileTrigger("Sprinkler quotes"));

      const items = await screen.findAllByRole("menuitem");
      // First, ahead of the alphabetically-earlier `kodabi`, and carrying the
      // ↵ that says Enter files here.
      expect(items[0]).toHaveTextContent("briarwood-golf");
      expect(items[0]).toHaveTextContent("↵");
      // Listed once: the suggestion is not repeated among the others.
      expect(items.filter((item) => item.textContent?.includes("briarwood-golf"))).toHaveLength(1);
    });

    it("files to the suggestion on Enter, which is what the ↵ hint promises", async () => {
      // The hint is a claim about the keyboard, so it is worth a keyboard
      // test: a ↵ beside a row that Enter does not reach would be a lie
      // drawn in the one colour reserved for the system's suggestions.
      const user = userEvent.setup({ advanceTimers: advanceTimersForUserEvent });
      serveVault(
        [
          makeNote({
            id: "n_a1b2c3",
            title: "Sprinkler quotes",
            guess: { project: "briarwood-golf", confidence: 0.41 },
          }),
        ],
        ["kodabi", "briarwood-golf"],
      );
      onCommand("file_note_to_project", () => ({
        note: { ...PLANNING, project: "briarwood-golf" },
        previous: { path: PLANNING.path, project: null },
        moved: true,
      }));
      renderInbox();
      await screen.findByText("Sprinkler quotes");

      // Opened from the keyboard, base-ui lands the highlight on the first
      // item — so Enter takes it. (Opened by CLICK it highlights nothing, and
      // Enter does nothing, which is right: a pointer user is not reaching
      // for a key they were never shown.)
      fileTrigger("Sprinkler quotes").focus();
      await user.keyboard("{ArrowDown}");
      await waitFor(() => {
        expect(screen.getByRole("menu")).toBeInTheDocument();
      });
      await user.keyboard("{Enter}");

      await waitFor(() => {
        expect(invoke).toHaveBeenCalledWith("file_note_to_project", {
          input: { id: "n_a1b2c3", project: "briarwood-golf" },
        });
      });
    });
  });

  it("carries one transition recipe at a time, so no leg loses its own clock", async () => {
    // `transition-[…]` and `duration-*` are each ONE CSS property: a second
    // utility for either is resolved by Tailwind's emission order, not by the
    // className, so the longest property list and the longest duration win
    // for every leg at once. Stacking the hover lift, the entrance and the
    // exit "so all three are ready" is therefore not additive — it silently
    // retires two of them. The same counting `Menu.test.tsx` does, for the
    // same failure: an instruction that quietly did not happen.
    serveVault([PLANNING]);

    renderInbox();
    await screen.findByText("Quarterly planning");

    const classes = Array.from(screen.getByTestId("inbox-card").classList);
    expect(classes.filter((name) => name.startsWith("transition-["))).toHaveLength(1);
    expect(classes.filter((name) => /^duration-\d/.test(name))).toHaveLength(1);
    // At rest that one recipe is the hover lift's, naming the two properties
    // `glass-card-lift` actually sets — a lift whose shadow snapped on while
    // the card eased upward would read as a drop shadow, not an object
    // rising.
    expect(classes).toContain("transition-[translate,box-shadow,border-color]");
    expect(classes).toContain("duration-180");
  });

  it("opens the note when the card body is clicked", async () => {
    const user = userEvent.setup({ advanceTimers: advanceTimersForUserEvent });
    serveVault([
      makeNote({
        id: "n_a1b2c3",
        title: "Quarterly planning",
        snippet: "Walked the Q3 budget line by line.",
      }),
    ]);
    const navigate = renderInboxWithNavigateSpy();
    await screen.findByText("Quarterly planning");

    // The snippet, not the title: the whole card is one button now, and the
    // text farthest from the old title-only target is what proves it.
    await user.click(screen.getByText("Walked the Q3 budget line by line."));

    expect(navigate).toHaveBeenCalledWith({
      kind: "noteEditor",
      noteId: "n_a1b2c3",
      project: "Inbox",
      // The note screen's back link returns to where the note was opened from,
      // so opening one says where that was.
      origin: { kind: "inbox" },
    });
  });

  it("opens the note from the keyboard: the card is one real button", async () => {
    const user = userEvent.setup({ advanceTimers: advanceTimersForUserEvent });
    serveVault([PLANNING]);
    const navigate = renderInboxWithNavigateSpy();
    await screen.findByText("Quarterly planning");

    // Anchored to the start: the card's accessible name is its whole text
    // (title, then meta), so `^title` picks the card, not the row's own
    // `Delete "Quarterly planning"` button. One tab stop, Enter — the entire
    // keyboard path.
    screen.getByRole("button", { name: /^Quarterly planning/ }).focus();
    await user.keyboard("{Enter}");

    expect(navigate).toHaveBeenCalledWith({
      kind: "noteEditor",
      noteId: "n_a1b2c3",
      project: "Inbox",
      // The note screen's back link returns to where the note was opened from,
      // so opening one says where that was.
      origin: { kind: "inbox" },
    });
  });

  it("files without navigating: the picker sits over the card, not inside it", async () => {
    const user = userEvent.setup({ advanceTimers: advanceTimersForUserEvent });
    serveVault([PLANNING]);
    onCommand("file_note_to_project", () => ({
      note: { ...PLANNING, project: "briarwood-golf", path: "Projects/briarwood-golf/planning.md" },
      previous: { path: PLANNING.path, project: null },
      moved: true,
    }));
    const navigate = renderInboxWithNavigateSpy();
    await screen.findByText("Quarterly planning");

    await fileNote(user, "Quarterly planning", "briarwood-golf");

    expect(invoke).toHaveBeenCalledWith("file_note_to_project", {
      input: { id: "n_a1b2c3", project: "briarwood-golf" },
    });
    expect(navigate).not.toHaveBeenCalled();
  });

  it("re-routes a note to the chosen project and drops the row once the vault refetches", async () => {
    const user = userEvent.setup({ advanceTimers: advanceTimersForUserEvent });
    serveVault([PLANNING, VENDOR]);
    onCommand("file_note_to_project", () => ({
      note: { ...PLANNING, project: "briarwood-golf", path: "Projects/briarwood-golf/planning.md" },
      previous: { path: PLANNING.path, project: null },
      moved: true,
    }));
    renderInbox();
    await screen.findByText("Quarterly planning");

    await fileNote(user, "Quarterly planning", "briarwood-golf");

    // The correction itself: note id in, project slug out, nested under the
    // `input` key the Rust DTO expects.
    expect(invoke).toHaveBeenCalledWith("file_note_to_project", {
      input: { id: "n_a1b2c3", project: "briarwood-golf" },
    });

    // The backend broadcasts `vault:changed` after a re-route; the bridge turns
    // that into the DOM event this hook fans out on. Once the list refetches
    // without the filed note, its row is gone.
    serveVault([VENDOR]);
    act(() => {
      notifyVaultChanged();
    });

    await waitFor(() => {
      expect(screen.queryByText("Quarterly planning")).not.toBeInTheDocument();
    });
    expect(screen.getByText("Vendor follow-up")).toBeInTheDocument();
  });

  it("keeps the row and surfaces the message when the re-route fails", async () => {
    const user = userEvent.setup({ advanceTimers: advanceTimersForUserEvent });
    serveVault([PLANNING]);
    onCommand("file_note_to_project", () => {
      throw "That project does not exist. It may have been renamed or deleted; refresh the list and try again.";
    });
    renderInbox();
    await screen.findByText("Quarterly planning");

    await fileNote(user, "Quarterly planning", "briarwood-golf");

    // Exact, and the whole line: a command's rejection is finished user copy
    // now (`src-tauri/src/user_errors.rs`), so the row renders it as-is rather
    // than behind a prefix of its own. The old spelling here concatenated a
    // developer string onto "Couldn't file this note: ", which is the leak
    // docs/DESIGN_SYSTEM.md §3 forbids.
    expect(
      await screen.findByText(
        "That project does not exist. It may have been renamed or deleted; refresh the list and try again.",
      ),
    ).toBeInTheDocument();
    // The note is still unfiled, so it must still be actionable: row present
    // and the trigger back (not stuck on "Filing…").
    expect(screen.getByText("Quarterly planning")).toBeInTheDocument();
    expect(fileTrigger("Quarterly planning")).not.toHaveAttribute("aria-busy");
  });

  it("still offers filing in a project-less vault, because the menu can make one", async () => {
    const user = userEvent.setup({ advanceTimers: advanceTimersForUserEvent });
    serveVault([PLANNING], []);

    renderInbox();
    await screen.findByText("Quarterly planning");

    // The old gate hid the picker when there was nowhere to file, because a
    // control whose only outcome is a dead end should not be offered. The
    // menu's foot is the better fix: there is always somewhere to file, if
    // you are willing to make it.
    await user.click(fileTrigger("Quarterly planning"));
    const items = await screen.findAllByRole("menuitem");
    expect(items).toHaveLength(1);
    expect(items[0]).toHaveTextContent("New project…");
  });

  it("creates a project from the menu's foot and files the note into it", async () => {
    const user = userEvent.setup({ advanceTimers: advanceTimersForUserEvent });
    serveVault([PLANNING], []);
    onCommand("create_project", () => makeProject("riverbend-deck"));
    onCommand("file_note_to_project", () => ({
      note: { ...PLANNING, project: "riverbend-deck" },
      previous: { path: PLANNING.path, project: null },
      moved: true,
    }));
    renderInbox();
    await screen.findByText("Quarterly planning");

    await user.click(fileTrigger("Quarterly planning"));
    await user.click(await screen.findByRole("menuitem", { name: "New project…" }));
    const dialog = await screen.findByRole("dialog", { name: "New project" });
    await user.type(within(dialog).getByLabelText("Project name"), "riverbend-deck");
    await user.click(within(dialog).getByRole("button", { name: "Create" }));

    // The errand was "file this note somewhere new", so creating the project
    // finishes it rather than leaving the note where it was.
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("file_note_to_project", {
        input: { id: "n_a1b2c3", project: "riverbend-deck" },
      });
    });
  });

  it("says nothing is waiting when the inbox is empty", async () => {
    serveVault([]);

    renderInbox();

    expect(await screen.findByText(/Nothing waiting/)).toBeInTheDocument();
  });

  it("states the workload in the header, and only when there is one", async () => {
    // The header says what the view is FOR before the list is read. At zero it
    // says nothing: the empty state below is already that sentence, and a
    // header that also says "nothing" says it worse.
    serveVault([PLANNING, VENDOR]);

    const { unmount } = renderInbox();

    // Both halves of the state on one line: what is left, and what you have
    // already done about it.
    expect(await screen.findByText("2 unfiled · 0 filed this session")).toBeInTheDocument();
    unmount();

    resetTauriMocks();
    serveVault([]);
    renderInbox();

    await screen.findByText(/Nothing waiting/);
    expect(screen.queryByText(/unfiled/)).not.toBeInTheDocument();
  });

  it("counts a filed note into the session tally and lights the meter", async () => {
    const user = userEvent.setup({ advanceTimers: advanceTimersForUserEvent });
    serveVault([PLANNING, VENDOR]);
    onCommand("file_note_to_project", () => ({
      note: { ...PLANNING, project: "briarwood-golf" },
      previous: { path: PLANNING.path, project: null },
      moved: true,
    }));
    renderInbox();
    // Both rows, asserted rather than awaited: one `list_notes` read serves the
    // whole queue, so they land in the same commit and the second find cannot
    // wait on anything. It is here because this is the one filing test with
    // more than one note in the queue, and the tally assertions below are only
    // meaningful if the queue really started at two.
    await screen.findByText("Quarterly planning");
    expect(screen.getByText("Vendor follow-up")).toBeInTheDocument();

    // Nothing cleared yet: an unlit meter over a full queue is the honest
    // starting state.
    const segments = () =>
      Array.from(screen.getByTestId("inbox-meter").children) as HTMLElement[];
    expect(segments()).toHaveLength(5);
    expect(segments().filter((segment) => segment.dataset.lit)).toHaveLength(0);

    await fileNote(user, "Quarterly planning", "briarwood-golf");
    serveVault([VENDOR]);
    act(() => {
      notifyVaultChanged();
    });

    await waitFor(() => {
      expect(screen.getByText("1 unfiled · 1 filed this session")).toBeInTheDocument();
    });
    // Half the queue cleared, and the lit segments are INK — green is the
    // kodama's voice, never progress (docs/DESIGN_SYSTEM.md §2).
    const lit = segments().filter((segment) => segment.dataset.lit);
    expect(lit).toHaveLength(3);
    expect(lit[0]).toHaveClass("bg-ink/50");
  });

  it("holds no needs-attention queue of its own", async () => {
    // The Inbox has one job. Failed captures are a different job with a
    // different verb, and they now live in their own view; even served one,
    // this view must not grow a second queue back.
    serveVault([PLANNING]);
    onCommand("list_failed_sessions", () => [
      {
        path: "sessions/2026-07-01T10-00-00Z-team-sync.jsonl",
        file_name: "2026-07-01T10-00-00Z-team-sync.jsonl",
        slug: "team-sync",
        captured_at: "2026-07-01T10:00:00Z",
        dismissed: false,
      },
    ]);

    renderInbox();
    await screen.findByText("Quarterly planning");

    expect(screen.queryByTestId("needs-attention")).not.toBeInTheDocument();
    expect(screen.queryByTestId("retry-distill")).not.toBeInTheDocument();
  });

  it("surfaces a failed listing instead of claiming an empty inbox, in the user's words", async () => {
    // The leak pin for a view-level StatusMessage. The thrown value is shaped
    // like the developer-facing string a wrapper would emit if the translation
    // in `user_errors.rs` were reverted; the assertion is that none of it
    // reaches the screen (docs/DESIGN_SYSTEM.md §3), and that the listing still
    // refuses to read as "nothing here".
    onCommand("list_notes", () => {
      throw "note I/O failed at C:\\Users\\someone\\kb\\Inbox: Access is denied. (os error 5)";
    });
    onCommand("list_projects", () => ({ inbox_note_count: 0, projects: [] }));

    renderInbox();

    expect(
      await screen.findByText(
        "Couldn't read your notes. They are still on disk; reopen this view to try again.",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/os error|C:\\|Access is denied/)).toBeNull();
    expect(screen.queryByText(/Nothing waiting/)).not.toBeInTheDocument();
  });

  describe("the delete affordance", () => {
    it("offers delete even when there is no project to file into", async () => {
      serveVault([PLANNING], []); // a project-less vault: nowhere to file
      renderInbox();
      await screen.findByText("Quarterly planning");

      // An unwanted note must be removable wherever it sits, whatever the
      // rest of the vault looks like.
      expect(
        screen.getByRole("button", { name: 'Delete "Quarterly planning"' }),
      ).toBeInTheDocument();
    });

    it("cancels without deleting", async () => {
      const user = userEvent.setup({ advanceTimers: advanceTimersForUserEvent });
      serveVault([PLANNING]);
      renderInbox();
      await screen.findByText("Quarterly planning");

      await user.click(
        screen.getByRole("button", { name: 'Delete "Quarterly planning"' }),
      );
      await user.click(screen.getByRole("button", { name: "Cancel" }));

      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
      expect(invokedCommands()).not.toContain("delete_note");
    });

    it("deletes by id, and the row leaves once the vault refetches", async () => {
      const user = userEvent.setup({ advanceTimers: advanceTimersForUserEvent });
      serveVault([PLANNING, VENDOR]);
      onCommand("delete_note", () => ({
        id: "n_a1b2c3",
        title: "Quarterly planning",
        project: null,
        session_deleted: false,
      }));
      renderInbox();
      await screen.findByText("Quarterly planning");

      await user.click(
        screen.getByRole("button", { name: 'Delete "Quarterly planning"' }),
      );
      const dialog = screen.getByRole("dialog", { name: "Delete this note?" });
      await user.click(within(dialog).getByRole("button", { name: "Delete note" }));

      await waitFor(() => {
        expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
      });
      expect(invoke).toHaveBeenCalledWith("delete_note", { id: "n_a1b2c3" });

      // The backend broadcasts vault:changed; the list refetches without the
      // deleted note, so its row (and the count) are gone.
      serveVault([VENDOR]);
      act(() => {
        notifyVaultChanged();
      });
      await waitFor(() => {
        expect(screen.queryByText("Quarterly planning")).not.toBeInTheDocument();
      });
      expect(screen.getByText("Vendor follow-up")).toBeInTheDocument();
    });

    it("warns that a session-backed note's recording is deleted too", async () => {
      const user = userEvent.setup({ advanceTimers: advanceTimersForUserEvent });
      const DISTILLED = makeNote({
        id: "n_s1s2s3",
        title: "Team sync",
        source: "sessions/2026-07-01T10-00-00Z-team-sync.jsonl",
      });
      serveVault([DISTILLED]);
      renderInbox();
      await screen.findByText("Team sync");

      await user.click(screen.getByRole("button", { name: 'Delete "Team sync"' }));

      const dialog = screen.getByRole("dialog", { name: "Delete this note?" });
      expect(
        within(dialog).getByText(/recording and transcript/),
      ).toBeInTheDocument();
    });

    it("keeps the row and shows the error when the delete fails", async () => {
      const user = userEvent.setup({ advanceTimers: advanceTimersForUserEvent });
      serveVault([PLANNING]);
      onCommand("delete_note", () => {
        throw "Couldn't delete the note. It is untouched; try again.";
      });
      renderInbox();
      await screen.findByText("Quarterly planning");

      await user.click(
        screen.getByRole("button", { name: 'Delete "Quarterly planning"' }),
      );
      const dialog = screen.getByRole("dialog", { name: "Delete this note?" });
      await user.click(within(dialog).getByRole("button", { name: "Delete note" }));

      expect(
        await within(dialog).findByText(
          "Couldn't delete the note. It is untouched; try again.",
        ),
      ).toBeInTheDocument();
      // Scoped to the list: the open dialog names the note too, in its subject
      // strip, so an unscoped query would match either one.
      expect(
        within(screen.getByTestId("inbox-list")).getByText("Quarterly planning"),
      ).toBeInTheDocument();
    });
  });

  describe("the pipeline placeholder", () => {
    it("appears the instant a capture stops, before the backend says anything", async () => {
      serveVault([]);
      renderInbox();
      await screen.findByText(/Nothing waiting/);

      await act(async () => {
        emitFromBackend(CAPTURE_STATE_EVENT, LISTENING);
      });
      await act(async () => {
        emitFromBackend(CAPTURE_STATE_EVENT, IDLE);
      });

      // Not a single transcription or distill event has fired: the witnessed
      // stop alone mounts the placeholder.
      expect(screen.getByTestId("pipeline-placeholder")).toBeInTheDocument();
      expect(activeStatusText()).toBe("Transcribing the capture");
      expect(announcedStatusText()).toBe("Transcribing the capture");
      // Work is starting, not waiting: nothing here says "queued".
      expect(activeMetaText()).toContain("just stopped · processing audio");
      expect(activeMetaText()).not.toContain("queued");
    });

    it("says queued only while a run is parked behind another, and never gives up on it", async () => {
      serveVault([]);
      renderInbox();
      await screen.findByText(/Nothing waiting/);

      await act(async () => {
        emitFromBackend(CAPTURE_STATE_EVENT, LISTENING);
      });
      await act(async () => {
        emitFromBackend(CAPTURE_STATE_EVENT, IDLE);
      });
      await act(async () => {
        emitFromBackend(TRANSCRIPTION_STATE_EVENT, { status: "queued" });
      });

      expect(activeMetaText()).toContain("queued behind the previous capture");
      // The heading and the announcement still describe the phase, not the beat.
      expect(activeStatusText()).toBe("Transcribing the capture");
      expect(announcedStatusText()).toBe("Transcribing the capture");

      // A predecessor can hold the lock for minutes; the grace timer must not
      // retire a run that has already acknowledged the stop.
      await act(async () => {
        vi.advanceTimersByTime(10_000); // GRACE_MS
      });
      expect(screen.getByTestId("pipeline-placeholder")).toBeInTheDocument();
    });

    it("shows real progress against the recording, then names the cleanup stage", async () => {
      serveVault([]);
      renderInbox();
      await screen.findByText(/Nothing waiting/);

      await act(async () => {
        emitFromBackend(CAPTURE_STATE_EVENT, LISTENING);
      });
      await act(async () => {
        emitFromBackend(CAPTURE_STATE_EVENT, IDLE);
      });
      await act(async () => {
        emitFromBackend(TRANSCRIPTION_STATE_EVENT, {
          status: "transcribing",
          seconds_processed: 750,
          total_seconds: 3484,
        });
      });

      expect(activeMetaText()).toContain("processing audio · 12:30 of 58:04");
      expect(progressFillWidth()).toBe(`${(750 / 3484) * 100}%`);
      // The bar is a visual aid only: the announcement is unchanged, and
      // nothing in the bar reaches an assistive reader.
      expect(announcedStatusText()).toBe("Transcribing the capture");
      expect(
        screen.getByTestId("placeholder-progress-fill").closest("[aria-hidden='true']"),
      ).not.toBeNull();
      expect(
        screen.getByTestId("pipeline-placeholder").querySelectorAll(".sr-only"),
      ).toHaveLength(1);

      // The audio is through the engines but the run isn't over: a bar that
      // simply stopped at full would read as a hang.
      await act(async () => {
        emitFromBackend(TRANSCRIPTION_STATE_EVENT, {
          status: "transcribing",
          seconds_processed: 3484,
          total_seconds: 3484,
        });
      });
      expect(activeMetaText()).toContain("cleaning up transcript");
      expect(progressFillWidth()).toBe("100%");
    });

    it("holds through the backend's own transcribing without being waived", async () => {
      serveVault([]);
      renderInbox();
      await screen.findByText(/Nothing waiting/);

      await act(async () => {
        emitFromBackend(CAPTURE_STATE_EVENT, LISTENING);
      });
      await act(async () => {
        emitFromBackend(CAPTURE_STATE_EVENT, IDLE);
      });
      await act(async () => {
        emitFromBackend(TRANSCRIPTION_STATE_EVENT, TRANSCRIBING_AT_START);
      });
      expect(activeStatusText()).toBe("Transcribing the capture");

      // The real stage is not waiver-eligible: a genuine transcription can
      // run for minutes.
      await act(async () => {
        vi.advanceTimersByTime(10_000); // GRACE_MS
      });
      expect(screen.getByTestId("pipeline-placeholder")).toBeInTheDocument();
    });

    it("stays hidden when a start fails without ever recording", async () => {
      serveVault([]);
      renderInbox();
      await screen.findByText(/Nothing waiting/);

      await act(async () => {
        emitFromBackend(CAPTURE_STATE_EVENT, STARTING);
      });
      await act(async () => {
        emitFromBackend(CAPTURE_STATE_EVENT, IDLE);
      });
      expect(screen.queryByTestId("pipeline-placeholder")).not.toBeInTheDocument();
    });

    it("gives up on a stop the backend never acknowledges", async () => {
      // A mis-tap: a session under the backend's minimum duration is dropped
      // without a single event.
      serveVault([]);
      renderInbox();
      await screen.findByText(/Nothing waiting/);

      await act(async () => {
        emitFromBackend(CAPTURE_STATE_EVENT, LISTENING);
      });
      await act(async () => {
        emitFromBackend(CAPTURE_STATE_EVENT, IDLE);
      });
      expect(screen.getByTestId("pipeline-placeholder")).toBeInTheDocument();

      await act(async () => {
        vi.advanceTimersByTime(10_000); // GRACE_MS
      });
      expect(screen.queryByTestId("pipeline-placeholder")).not.toBeInTheDocument();
      expect(await screen.findByText(/Nothing waiting/)).toBeInTheDocument();
    });

    it("does not replay a given-up stop when the Inbox remounts", async () => {
      serveVault([]);
      onCommand("capture_phase", () => IDLE);
      const { rerender } = render(
        <NavigationProvider>
          <CapturePipelineProvider>
            <InboxView />
          </CapturePipelineProvider>
        </NavigationProvider>,
      );
      await screen.findByText(/Nothing waiting/);

      await act(async () => {
        emitFromBackend(CAPTURE_STATE_EVENT, LISTENING);
      });
      await act(async () => {
        emitFromBackend(CAPTURE_STATE_EVENT, IDLE);
      });
      await act(async () => {
        vi.advanceTimersByTime(10_000); // GRACE_MS
      });
      expect(screen.queryByTestId("pipeline-placeholder")).not.toBeInTheDocument();

      // Navigate away and back. The provider — and `stopPending`, which
      // would otherwise persist until the next capture — survive the view
      // unmounting, so without the shell retiring the given-up stop, this
      // would replay the phantom placeholder for another grace period.
      rerender(
        <NavigationProvider>
          <CapturePipelineProvider>
            <div />
          </CapturePipelineProvider>
        </NavigationProvider>,
      );
      rerender(
        <NavigationProvider>
          <CapturePipelineProvider>
            <InboxView />
          </CapturePipelineProvider>
        </NavigationProvider>,
      );
      expect(await screen.findByText(/Nothing waiting/)).toBeInTheDocument();
      expect(screen.queryByTestId("pipeline-placeholder")).not.toBeInTheDocument();
    });

    it("keeps the placeholder when transcribing lands before the stop's own broadcast", async () => {
      serveVault([]);
      renderInbox();
      await screen.findByText(/Nothing waiting/);

      await act(async () => {
        emitFromBackend(CAPTURE_STATE_EVENT, LISTENING);
      });
      // The worker's opening event beats the `capture:state idle` broadcast —
      // it must not be dropped as a stale straggler.
      await act(async () => {
        emitFromBackend(TRANSCRIPTION_STATE_EVENT, TRANSCRIBING_AT_START);
      });
      await act(async () => {
        emitFromBackend(CAPTURE_STATE_EVENT, IDLE);
      });
      expect(activeStatusText()).toBe("Transcribing the capture");

      // On the real stage, not the synthetic one, so the grace timer never
      // retires it mid-run.
      await act(async () => {
        vi.advanceTimersByTime(10_000); // GRACE_MS
      });
      expect(screen.getByTestId("pipeline-placeholder")).toBeInTheDocument();

      await act(async () => {
        emitFromBackend(TRANSCRIPTION_STATE_EVENT, { status: "saved", path: "t.jsonl" });
      });
      expect(activeStatusText()).toBe("Distilling the meeting");
    });

    it("wears a row's silhouette and advances as the pipeline does", async () => {
      serveVault([PLANNING]);
      renderInbox();
      await screen.findByText("Quarterly planning");

      await act(async () => {
        emitFromBackend(TRANSCRIPTION_STATE_EVENT, TRANSCRIBING_AT_START);
      });
      expect(activeStatusText()).toBe("Transcribing the capture");
      expect(announcedStatusText()).toBe("Transcribing the capture");

      // The gap before distill actually picks up is sub-millisecond in a real
      // build, so the placeholder already reads "distilling" here.
      await act(async () => {
        emitFromBackend(TRANSCRIPTION_STATE_EVENT, { status: "saved", path: "t.jsonl" });
      });
      expect(activeStatusText()).toBe("Distilling the meeting");
      // The announcement line's text genuinely rewrites — a class flip alone
      // would announce nothing — and carries exactly one stage at a time.
      expect(announcedStatusText()).toBe("Distilling the meeting");

      await act(async () => {
        emitFromBackend(DISTILL_STATE_EVENT, { status: "distilling" });
      });
      expect(activeStatusText()).toBe("Distilling the meeting");
    });

    it("shows over an empty inbox without the empty-state message", async () => {
      serveVault([]);
      renderInbox();
      await screen.findByText(/Nothing waiting/);

      await act(async () => {
        emitFromBackend(TRANSCRIPTION_STATE_EVENT, TRANSCRIBING_AT_START);
      });

      expect(screen.getByTestId("pipeline-placeholder")).toBeInTheDocument();
      expect(screen.queryByText(/Nothing waiting/)).not.toBeInTheDocument();
    });

    it("hands off silently to the real row, with its fill-in, once a note routed to the Inbox appears", async () => {
      serveVault([PLANNING]);
      renderInbox();
      await screen.findByText("Quarterly planning");

      // The backend path is absolute with the OS's own separators; the note
      // list's path is KB-relative with forward slashes.
      await act(async () => {
        emitFromBackend(DISTILL_STATE_EVENT, {
          status: "saved",
          path: "C:\\kb\\Inbox\\n_g7h8i9.md",
          session_path: "s2.jsonl",
        });
      });
      expect(activeStatusText()).toBe("Distilling the meeting");

      const NEW_NOTE = makeNote({ id: "n_g7h8i9", title: "New capture" });
      serveVault([NEW_NOTE, PLANNING]);
      act(() => {
        notifyVaultChanged();
      });

      await waitFor(() => {
        expect(screen.queryByTestId("pipeline-placeholder")).not.toBeInTheDocument();
      });
      const rows = screen.getAllByText("New capture");
      expect(rows).toHaveLength(1);
      // The arriving row plays the entrance the placeholder was holding its
      // place for, so the handoff reads as a fill-in rather than a swap. The
      // placeholder itself leaves without an exit of its own — silently, in
      // the same commit — which is what makes the two read as one note
      // resolving rather than as one leaving and another arriving.
      expect(rows[0].closest("[data-testid='inbox-card']")).toHaveAttribute("data-fresh");
    });

    it("vanishes immediately and hands off to a toast when distill routes elsewhere", async () => {
      serveVault([]);
      renderInbox();
      await screen.findByText(/Nothing waiting/);

      await act(async () => {
        emitFromBackend(DISTILL_STATE_EVENT, {
          status: "saved",
          path: "/kb/briarwood-golf/meeting.md",
          session_path: "s1.jsonl",
        });
      });
      // No dwell on the row itself any more: it starts leaving at once, and
      // the toast is ALREADY up while it does. That overlap is the point —
      // the eye follows the card out to the left and finds the answer
      // arriving in the corner, with no beat in between where the app has
      // nothing to say.
      expect(screen.getByTestId("pipeline-placeholder")).toBeInTheDocument();
      const toast = screen.getByRole("button", {
        name: 'Open the note filed to "briarwood-golf"',
      });
      expect(toast).toHaveTextContent("Filed to");
      expect(toast).toHaveTextContent("briarwood-golf");
      // Scoped to the toast: the placeholder is still up alongside it (that
      // overlap is the point), and it carries a `role="status"` of its own.
      expect(within(toast).getByRole("status")).toHaveTextContent("briarwood-golf");
      // The hue of the folder it landed in, carried over from the card that
      // wore it on the way out.
      expect(toast).toHaveClass("border-l-coral");

      await act(async () => {
        vi.advanceTimersByTime(280); // VANISH_MS
      });
      expect(screen.queryByTestId("pipeline-placeholder")).not.toBeInTheDocument();

      // The dwell ends and the toast is dropped from state; its own fade is
      // the exit variant's, played on the way out rather than announced by a
      // class first.
      await act(async () => {
        vi.advanceTimersByTime(3500); // TOAST_DWELL_MS
      });
      await waitForRemoval(() =>
        screen.queryByRole("button", { name: /Open the note filed to/ }),
      );
    });

    it("pauses the toast's dwell while hovered, and resumes it on unhover", async () => {
      const user = userEvent.setup({ advanceTimers: advanceTimersForUserEvent });
      serveVault([]);
      renderInbox();
      await screen.findByText(/Nothing waiting/);

      await act(async () => {
        emitFromBackend(DISTILL_STATE_EVENT, {
          status: "saved",
          path: "/kb/briarwood-golf/meeting.md",
          session_path: "s6.jsonl",
        });
      });
      const toast = screen.getByRole("button", { name: /Open the note filed to/ });

      await user.hover(toast);
      await act(async () => {
        vi.advanceTimersByTime(5000); // well past TOAST_DWELL_MS
      });
      expect(
        screen.getByRole("button", { name: /Open the note filed to/ }),
      ).toBeInTheDocument();

      await user.unhover(toast);
      await act(async () => {
        vi.advanceTimersByTime(3500); // TOAST_DWELL_MS, restarted
      });
      await waitForRemoval(() =>
        screen.queryByRole("button", { name: /Open the note filed to/ }),
      );
    });

    it("opens the filed note when the toast is clicked", async () => {
      const user = userEvent.setup({ advanceTimers: advanceTimersForUserEvent });
      onCommand("list_projects", () => ({
        inbox_note_count: 0,
        projects: [makeProject("briarwood-golf")],
      }));
      const FILED = makeNote({
        id: "n_x9y8z7",
        title: "Vendor sync",
        path: "briarwood-golf/vendor-sync.md",
      });
      onCommand("list_notes", (args) =>
        args?.project === "briarwood-golf" ? [FILED] : [],
      );
      const navigate = renderInboxWithNavigateSpy();
      await screen.findByText(/Nothing waiting/);

      await act(async () => {
        emitFromBackend(DISTILL_STATE_EVENT, {
          status: "saved",
          path: "C:\\kb\\briarwood-golf\\vendor-sync.md",
          session_path: "s7.jsonl",
        });
      });

      await user.click(screen.getByRole("button", { name: /Open the note filed to/ }));

      await waitFor(() => {
        expect(navigate).toHaveBeenCalledWith({
          kind: "noteEditor",
          noteId: "n_x9y8z7",
          project: "briarwood-golf",
          origin: { kind: "inbox" },
        });
      });
    });

    it("falls back to the project view when the filed note can't be found", async () => {
      const user = userEvent.setup({ advanceTimers: advanceTimersForUserEvent });
      onCommand("list_projects", () => ({
        inbox_note_count: 0,
        projects: [makeProject("briarwood-golf")],
      }));
      // Nothing matches the saved path — moved, renamed, or the lookup fails.
      onCommand("list_notes", () => []);
      const navigate = renderInboxWithNavigateSpy();
      await screen.findByText(/Nothing waiting/);

      await act(async () => {
        emitFromBackend(DISTILL_STATE_EVENT, {
          status: "saved",
          path: "C:\\kb\\briarwood-golf\\vendor-sync.md",
          session_path: "s8.jsonl",
        });
      });

      await user.click(screen.getByRole("button", { name: /Open the note filed to/ }));

      await waitFor(() => {
        expect(navigate).toHaveBeenCalledWith({ kind: "project", slug: "briarwood-golf" });
      });
    });

    it("collapses without comment on a skipped or failed distill", async () => {
      serveVault([]);
      renderInbox();
      await screen.findByText(/Nothing waiting/);

      await act(async () => {
        emitFromBackend(DISTILL_STATE_EVENT, { status: "distilling" });
      });
      expect(screen.getByTestId("pipeline-placeholder")).toBeInTheDocument();

      await act(async () => {
        emitFromBackend(DISTILL_STATE_EVENT, {
          status: "skipped",
          reason: "no speech",
          session_path: "s3.jsonl",
        });
      });
      expect(screen.queryByTestId("pipeline-placeholder")).not.toBeInTheDocument();
      expect(await screen.findByText(/Nothing waiting/)).toBeInTheDocument();
    });

    it("gives up on a stalled distill after a grace period", async () => {
      // The scenario in a dev/mock build: transcription saves but nothing
      // ever follows it with a distill run.
      serveVault([]);
      renderInbox();
      await screen.findByText(/Nothing waiting/);

      await act(async () => {
        emitFromBackend(TRANSCRIPTION_STATE_EVENT, { status: "saved", path: "t.jsonl" });
      });
      expect(screen.getByTestId("pipeline-placeholder")).toBeInTheDocument();

      await act(async () => {
        vi.advanceTimersByTime(10_000);
      });
      expect(screen.queryByTestId("pipeline-placeholder")).not.toBeInTheDocument();
    });

    it("hides the moment a new capture starts", async () => {
      serveVault([]);
      renderInbox();
      await screen.findByText(/Nothing waiting/);

      await act(async () => {
        emitFromBackend(DISTILL_STATE_EVENT, { status: "distilling" });
      });
      expect(screen.getByTestId("pipeline-placeholder")).toBeInTheDocument();

      await act(async () => {
        emitFromBackend(CAPTURE_STATE_EVENT, LISTENING);
      });
      expect(screen.queryByTestId("pipeline-placeholder")).not.toBeInTheDocument();
    });

    it("clears a still-open toast the moment a new capture starts", async () => {
      serveVault([]);
      renderInbox();
      await screen.findByText(/Nothing waiting/);

      await act(async () => {
        emitFromBackend(DISTILL_STATE_EVENT, {
          status: "saved",
          path: "/kb/briarwood-golf/meeting.md",
          session_path: "s9.jsonl",
        });
      });
      expect(screen.getByRole("button", { name: /Open the note filed to/ })).toBeInTheDocument();

      await act(async () => {
        emitFromBackend(CAPTURE_STATE_EVENT, LISTENING);
      });
      await waitForRemoval(() =>
        screen.queryByRole("button", { name: /Open the note filed to/ }),
      );
    });

    it("does not replay the vanish or the filed toast when the Inbox remounts", async () => {
      serveVault([]);
      onCommand("capture_phase", () => IDLE);
      const { rerender } = render(
        <NavigationProvider>
          <CapturePipelineProvider>
            <InboxView />
          </CapturePipelineProvider>
        </NavigationProvider>,
      );
      await screen.findByText(/Nothing waiting/);

      await act(async () => {
        emitFromBackend(DISTILL_STATE_EVENT, {
          status: "saved",
          path: "/kb/briarwood-golf/meeting.md",
          session_path: "s10.jsonl",
        });
      });
      // The toast is up from the moment the outcome lands — it overlaps the
      // vanish rather than following it.
      expect(screen.getByRole("button", { name: /Open the note filed to/ })).toBeInTheDocument();
      await act(async () => {
        vi.advanceTimersByTime(280); // VANISH_MS — the placeholder is retired
      });
      await act(async () => {
        vi.advanceTimersByTime(3500); // TOAST_DWELL_MS — seen out in full
      });
      await waitForRemoval(() =>
        screen.queryByRole("button", { name: /Open the note filed to/ }),
      );

      // Navigate away and back. The provider — and the distill hook's
      // terminal `saved`, which persists until the next capture — survive
      // the view unmounting, so without a shell-held record of the outcome
      // having been presented, this would replay the whole gesture.
      rerender(
        <NavigationProvider>
          <CapturePipelineProvider>
            <div />
          </CapturePipelineProvider>
        </NavigationProvider>,
      );
      rerender(
        <NavigationProvider>
          <CapturePipelineProvider>
            <InboxView />
          </CapturePipelineProvider>
        </NavigationProvider>,
      );
      await screen.findByText(/Nothing waiting/);
      expect(screen.queryByTestId("pipeline-placeholder")).not.toBeInTheDocument();
      await act(async () => {
        vi.advanceTimersByTime(500);
      });
      expect(
        screen.queryByRole("button", { name: /Open the note filed to/ }),
      ).not.toBeInTheDocument();
    });

    it("stays gone when the freshly landed note is filed away again", async () => {
      serveVault([PLANNING]);
      renderInbox();
      await screen.findByText("Quarterly planning");

      await act(async () => {
        emitFromBackend(DISTILL_STATE_EVENT, {
          status: "saved",
          path: "C:\\kb\\Inbox\\n_g7h8i9.md",
          session_path: "s11.jsonl",
        });
      });
      const NEW_NOTE = makeNote({ id: "n_g7h8i9", title: "New capture" });
      serveVault([NEW_NOTE, PLANNING]);
      act(() => {
        notifyVaultChanged();
      });
      // Both conditions, and both matter. The note LANDING is what arms the
      // fill-in timer below, and a wait for something to be absent would be
      // satisfied by it never having arrived; the placeholder GOING is the
      // handoff itself, and it settles a beat later if a refetch already in
      // flight answers with the older list first.
      await screen.findByText("New capture");
      await waitFor(() => {
        expect(screen.queryByTestId("pipeline-placeholder")).not.toBeInTheDocument();
      });
      await act(async () => {
        vi.advanceTimersByTime(200); // FILL_IN_MS — the outcome is now handled
      });

      // The user files the fresh note somewhere else: it leaves the list
      // while the distill hook still reports the run as saved-to-Inbox. The
      // placeholder must not come back as a phantom "distilling" row.
      serveVault([PLANNING]);
      act(() => {
        notifyVaultChanged();
      });
      await waitFor(() => {
        expect(screen.queryByText("New capture")).not.toBeInTheDocument();
      });
      expect(screen.queryByTestId("pipeline-placeholder")).not.toBeInTheDocument();
    });

    it("shows the current stage when the Inbox mounts mid-pipeline", async () => {
      serveVault([]);
      onCommand("capture_phase", () => IDLE);
      const { rerender } = render(
        <NavigationProvider>
          <CapturePipelineProvider>
            <div />
          </CapturePipelineProvider>
        </NavigationProvider>,
      );
      await act(async () => {
        await Promise.resolve();
      });

      // The pipeline started while some other screen was mounted — the Inbox
      // was not there to catch the event with its own listener.
      await act(async () => {
        emitFromBackend(DISTILL_STATE_EVENT, { status: "distilling" });
      });

      rerender(
        <NavigationProvider>
          <CapturePipelineProvider>
            <InboxView />
          </CapturePipelineProvider>
        </NavigationProvider>,
      );

      await screen.findByTestId("pipeline-placeholder");
      expect(activeStatusText()).toBe("Distilling the meeting");
    });
  });
});
