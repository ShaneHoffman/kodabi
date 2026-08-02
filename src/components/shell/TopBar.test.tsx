import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CapturePipelineProvider } from "../providers/CapturePipelineProvider";
import { NavigationProvider } from "../providers/NavigationProvider";
import { MainContent } from "./MainContent";
import { TopBar } from "./TopBar";
import { emitFromBackend, onCommand, resetTauriMocks } from "../../test/tauri";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

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
      <CapturePipelineProvider>
        <TopBar onOpenPalette={onOpenPalette} />
        <MainContent />
      </CapturePipelineProvider>
    </NavigationProvider>,
  );
}

describe("TopBar", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("opens the command palette and names its shortcut", async () => {
    const user = userEvent.setup();
    const onOpenPalette = vi.fn();
    serveVault();

    renderShell(onOpenPalette);

    const commands = screen.getByRole("button", { name: /Commands/ });
    // The hint is part of the control's own name, so a screen reader hears the
    // key without having to find a separate node.
    expect(commands).toHaveTextContent("Ctrl K");

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
    // Idle is not silence: the pill says so rather than disappearing, which is
    // what makes "is it recording" answerable in one place.
    expect(await screen.findByRole("status")).toHaveTextContent("Idle");

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
});
