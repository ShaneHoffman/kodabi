import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CaptureOverlayPill } from "./CaptureOverlayPill";
import type { CaptureStateEvent } from "../useCaptureState";
import {
  emitFromBackend,
  invokedCommands,
  onCommand,
  resetTauriMocks,
} from "../test/tauri";

vi.mock("@tauri-apps/api/core", () => import("../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../test/tauri"));

/** The event `useCaptureState` subscribes to, mirroring
 * `capture_control::CAPTURE_STATE_EVENT`. Held privately by that hook, so the
 * name is restated here rather than imported. */
const CAPTURE_STATE_EVENT = "capture:state";

const IDLE: CaptureStateEvent = {
  phase: "idle",
  sources: { loopback: "off", microphone: "off" },
};

const LISTENING: CaptureStateEvent = {
  phase: "listening",
  sources: { loopback: "live", microphone: "live" },
};

/** Deliver a `capture:state` payload the way the backend broadcast would. */
function emitCaptureState(state: CaptureStateEvent): void {
  act(() => {
    emitFromBackend(CAPTURE_STATE_EVENT, state);
  });
}

/** Let the seed `invoke` and the `listen(...).then(...)` subscription settle. */
async function flush(): Promise<void> {
  await act(async () => {});
}

/** Render with `capture_phase` seeding the given state, as mounting mid-capture
 * would. The pill's window is pre-created hidden, so this is the real path.
 * Time is advanced past the label debounce, since every assertion here is about
 * the settled label rather than the debounce itself. */
async function renderSeeded(seed: CaptureStateEvent) {
  onCommand("capture_phase", () => seed);
  const result = render(<CaptureOverlayPill />);
  await flush();
  act(() => void vi.advanceTimersByTime(500));
  return result;
}

describe("CaptureOverlayPill", () => {
  beforeEach(() => {
    resetTauriMocks();
    // The label is debounced, so tests drive time explicitly rather than
    // waiting on it.
    vi.useFakeTimers({ shouldAdvanceTime: true });
  });

  it("shows the listening state when seeded mid-capture", async () => {
    const { container } = await renderSeeded(LISTENING);

    const label = screen.getByRole("status");
    expect(label).toHaveTextContent("Listening");
    // Two carriers, and the invariant needs both. The reserved green claims
    // audio is genuinely being recorded and lives on the mark alone (as text it
    // fails the AA contrast floor in the light theme); the label says the same
    // thing through value, at full ink rather than faint.
    expect(container.querySelector(".spirit-mark")).toHaveClass("is-listening");
    expect(label).toHaveClass("text-text");
  });

  it("renders nothing at all while capture is idle", async () => {
    await renderSeeded(IDLE);

    // The backend hides the window too; this is the independent second guard.
    // An empty pill is acceptable, a pill claiming to record is not.
    expect(screen.queryByTestId("capture-overlay-pill")).not.toBeInTheDocument();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("disappears when the backend reports capture stopped", async () => {
    await renderSeeded(LISTENING);
    expect(screen.getByTestId("capture-overlay-pill")).toBeInTheDocument();

    // Whichever path drove the stop (hotkey, tray, IPC, watchdog), it arrives
    // as this one event.
    emitCaptureState(IDLE);
    act(() => void vi.advanceTimersByTime(500));

    expect(screen.queryByTestId("capture-overlay-pill")).not.toBeInTheDocument();
  });

  it("appears when the backend reports a capture starting", async () => {
    await renderSeeded(IDLE);
    expect(screen.queryByTestId("capture-overlay-pill")).not.toBeInTheDocument();

    emitCaptureState({
      phase: "starting",
      sources: { loopback: "off", microphone: "off" },
    });
    act(() => void vi.advanceTimersByTime(500));

    // Present during the device-negotiation window: a pill that said nothing
    // for that ~1s would be indistinguishable from a capture that never began.
    expect(screen.getByTestId("capture-overlay-pill")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Starting");
  });

  it("names what is still recording when a source drops out", async () => {
    const { container } = await renderSeeded({
      phase: "degraded",
      sources: { loopback: "failed", microphone: "live" },
    });

    const label = screen.getByRole("status");
    // Never plain "Listening" while only one source survives, but still on air:
    // the mic genuinely is recording, so the mark keeps the green and the label
    // stays at full ink.
    expect(label).toHaveTextContent("Mic only");
    expect(container.querySelector(".spirit-mark")).toHaveClass("is-degraded");
    expect(label).toHaveClass("text-text");
  });

  it("drops the green when a degraded capture has nothing live", async () => {
    const { container } = await renderSeeded({
      phase: "degraded",
      sources: { loopback: "stalled", microphone: "stalled" },
    });

    const label = screen.getByRole("status");
    expect(label).toHaveTextContent("Reconnecting");
    // Nothing is reaching disk, so neither carrier may claim it is: the mark
    // falls back to the moving ink form, and the label recedes to faint.
    expect(container.querySelector(".spirit-mark")).toHaveClass("is-reconnecting");
    expect(label).toHaveClass("text-text-faint");
    expect(label).not.toHaveClass("text-text");
  });

  it("dismisses the pill for the current capture session", async () => {
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    onCommand("dismiss_capture_overlay", () => null);
    await renderSeeded(LISTENING);

    await user.click(screen.getByTestId("capture-overlay-dismiss"));

    // The backend owns the hide and the session-scoped flag; the pill only
    // asks.
    expect(invokedCommands()).toContain("dismiss_capture_overlay");
  });

  it("keeps the whole pill draggable and the dismiss control clickable", async () => {
    await renderSeeded(LISTENING);

    // `deep`, not the bare attribute: bare drags only on a press landing
    // exactly on the root, so the pill inside it would be a dead zone.
    expect(screen.getByTestId("capture-overlay-root")).toHaveAttribute(
      "data-tauri-drag-region",
      "deep",
    );
    // Tauri's drag script excludes <button> subtrees, which is the only reason
    // a control inside a deep drag region stays pressable. A div would drag.
    expect(screen.getByTestId("capture-overlay-dismiss").tagName).toBe("BUTTON");
  });

  it("keeps the pill inset from the window edge so its shadow can fade out", async () => {
    await renderSeeded(LISTENING);

    // The pill carries a drop shadow. Flush to the window bounds that shadow is
    // clipped flat and reads as a rectangle around a rounded pill, which is
    // exactly what the transparent window is meant to avoid. The padded frame
    // is the fade-out room, and the window is sized for it.
    expect(screen.getByTestId("capture-overlay-root")).toHaveClass("p-sm");
  });
});
