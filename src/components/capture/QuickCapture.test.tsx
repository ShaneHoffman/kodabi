import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FLASH_MS, QuickCapture } from "./QuickCapture";
import type { QuickCaptureOutcome } from "../../quickCapture";
import { QUICK_CAPTURE_SHOWN_EVENT } from "../../events";
import type { CaptureStateEvent } from "../../useCaptureState";
import {
  emitFromBackend,
  invoke,
  invokedCommands,
  onCommand,
  resetTauriMocks,
} from "../../test/tauri";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

function outcome(project: string | null): QuickCaptureOutcome {
  return {
    id: "n_a1b2c3",
    path: project ? `Projects/${project}/thought.md` : "Inbox/thought.md",
    project,
    confidence: 0.92,
  };
}

function box(): HTMLTextAreaElement {
  return screen.getByRole("textbox", { name: "Capture a thought" });
}

/** A `quick_capture_submit` the test settles by hand, so a submit can still be
 * in flight while the window is dismissed and popped again. */
function deferredSubmit(): {
  resolve: (outcome: QuickCaptureOutcome) => void;
  reject: (reason: unknown) => void;
} {
  let resolve!: (outcome: QuickCaptureOutcome) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<QuickCaptureOutcome>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  onCommand("quick_capture_submit", () => promise);
  return {
    resolve: (settled) => resolve(settled),
    reject: (reason) => reject(reason),
  };
}

/** The backend bringing the window forward again (Rust emits this on show +
 * focus), which is what starts a new capture session. */
async function reshow(): Promise<void> {
  await act(async () => {
    emitFromBackend(QUICK_CAPTURE_SHOWN_EVENT);
  });
}

/** A capture phase arriving from the backend, the way `audio_cmds` broadcasts
 * it. */
async function emitCapture(state: CaptureStateEvent): Promise<void> {
  await act(async () => {
    emitFromBackend("capture:state", state);
  });
}

/** Audio genuinely reaching disk: the one state that earns the green. */
const LISTENING: CaptureStateEvent = {
  phase: "listening",
  sources: { loopback: "live", microphone: "live" },
};

/** A capture that has been asked for but has not opened a device yet. Engaged,
 * but nothing is being recorded. */
const STARTING: CaptureStateEvent = {
  phase: "starting",
  sources: { loopback: "off", microphone: "off" },
};

/** A running capture whose sources have all dropped out. Still engaged (it
 * will resume with no further press), still not recording. */
const RECONNECTING: CaptureStateEvent = {
  phase: "degraded",
  sources: { loopback: "stalled", microphone: "stalled" },
};

/** Put the box into its recording state, the way `audio_cmds` does. */
async function goLive(): Promise<void> {
  await emitCapture(LISTENING);
}

describe("QuickCapture", () => {
  beforeEach(() => {
    resetTauriMocks();
    onCommand("hide_quick_capture", () => null);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("submits the trimmed draft on Enter", async () => {
    const user = userEvent.setup();
    onCommand("quick_capture_submit", () => outcome("briarwood-golf"));
    render(<QuickCapture />);

    await user.type(box(), "  ring the vendor back  {Enter}");

    expect(invoke).toHaveBeenCalledWith("quick_capture_submit", {
      text: "ring the vendor back",
    });
  });

  it("submits the draft from the visible button, not only from Enter", async () => {
    // The pointer-only path this window is required to have: it is opened by
    // hotkey, but a user who never focused it cannot press Enter into it
    // (docs/DESIGN_SYSTEM.md §6).
    const user = userEvent.setup();
    onCommand("quick_capture_submit", () => outcome("briarwood-golf"));
    render(<QuickCapture />);

    await user.type(box(), "  ring the vendor back  ");
    await user.click(screen.getByRole("button", { name: "File it" }));

    expect(invoke).toHaveBeenCalledWith("quick_capture_submit", {
      text: "ring the vendor back",
    });
    expect(await screen.findByText("→ briarwood-golf")).toBeInTheDocument();
  });

  it("offers no submit until there is something to file", async () => {
    const user = userEvent.setup();
    onCommand("quick_capture_submit", () => outcome(null));
    render(<QuickCapture />);

    await user.click(screen.getByRole("button", { name: "File it" }));
    expect(invokedCommands()).not.toContain("quick_capture_submit");

    // Whitespace is not a thought either.
    await user.type(box(), "   ");
    await user.click(screen.getByRole("button", { name: "File it" }));
    expect(invokedCommands()).not.toContain("quick_capture_submit");
  });

  it("clears the draft and flashes where the note landed", async () => {
    const user = userEvent.setup();
    onCommand("quick_capture_submit", () => outcome("briarwood-golf"));
    render(<QuickCapture />);

    await user.type(box(), "ring the vendor back{Enter}");

    expect(await screen.findByText("→ briarwood-golf")).toBeInTheDocument();
    expect(box()).toHaveValue("");
  });

  it("flashes Inbox when the router placed nothing", async () => {
    const user = userEvent.setup();
    onCommand("quick_capture_submit", () => outcome(null));
    render(<QuickCapture />);

    await user.type(box(), "a stray thought{Enter}");

    expect(await screen.findByText("→ Inbox")).toBeInTheDocument();
  });

  it("hides the window once the flash has been read", async () => {
    vi.useFakeTimers();
    onCommand("quick_capture_submit", () => outcome("briarwood-golf"));
    render(<QuickCapture />);

    // fireEvent, not userEvent: userEvent's typing loop awaits real timers
    // that fake timers never fire, so it deadlocks. These two synchronous
    // events are the same path the component sees from a real keystroke.
    fireEvent.change(box(), { target: { value: "ring the vendor back" } });
    fireEvent.keyDown(box(), { key: "Enter" });
    // Settle the submit promise so the flash timer is armed.
    await act(async () => {});

    // The flash has to last long enough to read: no hide before it elapses.
    expect(invokedCommands()).not.toContain("hide_quick_capture");

    act(() => {
      vi.advanceTimersByTime(FLASH_MS);
    });

    expect(invokedCommands()).toContain("hide_quick_capture");
  });

  it("keeps the draft and shows the backend message when filing fails", async () => {
    const user = userEvent.setup();
    onCommand("quick_capture_submit", () => {
      throw "the vault is not writable";
    });
    render(<QuickCapture />);

    await user.type(box(), "ring the vendor back{Enter}");

    expect(await screen.findByText(/the vault is not writable/)).toBeInTheDocument();
    // A lost thought is the failure that matters here: the draft stays put and
    // the window does not dismiss itself out from under it.
    expect(box()).toHaveValue("ring the vendor back");
    expect(invokedCommands()).not.toContain("hide_quick_capture");
  });

  it("never submits an empty draft", async () => {
    const user = userEvent.setup();
    render(<QuickCapture />);

    await user.type(box(), "   {Enter}");

    expect(invokedCommands()).not.toContain("quick_capture_submit");
  });

  it("keeps a failed capture's message across a dismiss and re-show", async () => {
    const user = userEvent.setup();
    onCommand("quick_capture_submit", () => {
      throw "the vault is not writable";
    });
    render(<QuickCapture />);

    await user.type(box(), "ring the vendor back{Enter}");
    await screen.findByText(/the vault is not writable/);

    await reshow();

    // A blur-dismiss must not bury a failed capture: the error and the draft
    // are both still there the next time the box pops.
    expect(screen.getByText(/the vault is not writable/)).toBeInTheDocument();
    expect(box()).toHaveValue("ring the vendor back");
  });

  it("clears a stale success flash on re-show", async () => {
    const user = userEvent.setup();
    onCommand("quick_capture_submit", () => outcome("briarwood-golf"));
    render(<QuickCapture />);

    await user.type(box(), "ring the vendor back{Enter}");
    await screen.findByText("→ briarwood-golf");

    await reshow();

    // Unlike an error, a spent success flash is noise on a fresh box.
    expect(screen.queryByText("→ briarwood-golf")).not.toBeInTheDocument();
  });

  it("leaves a fresh draft alone when a stale submit lands", async () => {
    const user = userEvent.setup();
    const filing = deferredSubmit();
    render(<QuickCapture />);

    await user.type(box(), "ring the vendor back{Enter}");
    expect(screen.getByText("Filing…")).toBeInTheDocument();

    // Dismissed and popped again while the write is still in flight, and the
    // user has moved on to a different thought.
    await reshow();
    await user.clear(box());
    await user.type(box(), "book the flights");

    await act(async () => {
      filing.resolve(outcome("briarwood-golf"));
    });

    // The first thought did land — but its success belongs to a capture
    // session that is over, so it may not clear, flash over, or dismiss the
    // one the user is in the middle of.
    expect(box()).toHaveValue("book the flights");
    expect(screen.queryByText("→ briarwood-golf")).not.toBeInTheDocument();
    expect(invokedCommands()).not.toContain("hide_quick_capture");
  });

  it("leaves a fresh draft alone when a stale failure lands", async () => {
    const user = userEvent.setup();
    const filing = deferredSubmit();
    render(<QuickCapture />);

    await user.type(box(), "ring the vendor back{Enter}");
    await reshow();
    await user.clear(box());
    await user.type(box(), "book the flights");

    await act(async () => {
      filing.reject("the vault is not writable");
    });

    // Same guard from the other side: a late failure must not wipe the draft
    // or pin an error on a capture it has nothing to do with.
    expect(box()).toHaveValue("book the flights");
    expect(
      screen.queryByText(/the vault is not writable/),
    ).not.toBeInTheDocument();
  });

  it("files the note typed alongside a recording when the recording is stopped", async () => {
    // The box invites one ("Add a note alongside the recording…") and it is a
    // separate note, not part of the transcript, so nothing downstream would
    // ever pick it up. Stopping without filing it simply lost the thought.
    const user = userEvent.setup();
    onCommand("start_capture", () => null);
    onCommand("stop_capture", () => null);
    onCommand("quick_capture_submit", () => outcome("briarwood-golf"));
    render(<QuickCapture />);

    await goLive();
    await user.type(box(), "ask about the invoice{Enter}");

    expect(invokedCommands()).toContain("stop_capture");
    expect(invoke).toHaveBeenCalledWith("quick_capture_submit", {
      text: "ask about the invoice",
    });
  });

  it("tells the truth about what Escape does to a recording", async () => {
    // There is no cancel: Escape runs the same stop-and-file path Enter does,
    // and a hint promising otherwise would be a lie about a recording the user
    // is trying to abandon.
    onCommand("start_capture", () => null);
    onCommand("stop_capture", () => null);
    render(<QuickCapture />);

    await goLive();

    expect(screen.getByText(/stops and files/)).toBeInTheDocument();
    expect(screen.queryByText(/Esc cancels/)).not.toBeInTheDocument();

    fireEvent.keyDown(box(), { key: "Escape" });
    expect(invokedCommands()).toContain("stop_capture");
  });

  it("keeps the recording chrome through a start that has not gone live", async () => {
    // "Engaged" and "on air" are different questions. A capture that is still
    // opening its devices is engaged, so the header, the timer and Stop all
    // belong on screen — but nothing is reaching disk, so none of it may wear
    // the reserved green.
    onCommand("start_capture", () => null);
    const { container } = render(<QuickCapture />);

    await emitCapture(STARTING);

    expect(screen.getByRole("status")).toHaveTextContent("Starting…");
    expect(container.querySelector(".capture__core")).not.toHaveClass(
      "capture__core--live",
    );
    expect(container.querySelector(".capture__glow")).toBeNull();
    expect(container.querySelector(".capture__wave")).toHaveClass(
      "capture__wave--quiet",
    );

    // The affordances match the session, not the audio: Stop is the only thing
    // on offer while a capture is running.
    expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Record" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "File it" })).not.toBeInTheDocument();
    expect(screen.getByText(/stops and files/)).toBeInTheDocument();
  });

  it("stops the capture on Enter mid-start instead of filing and vanishing", async () => {
    // The defect this guards: Enter pressed a beat after Record used to file
    // the draft and dismiss the window, leaving a capture running behind a
    // window that had just told the user it was done.
    onCommand("start_capture", () => null);
    onCommand("stop_capture", () => null);
    onCommand("quick_capture_submit", () => outcome("briarwood-golf"));
    render(<QuickCapture />);

    await emitCapture(STARTING);
    fireEvent.change(box(), { target: { value: "ask about the invoice" } });
    fireEvent.keyDown(box(), { key: "Enter" });

    expect(invokedCommands()).toContain("stop_capture");
    expect(invoke).toHaveBeenCalledWith("quick_capture_submit", {
      text: "ask about the invoice",
    });
  });

  it("stops the capture on Escape mid-start before hiding", async () => {
    onCommand("start_capture", () => null);
    onCommand("stop_capture", () => null);
    render(<QuickCapture />);

    await emitCapture(STARTING);
    fireEvent.keyDown(box(), { key: "Escape" });

    // Escape dismisses the window, so it must not leave a capture running
    // behind it with no visible way back to Stop.
    expect(invokedCommands()).toContain("stop_capture");
    expect(invokedCommands()).toContain("hide_quick_capture");
  });

  it("keeps a reconnecting capture on screen, in ink", async () => {
    onCommand("stop_capture", () => null);
    const { container } = render(<QuickCapture />);

    await emitCapture(RECONNECTING);

    // Both sources dropped: the session is still engaged and will resume with
    // no further press, so the chrome stays — but the green does not.
    expect(screen.getByRole("status")).toHaveTextContent("Reconnecting");
    expect(container.querySelector(".capture__core")).not.toHaveClass(
      "capture__core--live",
    );
    expect(container.querySelector(".capture__glow")).toBeNull();
    expect(container.querySelector(".capture__wave")).toHaveClass(
      "capture__wave--quiet",
    );
    expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();

    fireEvent.keyDown(box(), { key: "Enter" });
    expect(invokedCommands()).toContain("stop_capture");
  });

  it("keeps the clock running across a mid-capture dropout", async () => {
    vi.useFakeTimers();
    const { container } = render(<QuickCapture />);

    // fireEvent/emit only under fake timers — see the flash test above.
    await goLive();
    expect(screen.getByText("0:00")).toBeInTheDocument();
    act(() => {
      vi.advanceTimersByTime(3000);
    });
    expect(screen.getByText("0:03")).toBeInTheDocument();

    await emitCapture(RECONNECTING);
    // The dot reacts at once; the announced label settles first.
    expect(container.querySelector(".capture__core")).not.toHaveClass(
      "capture__core--live",
    );
    expect(screen.getByRole("status")).toHaveTextContent("Listening");

    act(() => {
      vi.advanceTimersByTime(1000);
    });
    expect(screen.getByRole("status")).toHaveTextContent("Reconnecting");
    // The clock is the length of the recording, not the length of the current
    // uninterrupted stretch of audio: a dropout must not rewind it to 0:00.
    expect(screen.getByText("0:04")).toBeInTheDocument();

    await goLive();
    act(() => {
      vi.advanceTimersByTime(1000);
    });
    expect(container.querySelector(".capture__core")).toHaveClass(
      "capture__core--live",
    );
    expect(screen.getByText("0:05")).toBeInTheDocument();
  });
});
