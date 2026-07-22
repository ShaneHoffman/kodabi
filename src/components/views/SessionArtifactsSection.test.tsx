import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SessionArtifactsSection } from "./SessionArtifactsSection";
import { NoteEditorView } from "./NoteEditorView";
import { NavigationProvider } from "../NavigationProvider";
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
  it("renders the transcript collapsed, and expands to the exchange", async () => {
    serveArtifacts({ audio_path: null });
    render(<SessionArtifactsSection source={SOURCE} />);

    const toggle = await screen.findByRole("button", { name: /Transcript/ });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(toggle).toHaveTextContent("3 segments");
    expect(screen.queryByText("Morning, ready to start?")).toBeNull();

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

  it("offers the retained recording for playback and reveal", async () => {
    serveArtifacts();
    onCommand("reveal_session_audio", () => undefined);
    render(<SessionArtifactsSection source={SOURCE} />);

    const player = await screen.findByLabelText("Meeting recording");
    expect(player).toHaveAttribute("src", convertFileSrc(AUDIO_PATH));

    await userEvent.click(
      screen.getByRole("button", { name: "Reveal in Explorer" }),
    );

    expect(invoke).toHaveBeenCalledWith("reveal_session_audio", {
      audioPath: AUDIO_PATH,
    });
  });

  it("shows no recording row when the audio was not retained", async () => {
    serveArtifacts({ audio_path: null });
    render(<SessionArtifactsSection source={SOURCE} />);

    await screen.findByRole("button", { name: /Transcript/ });
    expect(screen.queryByLabelText("Meeting recording")).toBeNull();
    expect(screen.queryByRole("button", { name: "Reveal in Explorer" })).toBeNull();
  });

  it("states a pruned transcript instead of hiding the section", async () => {
    serveArtifacts({ transcript_available: false, segments: [] });
    render(<SessionArtifactsSection source={SOURCE} />);

    expect(
      await screen.findByText(
        "The raw transcript for this note is no longer stored.",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Transcript/ })).toBeNull();
    // The recording resolves independently of the missing transcript.
    expect(screen.getByLabelText("Meeting recording")).toBeInTheDocument();
  });

  it("surfaces a failed reveal beside the control", async () => {
    serveArtifacts();
    onCommand("reveal_session_audio", () => {
      throw "The recording file is missing.";
    });
    render(<SessionArtifactsSection source={SOURCE} />);

    await userEvent.click(
      await screen.findByRole("button", { name: "Reveal in Explorer" }),
    );

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

  it("mounts the pairing section for a session-sourced note", async () => {
    serveNote({});
    serveArtifacts();
    renderNote();

    expect(
      await screen.findByRole("button", { name: /Transcript/ }),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("Meeting recording")).toBeInTheDocument();
  });

  it("never fetches artifacts for a keyword-sourced note", async () => {
    serveNote({ source: "manual" });
    renderNote();

    await screen.findByText("The summary.");
    await waitFor(() => {
      expect(invokedCommands()).not.toContain("read_session_artifacts");
    });
    expect(screen.queryByRole("button", { name: /Transcript/ })).toBeNull();
  });
});
