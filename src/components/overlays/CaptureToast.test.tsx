import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CapturePipelineProvider } from "../providers/CapturePipelineProvider";
import { ModelStatusProvider } from "../providers/ModelStatusProvider";
import { CaptureToast } from "./CaptureToast";
import { DISTILL_STATE_EVENT } from "../../events";
import { emitFromBackend, onCommand, resetTauriMocks } from "../../test/tauri";
import { CLAUDE_MISSING_MESSAGE } from "../../test/claudeMissing";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

/* The two event names the hooks keep private. Spelled out rather than
 * exported for a test's convenience: these strings are the contract with Rust
 * (`audio_cmds`, `transcribe_cmds`), so pinning them here is the point. */
const CAPTURE_STATE_EVENT = "capture:state";
const TRANSCRIPTION_STATE_EVENT = "transcription:state";

const IDLE = { phase: "idle", sources: { loopback: "off", microphone: "off" } };
const LISTENING = {
  phase: "listening",
  sources: { loopback: "live", microphone: "live" },
};

/** Render with capture idle, and wait for the seed read to land — the pipeline
 * hooks drop events that arrive while a capture is engaged, so the phase has to
 * be settled before anything is emitted. */
async function renderToast() {
  onCommand("capture_phase", () => IDLE);
  const result = render(
    <ModelStatusProvider>
      <CapturePipelineProvider>
        <CaptureToast />
      </CapturePipelineProvider>
    </ModelStatusProvider>,
  );
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

  it("stays quiet through progress and success, since those now live in the Inbox", async () => {
    // The whole reason this toast exists now: a failure, not an announcement.
    // Progress and a saved note render live in the Inbox placeholder instead
    // (InboxView.test.tsx), so none of the pipeline's non-failure states
    // should ever surface here.
    await renderToast();

    await act(async () => {
      emitFromBackend(TRANSCRIPTION_STATE_EVENT, {
        status: "transcribing",
        seconds_processed: 12,
        total_seconds: 60,
      });
    });
    expect(screen.queryByTestId("capture-toast")).not.toBeInTheDocument();

    await act(async () => {
      emitFromBackend(TRANSCRIPTION_STATE_EVENT, { status: "queued" });
    });
    expect(screen.queryByTestId("capture-toast")).not.toBeInTheDocument();

    await act(async () => {
      emitFromBackend(TRANSCRIPTION_STATE_EVENT, { status: "saved", path: "t.txt" });
    });
    expect(screen.queryByTestId("capture-toast")).not.toBeInTheDocument();

    await distill({ status: "distilling" });
    expect(screen.queryByTestId("capture-toast")).not.toBeInTheDocument();

    await distill({ status: "saved", path: "n.md", session_path: "s.jsonl" });
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

  it("names the missing prerequisite when that is why the distill failed", async () => {
    // The first capture on a machine without the CLI. Sending the user to a
    // Needs attention row whose Retry can only fail the same way is the dead
    // end this fork exists to break, so the toast says what to install.
    await renderToast();

    await distill({
      status: "error",
      message: CLAUDE_MISSING_MESSAGE,
      session_path: "s.jsonl",
    });

    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent(/Claude Code isn't installed/);
    expect(alert).toHaveTextContent(/docs\.claude\.com/);
  });

  it("stays quiet for a skipped distill", async () => {
    // Nothing distillable (a silent capture) is a benign non-event. Popping a
    // panel to report that nothing happened is worse than saying nothing.
    await renderToast();

    await distill({ status: "skipped", reason: "no speech", session_path: "s.jsonl" });

    expect(screen.queryByTestId("capture-toast")).not.toBeInTheDocument();
  });

  it("announces a transcription failure too", async () => {
    await renderToast();

    await act(async () => {
      emitFromBackend(TRANSCRIPTION_STATE_EVENT, { status: "error", message: "boom" });
    });

    expect(screen.getByRole("alert")).toHaveTextContent(/Couldn't transcribe that capture/);
  });

  it("blames the missing models when that is the actual cause, and says the audio is safe", async () => {
    // Same failure event, different truth behind it. A transcription that
    // failed for want of models has not lost the recording: the spill survives
    // and a later launch retries it, so pointing the user at Needs attention
    // would send them to fix something that is not broken.
    onCommand("model_status", () => ({
      ready: false,
      bytesRequired: 762_000_000,
      bytesPresent: 0,
      sets: [],
      downloading: false,
      modelsDir: "C:\\app\\.models",
    }));
    await renderToast();

    await act(async () => {
      emitFromBackend(TRANSCRIPTION_STATE_EVENT, { status: "error", message: "boom" });
    });

    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent(/the models aren't downloaded/);
    expect(alert).toHaveTextContent(/recording is safe/);
    expect(alert).not.toHaveTextContent(/Needs attention/);
  });

  it("reports a second failure of the same kind after the first was dismissed", async () => {
    // The notice ids name a KIND of outcome, not a run, so the record of what
    // has been dismissed has to end when the pipeline goes quiet. Without
    // that, dismissing one capture's failure swallowed the next capture's
    // identical failure entirely — silence for a note that never landed.
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    await renderToast();

    await distill({ status: "error", message: "boom", session_path: "a.jsonl" });
    await user.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByTestId("capture-toast")).not.toBeInTheDocument();

    // A second capture runs and fails the same way. Both of its steps finish
    // inside the dwell, so no timer ever fires to refresh what was seen.
    await act(async () => {
      emitFromBackend(CAPTURE_STATE_EVENT, LISTENING);
    });
    await act(async () => {
      emitFromBackend(CAPTURE_STATE_EVENT, IDLE);
    });
    await distill({ status: "error", message: "boom again", session_path: "b.jsonl" });

    expect(screen.getByRole("alert")).toHaveTextContent(/Couldn't distill that capture/);
  });
});
