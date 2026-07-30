import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SessionArtifactsSection } from "./SessionArtifactsSection";
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

beforeEach(resetTauriMocks);

describe("SessionArtifactsSection", () => {
  it("shows one control at rest, whatever survived retention", async () => {
    // The whole point of the section's shape: a reader arriving at a note meets
    // ONE place to press below it, not a row per artifact
    // (docs/UI_CONVENTIONS.md, *Composition*). Both artifacts present is the
    // maximal case, so it is the one that has to hold.
    serveArtifacts();
    render(<SessionArtifactsSection source={SOURCE} />);

    const toggle = await screen.findByTestId("session-source");
    expect(screen.getAllByRole("button")).toHaveLength(1);
    expect(toggle).toHaveTextContent("Source · recording · 3 segments");
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    // Neither artifact is on screen until asked for.
    expect(screen.queryByLabelText("Meeting recording")).toBeNull();
    expect(
      screen.queryByRole("button", { name: "Reveal in Explorer" }),
    ).toBeNull();
    expect(screen.queryByText("Morning, ready to start?")).toBeNull();
  });

  it("summons the exchange, and states what it holds in one line", async () => {
    serveArtifacts({ audio_path: null });
    render(<SessionArtifactsSection source={SOURCE} />);

    const toggle = await screen.findByRole("button", { name: /Source/ });
    // With no recording the line says so, which is the only signal at rest for
    // what opening this will show.
    expect(toggle).toHaveTextContent("Source · 3 segments");

    await userEvent.click(toggle);

    expect(toggle).toHaveAttribute("aria-expanded", "true");
    // Every channel renders with its label and its text.
    expect(screen.getByText("You")).toBeInTheDocument();
    expect(screen.getByText("Them")).toBeInTheDocument();
    expect(screen.getByText("Unknown")).toBeInTheDocument();
    expect(screen.getByText("Morning, ready to start?")).toBeInTheDocument();
    expect(screen.getByText("Yes, the budget first.")).toBeInTheDocument();
    // Offsets read m:ss from session start.
    expect(screen.getByText("0:00")).toBeInTheDocument();
    expect(screen.getByText("1:05")).toBeInTheDocument();
  });

  it("counts a lone segment in the singular", async () => {
    serveArtifacts({ segments: [SEGMENTS[0]], audio_path: null });
    render(<SessionArtifactsSection source={SOURCE} />);

    expect(await screen.findByTestId("session-source")).toHaveTextContent(
      "Source · 1 segment",
    );
  });

  it("offers the retained recording for playback and reveal, once open", async () => {
    serveArtifacts();
    onCommand("reveal_session_audio", () => undefined);
    render(<SessionArtifactsSection source={SOURCE} />);

    await userEvent.click(await screen.findByTestId("session-source"));

    const player = screen.getByLabelText("Meeting recording");
    expect(player).toHaveAttribute("src", convertFileSrc(AUDIO_PATH));

    // Reveal sits beside the recording it acts on, not up in the note's title
    // row: the slot follows the subject, not the available space.
    await userEvent.click(screen.getByTestId("reveal-recording"));

    expect(invoke).toHaveBeenCalledWith("reveal_session_audio", {
      audioPath: AUDIO_PATH,
    });
  });

  it("shows no recording row when the audio was not retained", async () => {
    serveArtifacts({ audio_path: null });
    render(<SessionArtifactsSection source={SOURCE} />);

    await userEvent.click(await screen.findByTestId("session-source"));

    expect(screen.queryByLabelText("Meeting recording")).toBeNull();
    expect(screen.queryByTestId("reveal-recording")).toBeNull();
    // ...and the transcript still opened.
    expect(screen.getByText("Morning, ready to start?")).toBeInTheDocument();
  });

  it("states a pruned transcript at rest, rather than behind the click", async () => {
    // "Checked against the source" must never fail silently, so the sentence
    // stays visible while the artifact it describes is gone.
    serveArtifacts({ transcript_available: false, segments: [] });
    render(<SessionArtifactsSection source={SOURCE} />);

    expect(
      await screen.findByText(
        "The raw transcript for this note is no longer stored.",
      ),
    ).toBeInTheDocument();
    // The recording resolves independently of the missing transcript, and the
    // toggle's line names only what is actually there.
    expect(screen.getByTestId("session-source")).toHaveTextContent(
      "Source · recording",
    );

    await userEvent.click(screen.getByTestId("session-source"));

    expect(screen.getByLabelText("Meeting recording")).toBeInTheDocument();
  });

  it("offers nothing to press when neither artifact survived", async () => {
    serveArtifacts({
      transcript_available: false,
      segments: [],
      audio_path: null,
    });
    render(<SessionArtifactsSection source={SOURCE} />);

    expect(
      await screen.findByText(
        "The raw transcript for this note is no longer stored.",
      ),
    ).toBeInTheDocument();
    // A disclosure over an empty panel is worse than no disclosure.
    expect(screen.queryAllByRole("button")).toHaveLength(0);
  });

  it("surfaces a failed reveal beside the control", async () => {
    serveArtifacts();
    onCommand("reveal_session_audio", () => {
      throw "The recording file is missing.";
    });
    render(<SessionArtifactsSection source={SOURCE} />);

    await userEvent.click(await screen.findByTestId("session-source"));
    await userEvent.click(screen.getByTestId("reveal-recording"));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The recording file is missing.",
    );
  });

  it("surfaces a failed artifact read as an error", async () => {
    onCommand("read_session_artifacts", () => {
      throw "not a session source: sessions/nope.jsonl";
    });
    render(<SessionArtifactsSection source={SOURCE} />);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "not a session source",
    );
  });
});

describe("NoteEditorView source pairing", () => {
  function serveNote(overrides: Partial<NoteDetail>): void {
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

  function renderNote() {
    return render(
      <NavigationProvider>
        <NoteEditorView noteId="n_a1b2c3" project="Inbox" />
      </NavigationProvider>,
    );
  }

  it("holds the composition ceiling on the app's heaviest note", async () => {
    // A distilled session note whose audio AND transcript both survived
    // retention is the maximal reading surface in the app, and it used to put
    // five controls in four places. This is the lock on the number: three
    // places (the way out, the note's own title row, the source below it) and
    // four controls at rest. Anything that adds a fifth fails here first.
    serveNote({});
    serveArtifacts();
    renderNote();

    await screen.findByTestId("session-source");
    const controls = screen.getAllByRole("button");
    expect(controls).toHaveLength(4);
    // BackLink's arrow is aria-hidden, so its name is the destination alone.
    expect(controls[0]).toHaveAccessibleName("Inbox");
    expect(controls[1]).toHaveAccessibleName("Edit");
    expect(controls[2]).toHaveAccessibleName("Delete note");
    // Read by text, not by accessible name: the faint detail is its own inline
    // <span>, and the name algorithm trims each node before joining, so the
    // computed name loses the space before the first "·". The Needs Attention
    // shelf toggle ("Dismissed · N") has the identical shape, so this is the
    // house pattern rather than this control's problem.
    expect(controls[3]).toHaveTextContent("Source · recording · 3 segments");
  });

  it("never fetches artifacts for a keyword-sourced note", async () => {
    serveNote({ source: "manual" });
    renderNote();

    await screen.findByText("The summary.");
    await waitFor(() => {
      expect(invokedCommands()).not.toContain("read_session_artifacts");
    });
    expect(screen.queryByTestId("session-source")).toBeNull();
  });
});
