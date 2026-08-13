import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CaptureOverlayPill } from "./CaptureOverlayPill";
import type { CaptureStateEvent } from "../../useCaptureState";
import { emitFromBackend, onCommand, resetTauriMocks } from "../../test/tauri";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

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

  afterEach(() => {
    vi.useRealTimers();
  });

  it("shows the listening state when seeded mid-capture", async () => {
    const { container } = await renderSeeded(LISTENING);

    const label = screen.getByRole("status");
    expect(label).toHaveTextContent("Listening");
    // Two carriers, and the invariant needs both, but only one of them is
    // green: the reserved green claims audio is genuinely being recorded and
    // lives on the mark alone, so that on a desktop this window shares with
    // other people's applications there is exactly one green thing. The label
    // carries the same claim in words, at faint ink in every state.
    expect(container.querySelector(".spirit-mark")).toHaveClass("is-listening");
    expect(label).toHaveClass("text-ink-dim");
    expect(label).not.toHaveClass("text-kodama-ink");
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
    // the mic genuinely is recording, so the mark keeps the green. The label
    // reports the narrowed state in words, not by changing colour.
    expect(label).toHaveTextContent("Mic only");
    expect(container.querySelector(".spirit-mark")).toHaveClass("is-degraded");
    expect(label).toHaveClass("text-ink-dim");
    expect(label).not.toHaveClass("text-kodama-ink");
  });

  it("drops the green from the mark when a degraded capture has nothing live", async () => {
    const { container } = await renderSeeded({
      phase: "degraded",
      sources: { loopback: "stalled", microphone: "stalled" },
    });

    const label = screen.getByRole("status");
    expect(label).toHaveTextContent("Reconnecting");
    // Nothing is reaching disk, so the one carrier that can claim it stops:
    // the mark falls back to the moving ink form. The label, faint throughout,
    // says so in words instead.
    expect(container.querySelector(".spirit-mark")).toHaveClass("is-reconnecting");
    expect(label).toHaveClass("text-ink-dim");
    expect(label).not.toHaveClass("text-kodama-ink");
  });

  it("counts the session up from the moment capture engaged", async () => {
    await renderSeeded(LISTENING);

    // The pill claims a recording is running; a recording indicator that cannot
    // say how long it has been running is asking to be trusted about the one
    // thing the user would check.
    expect(screen.getByText("0:00")).toBeInTheDocument();

    act(() => void vi.advanceTimersByTime(3000));
    expect(screen.getByText("0:03")).toBeInTheDocument();
  });

  it("carries no controls at all", async () => {
    await renderSeeded(LISTENING);

    // Pure status. A pill the user can dismiss is a recording that can be made
    // invisible, which is the one thing this window exists to prevent; stopping
    // is the global shortcut and the main window.
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("keeps the whole pill draggable", async () => {
    await renderSeeded(LISTENING);

    // `deep`, not the bare attribute: bare drags only on a press landing
    // exactly on this element, so the mark, the label and the clock inside it
    // would each be a dead zone.
    expect(screen.getByTestId("capture-overlay-pill")).toHaveAttribute(
      "data-tauri-drag-region",
      "deep",
    );
  });

  it("fills its window edge to edge, with no frame around it", async () => {
    const { container } = await renderSeeded(LISTENING);

    // The pill is the whole window. A transparent webview window is not
    // click-through, so any margin around the pill would still show the grab
    // cursor and swallow clicks meant for the application underneath — an
    // invisible thing taking the mouse. Flush bounds keep what you see and what
    // you can grab the same shape, and are why `glass-pill` carries no drop
    // shadow: with nowhere to fade, it would clip into a dark wall instead.
    const pill = screen.getByTestId("capture-overlay-pill");
    expect(pill.parentElement).toBe(container);
    expect(pill).toHaveClass("h-screen", "w-screen");
    expect(pill).not.toHaveClass("p-3");
  });
});
