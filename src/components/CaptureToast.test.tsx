import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CaptureToast } from "./CaptureToast";
import { DISTILL_STATE_EVENT } from "../events";
import { emitFromBackend, onCommand, resetTauriMocks } from "../test/tauri";

vi.mock("@tauri-apps/api/core", () => import("../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../test/tauri"));

/* The two event names the hooks keep private. Spelled out rather than
 * exported for a test's convenience: these strings are the contract with Rust
 * (`audio_cmds`, `transcribe_cmds`), so pinning them here is the point. */
const CAPTURE_STATE_EVENT = "capture:state";
const TRANSCRIPTION_STATE_EVENT = "transcription:state";

const IDLE = { phase: "idle", sources: { loopback: "off", microphone: "off" } };

/** Render with capture idle, and wait for the seed read to land — the pipeline
 * hooks drop events that arrive while a capture is engaged, so the phase has to
 * be settled before anything is emitted. */
async function renderToast() {
  onCommand("capture_phase", () => IDLE);
  const result = render(<CaptureToast />);
  await act(async () => {
    await Promise.resolve();
  });
  return result;
}

/** Pretend Rust finished a distill run. */
async function distill(payload: Record<string, unknown>) {
  await act(async () => {
    emitFromBackend(DISTILL_STATE_EVENT, payload);
  });
}

describe("CaptureToast", () => {
  beforeEach(() => {
    resetTauriMocks();
    vi.useFakeTimers({ shouldAdvanceTime: true });
    // The capture hooks seed from `capture_phase` and then listen; nothing
    // should be showing until the pipeline says something.
    onCommand("capture_phase", () => IDLE);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("says nothing at rest", async () => {
    await renderToast();

    expect(screen.queryByTestId("capture-toast")).not.toBeInTheDocument();
  });

  it("announces a saved note, then takes itself away", async () => {
    // The whole reason this is a toast: it is an announcement, not state. It
    // used to render as a bare `SAVED` line stacked under the sidebar's IDLE
    // dot, where it stayed long after it was true.
    await renderToast();

    await distill({ status: "saved", path: "n.md", session_path: "s.jsonl" });
    expect(screen.getByTestId("capture-toast")).toHaveTextContent("Note saved.");
    expect(screen.getByRole("status")).toBeInTheDocument();

    await act(async () => {
      vi.advanceTimersByTime(4000);
    });

    expect(screen.queryByTestId("capture-toast")).not.toBeInTheDocument();
  });

  it("keeps a failure up until it is dismissed, and announces it assertively", async () => {
    // A failure arrives asynchronously and the user may not be looking
    // (docs/DESIGN_SYSTEM.md §6), so it must not time out from under them.
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await renderToast();

    await distill({ status: "error", message: "claude exited 1", session_path: "s.jsonl" });

    expect(screen.getByRole("alert")).toHaveTextContent(/Couldn't distill that capture/);

    await act(async () => {
      vi.advanceTimersByTime(20000);
    });
    expect(screen.getByTestId("capture-toast")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByTestId("capture-toast")).not.toBeInTheDocument();
  });

  it("points a failure at the view that still holds the session", async () => {
    // Dismissing is only safe because the toast is not the only record: the
    // session stays listed under Needs attention until it is retried.
    await renderToast();

    await distill({ status: "error", message: "boom", session_path: "s.jsonl" });

    expect(screen.getByRole("alert")).toHaveTextContent("Needs attention");
  });

  it("stays quiet for a skipped distill", async () => {
    // Nothing distillable (a silent capture) is a benign non-event. Popping a
    // panel to report that nothing happened is worse than saying nothing.
    await renderToast();

    await distill({ status: "skipped", reason: "no speech", session_path: "s.jsonl" });

    expect(screen.queryByTestId("capture-toast")).not.toBeInTheDocument();
  });

  it("does not announce a saved transcript, because the note is the result", async () => {
    // One capture, one announcement. Transcription saving is a step on the way
    // to the note, and distill is about to speak for both.
    await renderToast();

    await act(async () => {
      emitFromBackend(TRANSCRIPTION_STATE_EVENT, { status: "saved", path: "t.txt" });
    });

    expect(screen.queryByTestId("capture-toast")).not.toBeInTheDocument();
  });

  it("lets a second outcome speak after the first was dismissed", async () => {
    // The dwell timer is keyed on the notice's identity, so a new outcome is
    // never silenced by the previous one having been seen out.
    await renderToast();

    await distill({ status: "saved", path: "a.md", session_path: "a.jsonl" });
    await act(async () => {
      vi.advanceTimersByTime(4000);
    });
    expect(screen.queryByTestId("capture-toast")).not.toBeInTheDocument();

    // A later capture fails; the toast has to come back.
    await act(async () => {
      emitFromBackend(CAPTURE_STATE_EVENT, IDLE);
    });
    await distill({ status: "error", message: "boom", session_path: "b.jsonl" });

    expect(screen.getByRole("alert")).toBeInTheDocument();
  });
});
