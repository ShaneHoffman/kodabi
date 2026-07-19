import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { QuickCapture } from "./QuickCapture";
import type { QuickCaptureOutcome } from "../quickCapture";
import { invoke, onCommand, resetTauriMocks } from "../test/tauri";

vi.mock("@tauri-apps/api/core", () => import("../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../test/tauri"));

/** How long the destination flashes before the window hides itself
 * (`FLASH_MS` in QuickCapture.tsx). */
const FLASH_MS = 600;

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

function invokedCommands(): string[] {
  return invoke.mock.calls.map(([command]) => command);
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

    expect(await screen.findByText("the vault is not writable")).toBeInTheDocument();
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
});
