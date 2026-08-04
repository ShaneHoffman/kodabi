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
    snippet: "",
    ...overrides,
  };
}

function makeProject(slug: string): Project {
  return {
    id: slug,
    slug,
    display_name: slug,
    parent: null,
    note_count: 0,
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

/** Matches a row's picker by its accessible name, which is the sr-only label
 * followed by the trigger's own text ("… File to…"). A predicate, not a
 * `RegExp`: the note title is interpolated, and a title carrying `(` or `?`
 * would make a pattern that throws or quietly matches the wrong row. */
function pickerFor(title: string) {
  const label = `File "${title}" to project`;
  return (accessibleName: string) => accessibleName.startsWith(label);
}

/** Open a row's project picker and choose `project`. */
async function fileNote(
  user: ReturnType<typeof userEvent.setup>,
  title: string,
  project: string,
): Promise<void> {
  await user.click(screen.getByRole("combobox", { name: pickerFor(title) }));
  await user.click(screen.getByRole("option", { name: project }));
}

/** The status span currently reading `is-visible` inside the placeholder —
 * the crossfade renders both texts of a pair always, so a plain substring
 * match can't tell which one is actually showing. */
function activeStatusText(): string | null {
  const placeholder = screen.getByTestId("pipeline-placeholder");
  return placeholder.querySelector(".is-visible")?.textContent ?? null;
}

/** What the placeholder actually announces: the sr-only line inside its
 * `role="status"` region. The visual crossfade is aria-hidden and always
 * carries both stage texts, so only this line's rewrite is an announcement. */
function announcedStatusText(): string | null {
  const placeholder = screen.getByTestId("pipeline-placeholder");
  return placeholder.querySelector(".sr-only")?.textContent ?? null;
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

    expect(
      await screen.findByText("2026-07-10 · chat · 62% match"),
    ).toBeInTheDocument();
  });

  it("says nothing about kind on a plain note's row", async () => {
    serveVault([PLANNING]);

    renderInbox();

    // The majority case: a `note` segment on every row would be noise.
    expect(await screen.findByText("2026-07-01 · 41% match")).toBeInTheDocument();
  });

  it("opens the note when the card body is clicked", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
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
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
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
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
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
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
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
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    serveVault([PLANNING]);
    onCommand("file_note_to_project", () => {
      throw "no such project: briarwood-golf";
    });
    renderInbox();
    await screen.findByText("Quarterly planning");

    await fileNote(user, "Quarterly planning", "briarwood-golf");

    // Substring, not exact: the message is rendered behind a prefix naming what
    // failed, so the raw backend string is never the whole line
    // (docs/DESIGN_SYSTEM.md §3).
    expect(await screen.findByText(/no such project: briarwood-golf/)).toBeInTheDocument();
    // The note is still unfiled, so it must still be actionable: row present
    // and the picker back (not stuck on "Filing…").
    expect(screen.getByText("Quarterly planning")).toBeInTheDocument();
    expect(
      screen.getByRole("combobox", { name: pickerFor("Quarterly planning") }),
    ).toBeInTheDocument();
  });

  it("offers no picker when there is no project to file into", async () => {
    serveVault([PLANNING], []);

    renderInbox();

    // No per-row hint in its place either: pointing at the fix is the
    // sidebar's job (task #84's create-project affordance), not one copy of
    // the same sentence on every card.
    await screen.findByText("Quarterly planning");
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(screen.queryByText(/Create a project/)).not.toBeInTheDocument();
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

    // The count sits on the title's baseline, so the noun lives in the view's
    // name rather than being repeated in the count.
    expect(await screen.findByText("2 to file")).toBeInTheDocument();
    unmount();

    resetTauriMocks();
    serveVault([PLANNING]);
    renderInbox();

    expect(await screen.findByText("1 to file")).toBeInTheDocument();
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

  it("surfaces a failed listing instead of claiming an empty inbox", async () => {
    onCommand("list_notes", () => {
      throw "the vault is unreadable";
    });
    onCommand("list_projects", () => ({ inbox_note_count: 0, projects: [] }));

    renderInbox();

    expect(await screen.findByText(/the vault is unreadable/)).toBeInTheDocument();
    expect(screen.queryByText(/Nothing waiting/)).not.toBeInTheDocument();
  });

  describe("the delete affordance", () => {
    it("offers delete even when there is no project to file into", async () => {
      serveVault([PLANNING], []); // a project-less vault: nowhere to file
      renderInbox();
      await screen.findByText("Quarterly planning");

      // No picker (nothing to file into), but an unwanted note must still be
      // removable wherever it sits, so delete is always offered.
      expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
      expect(
        screen.getByRole("button", { name: 'Delete "Quarterly planning"' }),
      ).toBeInTheDocument();
    });

    it("cancels without deleting", async () => {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
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
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
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
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
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
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      serveVault([PLANNING]);
      onCommand("delete_note", () => {
        throw "the vault is read-only";
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
          "Couldn't delete the note: the vault is read-only",
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
        emitFromBackend(TRANSCRIPTION_STATE_EVENT, { status: "transcribing" });
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
        emitFromBackend(TRANSCRIPTION_STATE_EVENT, { status: "transcribing" });
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
        emitFromBackend(TRANSCRIPTION_STATE_EVENT, { status: "transcribing" });
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
        emitFromBackend(TRANSCRIPTION_STATE_EVENT, { status: "transcribing" });
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
      expect(rows[0].closest(".inbox__rowShell")).toHaveClass("inbox__rowShell--fresh");
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
      // No dwell on the row itself any more: it starts leaving at once.
      expect(screen.getByTestId("pipeline-placeholder").className).toContain(
        "inbox__slot--leaving",
      );
      expect(screen.queryByRole("button", { name: /Open the note filed to/ })).not.toBeInTheDocument();

      await act(async () => {
        vi.advanceTimersByTime(200); // VANISH_MS
      });
      expect(screen.queryByTestId("pipeline-placeholder")).not.toBeInTheDocument();
      const toast = screen.getByRole("button", {
        name: 'Open the note filed to "briarwood-golf"',
      });
      expect(toast).toHaveTextContent("Filed to");
      expect(toast).toHaveTextContent("briarwood-golf");
      expect(screen.getByRole("status")).toHaveTextContent("briarwood-golf");

      await act(async () => {
        vi.advanceTimersByTime(3500); // TOAST_DWELL_MS
      });
      expect(screen.getByRole("button", { name: /Open the note filed to/ })).toHaveClass(
        "inbox__toast--fading",
      );

      await act(async () => {
        vi.advanceTimersByTime(130); // TOAST_FADE_MS
      });
      expect(screen.queryByRole("button", { name: /Open the note filed to/ })).not.toBeInTheDocument();
    });

    it("pauses the toast's dwell while hovered, and resumes it on unhover", async () => {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
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
      await act(async () => {
        vi.advanceTimersByTime(200);
      });
      const toast = screen.getByRole("button", { name: /Open the note filed to/ });

      await user.hover(toast);
      await act(async () => {
        vi.advanceTimersByTime(5000); // well past TOAST_DWELL_MS
      });
      expect(screen.getByRole("button", { name: /Open the note filed to/ })).not.toHaveClass(
        "inbox__toast--fading",
      );

      await user.unhover(toast);
      await act(async () => {
        vi.advanceTimersByTime(3500); // TOAST_DWELL_MS, restarted
      });
      expect(screen.getByRole("button", { name: /Open the note filed to/ })).toHaveClass(
        "inbox__toast--fading",
      );
    });

    it("opens the filed note when the toast is clicked", async () => {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
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
      await act(async () => {
        vi.advanceTimersByTime(200);
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
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
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
      await act(async () => {
        vi.advanceTimersByTime(200);
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
      await act(async () => {
        vi.advanceTimersByTime(200);
      });
      expect(screen.getByRole("button", { name: /Open the note filed to/ })).toBeInTheDocument();

      await act(async () => {
        emitFromBackend(CAPTURE_STATE_EVENT, LISTENING);
      });
      expect(screen.queryByRole("button", { name: /Open the note filed to/ })).not.toBeInTheDocument();
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
      await act(async () => {
        vi.advanceTimersByTime(200); // VANISH_MS — the toast takes over
      });
      expect(screen.getByRole("button", { name: /Open the note filed to/ })).toBeInTheDocument();
      await act(async () => {
        vi.advanceTimersByTime(3500); // TOAST_DWELL_MS
      });
      await act(async () => {
        vi.advanceTimersByTime(130); // TOAST_FADE_MS — seen out in full
      });
      expect(
        screen.queryByRole("button", { name: /Open the note filed to/ }),
      ).not.toBeInTheDocument();

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
