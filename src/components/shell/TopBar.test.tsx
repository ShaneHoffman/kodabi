import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CapturePipelineProvider } from "../providers/CapturePipelineProvider";
import { ModelStatusProvider } from "../providers/ModelStatusProvider";
import { NavigationProvider } from "../providers/NavigationProvider";
import { UpdaterProvider } from "../providers/UpdaterProvider";
import { MainContent } from "./MainContent";
import { TopBar } from "./TopBar";
import { emitFromBackend, onCommand, resetTauriMocks } from "../../test/tauri";
import { resetUpdaterMocks } from "../../test/updater";
import {
  close,
  minimize,
  resetWindowMocks,
  resizeListenerCount,
  setMaximized,
  toggleMaximize,
} from "../../test/window";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));
vi.mock("@tauri-apps/plugin-updater", () => import("../../test/updater"));
vi.mock("@tauri-apps/plugin-process", () => import("../../test/updater"));
vi.mock("@tauri-apps/api/app", () => import("../../test/updater"));
vi.mock("@tauri-apps/api/window", () => import("../../test/window"));

/** Private to useCaptureState.ts, so the other capture tests spell it here
 * too rather than exporting it just for them. */
const CAPTURE_STATE_EVENT = "capture:state";

/** The reads the bar and the views behind it make. */
function serveVault(): void {
  onCommand("capture_phase", () => ({
    phase: "idle",
    sources: { loopback: "off", microphone: "off" },
  }));
  onCommand("list_projects", () => ({ inbox_note_count: 0, projects: [] }));
  onCommand("list_notes", () => []);
  onCommand("list_failed_sessions", () => []);
  onCommand("get_settings", () => {
    throw "settings are not under test here";
  });
}

function renderShell(onOpenPalette = () => {}) {
  return render(
    <NavigationProvider>
      <UpdaterProvider>
        <ModelStatusProvider>
          <CapturePipelineProvider>
            <TopBar onOpenPalette={onOpenPalette} />
            <MainContent />
          </CapturePipelineProvider>
        </ModelStatusProvider>
      </UpdaterProvider>
    </NavigationProvider>,
  );
}

describe("TopBar", () => {
  beforeEach(() => {
    resetTauriMocks();
    resetUpdaterMocks();
    resetWindowMocks();
  });

  it("opens the command palette and names its shortcut", async () => {
    const user = userEvent.setup();
    const onOpenPalette = vi.fn();
    serveVault();

    renderShell(onOpenPalette);

    const commands = screen.getByRole("button", { name: /Commands/ });
    // The hint is part of the control's own name, so a screen reader hears the
    // key without having to find a separate node. The control is icon-only, so
    // the name is the only place left that carries it — which is exactly why
    // this asserts the accessible name and not the text content.
    expect(commands).toHaveAccessibleName("Commands (Ctrl K)");

    await user.click(commands);

    expect(onOpenPalette).toHaveBeenCalledTimes(1);
  });

  it("navigates to Settings and marks itself current", async () => {
    const user = userEvent.setup();
    serveVault();
    renderShell();
    const settings = screen.getByRole("button", { name: "Settings" });
    expect(settings).not.toHaveAttribute("aria-current");

    await user.click(settings);

    expect(settings).toHaveAttribute("aria-current", "page");
  });

  it("takes the wordmark home", async () => {
    const user = userEvent.setup();
    serveVault();
    renderShell();
    await user.click(screen.getByRole("button", { name: "Settings" }));
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Settings" })).toBeInTheDocument();
    });

    await user.click(screen.getByRole("button", { name: "kodabi" }));

    expect(await screen.findByRole("heading", { name: "Inbox" })).toBeInTheDocument();
  });

  it("reports what capture is doing, wherever the user is standing", async () => {
    serveVault();
    renderShell();
    // Not listening is not silence: the pill says so rather than disappearing,
    // which is what makes "is it recording" answerable in one place.
    expect(await screen.findByRole("status")).toHaveTextContent("Not listening");

    act(() => {
      emitFromBackend(CAPTURE_STATE_EVENT, {
        phase: "listening",
        sources: { loopback: "live", microphone: "live" },
      });
    });

    // The label is debounced (a flapping VAD must not spam the live region),
    // so this waits rather than asserting synchronously.
    await waitFor(
      () => {
        expect(screen.getByRole("status")).toHaveTextContent("Listening");
      },
      { timeout: 2000 },
    );
  });

  it("says a start failed rather than reading as a rest", async () => {
    // A start whose every source failed derives phase `idle`. This bar is the
    // only surface that reports it: the tray says "Kodabi: idle", the OS
    // start notification is suppressed for a start that captured nothing, the
    // overlay pill renders nothing while capture is inactive, and CaptureToast
    // carries distill and transcription failures only.
    serveVault();
    renderShell();
    expect(await screen.findByRole("status")).toHaveTextContent("Not listening");

    act(() => {
      emitFromBackend(CAPTURE_STATE_EVENT, {
        phase: "idle",
        sources: { loopback: "failed", microphone: "failed" },
      });
    });

    await waitFor(
      () => {
        expect(screen.getByRole("status")).toHaveTextContent(
          "Capture failed to start",
        );
      },
      { timeout: 2000 },
    );
  });

  it("says transcription is not ready yet while the models are still missing", async () => {
    // The detail slot, not the headline and not the mark: nothing is wrong
    // with capture itself, and claiming otherwise would be the same kind of
    // lie in the opposite direction.
    serveVault();
    onCommand("model_status", () => ({
      ready: false,
      bytesRequired: 762_000_000,
      bytesPresent: 0,
      sets: [],
      downloading: false,
      modelsDir: "C:\\app\\.models",
    }));
    renderShell();

    await waitFor(() => {
      expect(screen.getByRole("status")).toHaveTextContent("Transcription not ready yet");
    });
    expect(screen.getByRole("status")).toHaveTextContent("Not listening");
  });

  it("says nothing about models once they are installed", async () => {
    serveVault();
    onCommand("model_status", () => ({
      ready: true,
      bytesRequired: 0,
      bytesPresent: 0,
      sets: [],
      downloading: false,
      modelsDir: "C:\\app\\.models",
    }));
    renderShell();

    expect(await screen.findByRole("status")).toHaveTextContent("Not listening");
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.getByRole("status")).not.toHaveTextContent("Transcription not ready");
  });

  it("teaches the capture chord on an idle pill with nothing else to say", async () => {
    // The one surface in the main window that names the hotkey, so a first-run
    // user meets it on the indicator the hotkey operates rather than only in a
    // Settings row they have no reason to open.
    serveVault();
    onCommand("model_status", () => ({
      ready: true,
      bytesRequired: 0,
      bytesPresent: 0,
      sets: [],
      downloading: false,
      modelsDir: "C:\\app\\.models",
    }));
    renderShell();

    await waitFor(() => {
      expect(screen.getByRole("status")).toHaveTextContent("Ctrl+Shift+K starts a capture");
    });
    expect(screen.getByRole("status")).toHaveTextContent("Not listening");
  });

  it("drops the chord hint once the capture it teaches is running", async () => {
    // Teaching outranks nothing, but only while the teaching is true: the hint
    // is withheld on every engaged pill, and a recording one is the plainest
    // case (its detail slot closes too).
    serveVault();
    onCommand("model_status", () => ({
      ready: true,
      bytesRequired: 0,
      bytesPresent: 0,
      sets: [],
      downloading: false,
      modelsDir: "C:\\app\\.models",
    }));
    renderShell();
    await waitFor(() => {
      expect(screen.getByRole("status")).toHaveTextContent("Ctrl+Shift+K starts a capture");
    });

    act(() => {
      emitFromBackend(CAPTURE_STATE_EVENT, {
        phase: "listening",
        sources: { loopback: "live", microphone: "live" },
      });
    });

    expect(screen.getByRole("status")).not.toHaveTextContent("Ctrl+Shift+K");
  });

  it("drops the chord hint on a capture that is engaged but recording nothing", async () => {
    // The case the pill's own live/not-live test cannot carry: `degraded` with
    // no source live is engaged and reaching disk with nothing, so the detail
    // slot stays open, and it can stay open for as long as the devices are
    // gone. A press there stops that capture (the tray says "Stop capture" for
    // every phase but idle), so the slot must not offer to start one. Same
    // reasoning covers the `starting` window.
    serveVault();
    onCommand("model_status", () => ({
      ready: true,
      bytesRequired: 0,
      bytesPresent: 0,
      sets: [],
      downloading: false,
      modelsDir: "C:\\app\\.models",
    }));
    renderShell();
    await waitFor(() => {
      expect(screen.getByRole("status")).toHaveTextContent("Ctrl+Shift+K starts a capture");
    });

    act(() => {
      emitFromBackend(CAPTURE_STATE_EVENT, {
        phase: "degraded",
        sources: { loopback: "stalled", microphone: "stalled" },
      });
    });

    // Gone on the live state, before the headline's debounce has caught up...
    expect(screen.getByRole("status")).not.toHaveTextContent("Ctrl+Shift+K");
    // ...and still gone once it has, which is the reading that would persist.
    await waitFor(
      () => {
        expect(screen.getByRole("status")).toHaveTextContent("Reconnecting");
      },
      { timeout: 2000 },
    );
    expect(screen.getByRole("status")).not.toHaveTextContent("Ctrl+Shift+K");
  });

  it("keeps a real problem ahead of the chord hint", async () => {
    // The detail slot holds one line, so the chain has to rank: something wrong
    // always outranks something to learn.
    serveVault();
    onCommand("model_status", () => ({
      ready: true,
      bytesRequired: 0,
      bytesPresent: 0,
      sets: [],
      downloading: false,
      modelsDir: "C:\\app\\.models",
    }));
    renderShell();
    expect(await screen.findByRole("status")).toHaveTextContent("Not listening");

    act(() => {
      emitFromBackend(CAPTURE_STATE_EVENT, {
        phase: "idle",
        sources: { loopback: "failed", microphone: "failed" },
      });
    });

    await waitFor(
      () => {
        expect(screen.getByRole("status")).toHaveTextContent("Capture failed to start");
      },
      { timeout: 2000 },
    );
    expect(screen.getByRole("status")).not.toHaveTextContent("Ctrl+Shift+K");
  });

  it("minimizes, and closes to the tray", async () => {
    const user = userEvent.setup();
    serveVault();
    renderShell();

    await user.click(screen.getByRole("button", { name: "Minimize" }));
    expect(minimize).toHaveBeenCalledTimes(1);
    expect(close).not.toHaveBeenCalled();

    // Close is hide-to-tray: lib.rs prevents the close and hides the window, so
    // this button ends up exactly where the native one did.
    await user.click(screen.getByRole("button", { name: "Close" }));
    expect(close).toHaveBeenCalledTimes(1);
  });

  it("toggles maximize and shows Restore once the window is maximized", async () => {
    const user = userEvent.setup();
    serveVault();
    renderShell();

    const maximize = await screen.findByRole("button", { name: "Maximize" });
    expect(screen.queryByRole("button", { name: "Restore" })).not.toBeInTheDocument();

    await user.click(maximize);
    expect(toggleMaximize).toHaveBeenCalledTimes(1);

    // The glyph does not follow the click, it follows the window: this is the
    // resize path, which is also how Win+Up and a drag to the top edge arrive.
    act(() => {
      setMaximized(true);
    });
    expect(await screen.findByRole("button", { name: "Restore" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Maximize" })).not.toBeInTheDocument();

    act(() => {
      setMaximized(false);
    });
    expect(await screen.findByRole("button", { name: "Maximize" })).toBeInTheDocument();
  });

  it("reads a window that was already maximized on mount", async () => {
    // The seed, not the subscription: nothing is listening yet when this runs,
    // so only the mount-time read can produce a Restore button.
    serveVault();
    setMaximized(true);

    renderShell();

    expect(await screen.findByRole("button", { name: "Restore" })).toBeInTheDocument();
  });

  it("drags by the bar itself, never by a control on it", async () => {
    serveVault();
    renderShell();

    // The view behind the bar renders a header of its own, so this reaches the
    // bar through something only the bar has.
    const bar = screen.getByRole("button", { name: "kodabi" }).closest("header");
    expect(bar).toHaveAttribute("data-tauri-drag-region");
    // Bare, not `deep`: `deep` would drag from anywhere in the subtree, which
    // is every button in the bar.
    expect(bar).not.toHaveAttribute("data-tauri-drag-region", "deep");

    for (const name of ["kodabi", "Commands", "Settings", "Minimize", "Maximize", "Close"]) {
      expect(screen.getByRole("button", { name: new RegExp(`^${name}`) })).not.toHaveAttribute(
        "data-tauri-drag-region",
      );
    }
    expect(screen.getByRole("navigation", { name: "App" })).not.toHaveAttribute(
      "data-tauri-drag-region",
    );
  });

  it("stops listening for resizes when it goes away", async () => {
    serveVault();
    const { unmount } = renderShell();
    await waitFor(() => {
      expect(resizeListenerCount()).toBe(1);
    });

    unmount();

    expect(resizeListenerCount()).toBe(0);
  });
});
