import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { NoteEditorView } from "./NoteEditorView";
import { NavigationProvider } from "../providers/NavigationProvider";
import type { NoteDetail } from "../../useNotes";
import type { SessionArtifacts, TranscriptSegment } from "../../useSessions";
import {
  convertFileSrc,
  invoke,
  invokedCommands,
  onCommand,
  resetTauriMocks,
} from "../../test/tauri";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

const SOURCE = "sessions/20260712T140335123Z-k4m2xp7q-budget-sync.jsonl";
const AUDIO_PATH =
  "C:\\kb\\sessions\\20260712T140335123Z-k4m2xp7q-budget-sync.wav";

const SEGMENTS: TranscriptSegment[] = [
  {
    index: 0,
    channel: "you",
    speaker: null,
    start_ms: 0,
    end_ms: 1500,
    text: "Morning, ready to start?",
  },
  {
    index: 1,
    channel: "them",
    speaker: null,
    start_ms: 1500,
    end_ms: 4000,
    text: "Yes, the budget first.",
  },
  {
    index: 2,
    channel: "unknown",
    speaker: null,
    start_ms: 65_000,
    end_ms: 66_000,
    text: "Could we circle back?",
  },
];

function serveArtifacts(overrides: Partial<SessionArtifacts> = {}): void {
  onCommand("read_session_artifacts", () => ({
    transcript_available: true,
    segments: SEGMENTS,
    audio_path: AUDIO_PATH,
    ...overrides,
  }));
}

function serveNote(overrides: Partial<NoteDetail> = {}): void {
  onCommand("read_note", () => ({
    id: "n_a1b2c3",
    path: "Inbox/budget-sync.md",
    title: "Budget sync",
    type: "meeting",
    project: null,
    date: "2026-07-12T14:03:35Z",
    tags: [],
    source: SOURCE,
    confidence: 0.41,
    snippet: "",
    body_markdown: "The summary.",
    ...overrides,
  }));
}

/** The panel only ever renders inside a note, and its transcript chip toggles
 * something the VIEW owns — so the view is the unit under test. Rendering the
 * panel bare would have to fake the one wire the redesign added. */
function renderNote() {
  return render(
    <NavigationProvider>
      <NoteEditorView noteId="n_a1b2c3" project="Inbox" />
    </NavigationProvider>,
  );
}

beforeEach(resetTauriMocks);

describe("SessionPanel", () => {
  it("stands each surviving artifact up as its own chip", async () => {
    // Both artifacts present is the maximal case. Each chip opens ONE thing and
    // neither nests inside the other (docs/UI_CONVENTIONS.md, *Composition*) —
    // which is what buys the second chip over the single disclosure this
    // replaced, now that they sit in a rail beside the note rather than as a
    // reader's fourth and fifth places to press below it.
    serveNote();
    serveArtifacts();
    renderNote();

    const transcript = await screen.findByTestId("session-source");
    const audio = screen.getByTestId("session-audio");
    expect(audio).toHaveTextContent("Audio");
    expect(transcript).toHaveTextContent("Transcript");
    expect(audio).toHaveAttribute("aria-expanded", "false");
    expect(transcript).toHaveAttribute("aria-expanded", "false");
    // Neither artifact is on screen until asked for. The player is the
    // exception, and deliberately so — see the mounted-at-rest test below.
    expect(screen.queryByTestId("source-panel")).toBeNull();
    expect(screen.queryByRole("button", { name: "Reveal in Explorer" })).toBeNull();
  });

  it("counts the transcript in words, which is what a reader is deciding about", async () => {
    // Segments were the old count, and they are an artifact of how the
    // recognizer chunked the audio: "3 segments" says nothing about how long
    // this will take to read.
    serveNote();
    serveArtifacts({ audio_path: null });
    renderNote();

    expect(await screen.findByTestId("session-source")).toHaveTextContent("12 words");
  });

  it("counts a lone word in the singular", async () => {
    serveNote();
    serveArtifacts({
      segments: [{ ...SEGMENTS[0], text: "Morning" }],
      audio_path: null,
    });
    renderNote();

    expect(await screen.findByTestId("session-source")).toHaveTextContent("1 word");
  });

  it("reads the recording's length off the last turn until the player knows better", async () => {
    // jsdom's <audio> never loads metadata, so this is the state a real player
    // is in for its first moments too: the last turn's end offset is the
    // recording's length to within a segment, and it arrives with the
    // transcript rather than after a round trip.
    serveNote();
    serveArtifacts();
    renderNote();

    expect(await screen.findByTestId("session-audio")).toHaveTextContent("1:06");
  });

  it("summons the exchange in the reading column, not in the rail", async () => {
    serveNote();
    serveArtifacts({ audio_path: null });
    renderNote();

    const transcript = await screen.findByTestId("session-source");
    await userEvent.click(transcript);

    expect(transcript).toHaveAttribute("aria-expanded", "true");
    // Every channel renders with its label and its text.
    expect(screen.getByText("You")).toBeInTheDocument();
    expect(screen.getByText("Them")).toBeInTheDocument();
    expect(screen.getByText("Unknown")).toBeInTheDocument();
    expect(screen.getByText("Morning, ready to start?")).toBeInTheDocument();
    expect(screen.getByText("Yes, the budget first.")).toBeInTheDocument();
    // Offsets read m:ss from session start.
    expect(screen.getByText("0:00")).toBeInTheDocument();
    expect(screen.getByText("1:05")).toBeInTheDocument();
    expect(screen.getAllByTestId("session-turn")).toHaveLength(3);
  });

  it("keeps the player mounted at rest, so listening survives closing the chip", async () => {
    // The old section unmounted the <audio> on collapse, which stopped playback
    // the moment you went back to reading. Hidden rather than absent costs one
    // ranged metadata read and buys both listening-while-reading and the
    // duration on the chip.
    serveNote();
    serveArtifacts();
    renderNote();

    const audio = await screen.findByTestId("session-audio");
    const player = screen.getByLabelText("Meeting recording");
    expect(player).toHaveAttribute("src", convertFileSrc(AUDIO_PATH));
    expect(player).not.toBeVisible();

    await userEvent.click(audio);
    expect(player).toBeVisible();

    await userEvent.click(audio);
    expect(player).not.toBeVisible();
    // Still there, still the same element: closing hid it, it did not stop it.
    expect(screen.getByLabelText("Meeting recording")).toBe(player);
  });

  it("offers reveal beside the recording it acts on, once open", async () => {
    serveNote();
    serveArtifacts();
    onCommand("reveal_session_audio", () => undefined);
    renderNote();

    await userEvent.click(await screen.findByTestId("session-audio"));
    // The slot follows the subject, not the available space: reveal acts on the
    // .wav, so it lives with the .wav rather than up in the note's title row.
    await userEvent.click(screen.getByTestId("reveal-recording"));

    expect(invoke).toHaveBeenCalledWith("reveal_session_audio", {
      audioPath: AUDIO_PATH,
    });
  });

  it("shows no audio chip when the recording was not retained", async () => {
    serveNote();
    serveArtifacts({ audio_path: null });
    renderNote();

    await screen.findByTestId("session-source");
    expect(screen.queryByTestId("session-audio")).toBeNull();
    expect(screen.queryByLabelText("Meeting recording")).toBeNull();
    expect(screen.queryByTestId("reveal-recording")).toBeNull();
  });

  it("states a pruned transcript at rest, rather than behind a chip", async () => {
    // "Checked against the source" must never fail silently, so the sentence
    // stays visible while the artifact it describes is gone.
    serveNote();
    serveArtifacts({ transcript_available: false, segments: [] });
    renderNote();

    expect(
      await screen.findByTestId("session-source-pruned"),
    ).toHaveTextContent("The raw transcript for this note is no longer stored.");
    // Nothing to open, so nothing offers to.
    expect(screen.queryByTestId("session-source")).toBeNull();
    // The recording resolves independently of the missing transcript — and with
    // no last turn to read, the chip carries no length.
    expect(screen.getByTestId("session-audio")).toHaveTextContent("Audio");
  });

  it("offers nothing to press when neither artifact survived", async () => {
    serveNote();
    serveArtifacts({
      transcript_available: false,
      segments: [],
      audio_path: null,
    });
    renderNote();

    await screen.findByTestId("session-source-pruned");
    // A chip over an empty panel is worse than no chip. Only the note's own
    // three controls remain.
    expect(screen.queryByTestId("session-source")).toBeNull();
    expect(screen.queryByTestId("session-audio")).toBeNull();
    expect(screen.getAllByRole("button")).toHaveLength(3);
  });

  it("surfaces a failed reveal beside the control", async () => {
    serveNote();
    serveArtifacts();
    onCommand("reveal_session_audio", () => {
      throw "The recording file is missing.";
    });
    renderNote();

    await userEvent.click(await screen.findByTestId("session-audio"));
    await userEvent.click(screen.getByTestId("reveal-recording"));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The recording file is missing.",
    );
  });

  it("does not carry a failed reveal across closing the chip", async () => {
    // The error belongs to the press, not to the chip's state. It lives on the
    // panel now that the panel outlives the player, so without a reset it comes
    // back on reopening — a `role="alert"` announcing a failure the reader did
    // not just cause.
    serveNote();
    serveArtifacts();
    onCommand("reveal_session_audio", () => {
      throw "The recording file is missing.";
    });
    renderNote();

    const audio = await screen.findByTestId("session-audio");
    await userEvent.click(audio);
    await userEvent.click(screen.getByTestId("reveal-recording"));
    await screen.findByRole("alert");

    await userEvent.click(audio);
    await userEvent.click(audio);

    expect(screen.getByTestId("reveal-recording")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("points the transcript chip at the turns it summons", async () => {
    // The one disclosure whose content is upstream of its trigger: the turns
    // land in the body column, which precedes the rail. `aria-expanded` alone
    // would leave nothing after the button and nothing to jump to.
    serveNote();
    serveArtifacts({ audio_path: null });
    renderNote();

    const transcript = await screen.findByTestId("session-source");
    await userEvent.click(transcript);

    const controls = transcript.getAttribute("aria-controls");
    expect(controls).toBeTruthy();
    expect(screen.getByTestId("source-panel")).toHaveAttribute("id", controls);
  });

  it("surfaces a failed artifact read as an error, without the exception behind it", async () => {
    // The leak pin for this surface. `SessionPanel` renders its error as the
    // ENTIRE message, with no sentence of its own around it, so whatever
    // arrives here is what the reader sees — which is why the hook supplies a
    // fixed sentence and discards the rejection (docs/DESIGN_SYSTEM.md §3:
    // "never leaks an exception string").
    //
    // The thrown value is shaped like the developer-facing string a wrapper
    // would produce if someone reverted the translation in `user_errors.rs`:
    // an absolute path and an OS error. Asserting on its ABSENCE (rather than
    // on the sentence alone) is what makes this a regression test.
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    serveNote();
    onCommand("read_session_artifacts", () => {
      throw "note I/O failed at C:\\Users\\someone\\kb\\sessions\\a.jsonl: Access is denied. (os error 5)";
    });
    renderNote();

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(
      "Couldn't read this note's session files. The note itself is fine; reopen it to try again.",
    );
    expect(screen.queryByText(/os error|C:\\|\.jsonl/)).toBeNull();
    // Discarded from the screen, not from the record: a hook with a fixed
    // sentence is the only place a rejection would otherwise vanish entirely,
    // and the console is all there is to catch it.
    expect(logged).toHaveBeenCalled();
    logged.mockRestore();
    // The panel exists even when the fetch failed: the testid means "this note
    // has a session", which is the claim a keyword-sourced note has to fail.
    expect(screen.getByTestId("session-artifacts")).toBeInTheDocument();
  });
});

describe("NoteEditorView source pairing", () => {
  it("holds the composition ceiling on the app's heaviest note", async () => {
    // A distilled session note whose audio AND transcript both survived
    // retention is the maximal reading surface in the app. This is the lock on
    // the number: the way out, the title row's one cluster, and one chip per
    // artifact — five controls at rest, and anything that adds a sixth fails
    // here first. Against docs/UI_CONVENTIONS.md, *Composition*: the back link
    // sits outside the count (getting around is not acting), Edit and Delete
    // are the view-owned header's single cluster, and each chip is one
    // disclosure over one subordinate section that does not nest.
    serveNote();
    serveArtifacts();
    renderNote();

    await screen.findByTestId("session-source");
    const controls = screen.getAllByRole("button");
    expect(controls).toHaveLength(5);
    // BackLink's arrow is aria-hidden, so its name is the destination alone.
    expect(controls[0]).toHaveAccessibleName("Inbox");
    expect(controls[1]).toHaveAccessibleName("Edit");
    expect(controls[2]).toHaveAccessibleName("Delete note");
    expect(controls[3]).toHaveTextContent("Audio");
    expect(controls[4]).toHaveTextContent("Transcript");
  });

  it("never fetches artifacts for a keyword-sourced note", async () => {
    serveNote({ source: "manual" });
    renderNote();

    await screen.findByText("The summary.");
    await waitFor(() => {
      expect(invokedCommands()).not.toContain("read_session_artifacts");
    });
    expect(screen.queryByTestId("session-artifacts")).toBeNull();
    expect(screen.queryByTestId("session-source")).toBeNull();
  });
});
