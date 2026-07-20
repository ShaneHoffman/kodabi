import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FLASH_MS, QuickCapture } from "./QuickCapture";
import type { QuickCaptureOutcome } from "../quickCapture";
import { QUICK_CAPTURE_SHOWN_EVENT } from "../events";
import {
  emitFromBackend,
  invoke,
  invokedCommands,
  onCommand,
  resetTauriMocks,
} from "../test/tauri";

vi.mock("@tauri-apps/api/core", () => import("../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../test/tauri"));

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
    onCommand("quick_capture_submit", () => outcome("paradise-golf"));
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
    onCommand("quick_capture_submit", () => outcome("paradise-golf"));
    render(<QuickCapture />);

    await user.type(box(), "  ring the vendor back  ");
    await user.click(screen.getByRole("button", { name: "File it" }));

    expect(invoke).toHaveBeenCalledWith("quick_capture_submit", {
      text: "ring the vendor back",
    });
    expect(await screen.findByText("→ paradise-golf")).toBeInTheDocument();
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
    onCommand("quick_capture_submit", () => outcome("paradise-golf"));
    render(<QuickCapture />);

    await user.type(box(), "ring the vendor back{Enter}");

    expect(await screen.findByText("→ paradise-golf")).toBeInTheDocument();
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
    onCommand("quick_capture_submit", () => outcome("paradise-golf"));
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
    onCommand("quick_capture_submit", () => outcome("paradise-golf"));
    render(<QuickCapture />);

    await user.type(box(), "ring the vendor back{Enter}");
    await screen.findByText("→ paradise-golf");

    await reshow();

    // Unlike an error, a spent success flash is noise on a fresh box.
    expect(screen.queryByText("→ paradise-golf")).not.toBeInTheDocument();
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
      filing.resolve(outcome("paradise-golf"));
    });

    // The first thought did land — but its success belongs to a capture
    // session that is over, so it may not clear, flash over, or dismiss the
    // one the user is in the middle of.
    expect(box()).toHaveValue("book the flights");
    expect(screen.queryByText("→ paradise-golf")).not.toBeInTheDocument();
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
});
