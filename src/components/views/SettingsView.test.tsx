import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SettingsView } from "./SettingsView";
import type { OverlaySettings, Settings } from "../../useSettings";
import { invoke, onCommand, resetTauriMocks } from "../../test/tauri";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

/** The shipped defaults: no pill for captures you start, one for auto-detected
 * captures (dormant until detection exists). */
const DEFAULTS: Settings = {
  consent_acknowledged: true,
  retention: { policy: "keep_all" },
  overlay: { manual_captures: false, auto_captures: true },
};

function settingsWith(overlay: OverlaySettings): Settings {
  return { ...DEFAULTS, overlay };
}

/** Render with `get_settings` seeded, and wait for the load to land. */
async function renderSeeded(settings: Settings = DEFAULTS) {
  onCommand("get_settings", () => settings);
  const result = render(<SettingsView />);
  await screen.findByTestId("overlay-manual-captures");
  return result;
}

function manualToggle(): HTMLInputElement {
  return screen.getByTestId("overlay-manual-captures");
}

function autoToggle(): HTMLInputElement {
  return screen.getByTestId("overlay-auto-captures");
}

/** The `overlay` argument of the last `set_capture_overlay` call. */
function lastOverlayArg(): OverlaySettings {
  const calls = invoke.mock.calls.filter(
    ([command]) => command === "set_capture_overlay",
  );
  const call = calls[calls.length - 1];
  return (call[1] as { overlay: OverlaySettings }).overlay;
}

describe("SettingsView capture overlay", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("reflects the stored flags", async () => {
    await renderSeeded(
      settingsWith({ manual_captures: true, auto_captures: false }),
    );

    expect(manualToggle()).toBeChecked();
    expect(autoToggle()).not.toBeChecked();
  });

  it("shows the shipped defaults: manual off, auto detected on", async () => {
    await renderSeeded();

    expect(manualToggle()).not.toBeChecked();
    expect(autoToggle()).toBeChecked();
  });

  it("sends both flags when one is toggled, and adopts the echoed result", async () => {
    const user = userEvent.setup();
    await renderSeeded();
    const saved = settingsWith({ manual_captures: true, auto_captures: true });
    onCommand("set_capture_overlay", () => saved);

    await user.click(manualToggle());

    // The command takes the whole struct, so the untouched flag must ride along
    // at its stored value rather than defaulting.
    expect(lastOverlayArg()).toEqual({
      manual_captures: true,
      auto_captures: true,
    });
    // Non-optimistic: the checkbox reflects what the backend echoed back.
    expect(manualToggle()).toBeChecked();
  });

  it("toggles the dormant auto-detected flag independently", async () => {
    const user = userEvent.setup();
    await renderSeeded();
    onCommand("set_capture_overlay", () =>
      settingsWith({ manual_captures: false, auto_captures: false }),
    );

    await user.click(autoToggle());

    expect(lastOverlayArg()).toEqual({
      manual_captures: false,
      auto_captures: false,
    });
    expect(autoToggle()).not.toBeChecked();
  });

  it("surfaces a failed save and leaves the toggle showing stored truth", async () => {
    const user = userEvent.setup();
    await renderSeeded();
    onCommand("set_capture_overlay", () => {
      throw "settings.toml is read only";
    });

    await user.click(manualToggle());

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "settings.toml is read only",
    );
    // Nothing was persisted, so the control must not imply otherwise.
    expect(manualToggle()).not.toBeChecked();
  });

  it("explains that auto detection does not exist yet", async () => {
    await renderSeeded();

    // The setting is user-changeable now but has nothing to act on, and the
    // copy has to say so rather than implying Kodabi is watching for meetings.
    expect(
      screen.getByText(/does not detect meetings on its own yet/i),
    ).toBeInTheDocument();
  });
});
