import { describe, expect, it } from "vitest";
import {
  pipelineStage,
  savedDestination,
  savedPathMatchesNote,
  transcribeBeat,
  type CapturePipeline,
} from "./useCapturePipeline";

const IDLE_CAPTURE = { phase: "idle" as const, sources: { loopback: "off" as const, microphone: "off" as const } };
const LISTENING_CAPTURE = {
  phase: "listening" as const,
  sources: { loopback: "live" as const, microphone: "live" as const },
};

function pipeline(overrides: Partial<CapturePipeline>): CapturePipeline {
  return {
    capture: IDLE_CAPTURE,
    transcription: { status: "idle" },
    distill: { status: "idle" },
    handledFiledId: null,
    markFiledHandled: () => {},
    stopPending: false,
    markStopHandled: () => {},
    ...overrides,
  };
}

describe("savedPathMatchesNote", () => {
  it("matches an absolute, backslash-separated saved path against a KB-relative note path", () => {
    expect(savedPathMatchesNote("C:\\kb\\Inbox\\n_a1b2c3.md", "Inbox/n_a1b2c3.md")).toBe(true);
  });

  it("matches a forward-slash absolute path the same way", () => {
    expect(savedPathMatchesNote("/home/user/kb/Inbox/n_a1b2c3.md", "Inbox/n_a1b2c3.md")).toBe(true);
  });

  it("does not match a different note's path", () => {
    expect(savedPathMatchesNote("C:\\kb\\Inbox\\n_a1b2c3.md", "Inbox/n_d4e5f6.md")).toBe(false);
  });
});

describe("savedDestination", () => {
  it("recognizes a note filed to the Inbox", () => {
    expect(savedDestination("C:\\kb\\Inbox\\n_a1b2c3.md", ["briarwood-golf"])).toEqual({
      kind: "inbox",
    });
  });

  it("recognizes a note filed to a top-level project", () => {
    expect(
      savedDestination("C:\\kb\\briarwood-golf\\meeting.md", ["briarwood-golf", "kodabi"]),
    ).toEqual({ kind: "project", slug: "briarwood-golf" });
  });

  it("prefers the longest matching slug for a nested project", () => {
    // A flat "acme" and a nested "clients/acme" both match as a path suffix;
    // the nested one is the one whose directory actually holds the file.
    expect(
      savedDestination("C:\\kb\\clients\\acme\\meeting.md", ["acme", "clients/acme"]),
    ).toEqual({ kind: "project", slug: "clients/acme" });
  });

  it("does not let a nested project's slug claim its sibling's file", () => {
    expect(
      savedDestination("C:\\kb\\acme\\meeting.md", ["acme", "clients/acme"]),
    ).toEqual({ kind: "project", slug: "acme" });
  });

  it("falls back to the parent directory name when no known slug matches", () => {
    // A note routed to a brand-new project can save before that project's
    // next `list_projects` refetch lands.
    expect(savedDestination("C:\\kb\\brand-new\\meeting.md", [])).toEqual({
      kind: "project",
      slug: "brand-new",
    });
  });
});

describe("pipelineStage", () => {
  it("is null while a capture is active, regardless of stale terminal state", () => {
    expect(
      pipelineStage(
        pipeline({
          capture: LISTENING_CAPTURE,
          distill: { status: "saved", path: "n.md", session_path: "s.jsonl" },
        }),
      ),
    ).toBeNull();
  });

  it("is null at rest", () => {
    expect(pipelineStage(pipeline({}))).toBeNull();
  });

  it("reports transcribing", () => {
    expect(
      pipelineStage(
        pipeline({
          transcription: { status: "transcribing", seconds_processed: 0, total_seconds: 60 },
        }),
      ),
    ).toEqual({
      id: "transcribing",
      kind: "transcribing",
      awaitingTranscribe: false,
    });
  });

  it("reports a queued run as the same started stage, never as one to give up on", () => {
    // A run parked behind `TRANSCRIBE_LOCK` has acknowledged the stop, so the
    // grace timer (`awaitingTranscribe`) must not waive it — a long
    // predecessor can hold it there for minutes.
    expect(pipelineStage(pipeline({ transcription: { status: "queued" } }))).toEqual({
      id: "transcribing",
      kind: "transcribing",
      awaitingTranscribe: false,
    });
  });

  it("reports a synthetic transcribing stage the moment a stop is pending", () => {
    // The backend announces `Transcribing` only once its worker holds the
    // transcribe lock, so right after a stop every raw state still reads
    // idle — the witnessed stop alone carries the placeholder.
    expect(pipelineStage(pipeline({ stopPending: true }))).toEqual({
      id: "stopped",
      kind: "transcribing",
      awaitingTranscribe: true,
    });
  });

  it("stays null during an active capture even with a stop pending", () => {
    // The provider clears the flag the render after a capture starts; this
    // covers the one render in between.
    expect(
      pipelineStage(pipeline({ capture: LISTENING_CAPTURE, stopPending: true })),
    ).toBeNull();
  });

  it("lets terminal failures outrank a pending stop", () => {
    expect(
      pipelineStage(
        pipeline({ stopPending: true, transcription: { status: "error", message: "boom" } }),
      ),
    ).toBeNull();
    expect(
      pipelineStage(
        pipeline({
          stopPending: true,
          distill: { status: "skipped", reason: "no speech", session_path: "s.jsonl" },
        }),
      ),
    ).toBeNull();
    expect(
      pipelineStage(
        pipeline({
          stopPending: true,
          distill: { status: "error", message: "boom", session_path: "s.jsonl" },
        }),
      ),
    ).toBeNull();
  });

  it("lets a filed outcome outrank a pending stop", () => {
    // `stopPending` clears only when the next capture starts, so a finished
    // pipeline must never fall back to the synthetic stage.
    expect(
      pipelineStage(
        pipeline({
          stopPending: true,
          distill: { status: "saved", path: "n.md", session_path: "s.jsonl" },
        }),
      ),
    ).toEqual({ id: "filed:s.jsonl", kind: "filed", savedPath: "n.md" });
  });

  it("reports distilling as awaited once transcription saves, before distill starts", () => {
    expect(
      pipelineStage(pipeline({ transcription: { status: "saved", path: "t.jsonl" } })),
    ).toEqual({ id: "awaiting-distill", kind: "distilling", awaitingDistill: true });
  });

  it("reports distilling as underway once distill actually starts, with a different id", () => {
    expect(pipelineStage(pipeline({ distill: { status: "distilling" } }))).toEqual({
      id: "distilling",
      kind: "distilling",
      awaitingDistill: false,
    });
  });

  it("distill outranks transcription once it has something to say", () => {
    expect(
      pipelineStage(
        pipeline({
          transcription: { status: "saved", path: "t.jsonl" },
          distill: { status: "distilling" },
        }),
      ),
    ).toEqual({ id: "distilling", kind: "distilling", awaitingDistill: false });
  });

  it("reports a filed outcome keyed on the session path", () => {
    expect(
      pipelineStage(
        pipeline({ distill: { status: "saved", path: "n.md", session_path: "s.jsonl" } }),
      ),
    ).toEqual({ id: "filed:s.jsonl", kind: "filed", savedPath: "n.md" });
  });

  it("is null on a skipped or failed distill, and on a failed transcription", () => {
    expect(
      pipelineStage(
        pipeline({ distill: { status: "skipped", reason: "no speech", session_path: "s.jsonl" } }),
      ),
    ).toBeNull();
    expect(
      pipelineStage(pipeline({ distill: { status: "error", message: "boom", session_path: "s.jsonl" } })),
    ).toBeNull();
    expect(
      pipelineStage(pipeline({ transcription: { status: "error", message: "boom" } })),
    ).toBeNull();
  });
});

describe("transcribeBeat", () => {
  const STARTING = {
    copy: "just stopped · processing audio",
    figures: null,
    fraction: null,
  };

  it("says work is starting before the backend has said anything", () => {
    expect(transcribeBeat({ status: "idle" })).toEqual(STARTING);
  });

  it("says queued only for a run genuinely parked behind another", () => {
    expect(transcribeBeat({ status: "queued" })).toEqual({
      copy: "queued behind the previous capture",
      figures: null,
      fraction: null,
    });
  });

  it("reports position within the recording while the ASR pass runs", () => {
    // 750s of a 58:04 recording: the figures read against the meeting the
    // user remembers, not the two-channel work total the backend divides down.
    expect(transcribeBeat({ status: "transcribing", seconds_processed: 750, total_seconds: 3484 }))
      .toEqual({
        copy: "processing audio",
        figures: "12:30 of 58:04",
        fraction: 750 / 3484,
      });
  });

  it("names the cleanup stage rather than leaving a full bar looking stalled", () => {
    expect(
      transcribeBeat({ status: "transcribing", seconds_processed: 3484, total_seconds: 3484 }),
    ).toEqual({ copy: "cleaning up transcript", figures: null, fraction: 1 });
  });

  it("clamps a fraction that overshoots its total", () => {
    // The backend clamps too, but a bar wider than its track is the kind of
    // thing that must not depend on both sides agreeing.
    const beat = transcribeBeat({
      status: "transcribing",
      seconds_processed: 3600,
      total_seconds: 3484,
    });
    expect(beat.fraction).toBe(1);
  });

  it("falls back to words rather than dividing by a zero or absent total", () => {
    expect(
      transcribeBeat({ status: "transcribing", seconds_processed: 0, total_seconds: 0 }),
    ).toEqual(STARTING);
    expect(
      transcribeBeat({
        status: "transcribing",
        seconds_processed: Number.NaN,
        total_seconds: 60,
      }),
    ).toEqual(STARTING);
  });

  it("treats terminal states as the starting beat, so nothing lingers mid-run", () => {
    expect(transcribeBeat({ status: "saved", path: "t.jsonl" })).toEqual(STARTING);
    expect(transcribeBeat({ status: "error", message: "boom" })).toEqual(STARTING);
  });
});
