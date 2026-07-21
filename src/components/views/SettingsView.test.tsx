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
  appearance: { theme: "system" },
};

function settingsWith(overlay: OverlaySettings): Settings {
  return { ...DEFAULTS, overlay };
}

/**
 * Render with `get_settings` seeded, wait for the load to land, and open the
 * tab the assertions need. The tabs FILTER, so a control that is not in the
 * active category is genuinely not rendered — reaching one is a click, exactly
 * as it is for the user.
 */
async function renderSeeded(
  settings: Settings = DEFAULTS,
  tab: "Privacy" | "Appearance" | "Capture" = "Privacy",
) {
  onCommand("get_settings", () => settings);
  const result = render(<SettingsView />);
  await screen.findByRole("tab", { name: "Privacy" });
  if (tab !== "Privacy") {
    await userEvent.setup().click(screen.getByRole("tab", { name: tab }));
  }
  return result;
}

function manualToggle(): HTMLElement {
  return screen.getByRole("switch", { name: /during captures you start/i });
}

function autoToggle(): HTMLElement {
  return screen.getByRole("switch", { name: /auto detected captures/i });
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
      "Capture",
    );

    expect(manualToggle()).toBeChecked();
    expect(autoToggle()).not.toBeChecked();
  });

  it("shows the shipped defaults: manual off, auto detected on", async () => {
    await renderSeeded(DEFAULTS, "Capture");

    expect(manualToggle()).not.toBeChecked();
    expect(autoToggle()).toBeChecked();
  });

  it("sends both flags when one is toggled, and adopts the echoed result", async () => {
    const user = userEvent.setup();
    await renderSeeded(DEFAULTS, "Capture");
    const saved = settingsWith({ manual_captures: true, auto_captures: true });
    onCommand("set_capture_overlay", () => saved);

    await user.click(manualToggle());

    // The command takes the whole struct, so the untouched flag must ride along
    // at its stored value rather than defaulting.
    expect(lastOverlayArg()).toEqual({
      manual_captures: true,
      auto_captures: true,
    });
    // Non-optimistic: the switch reflects what the backend echoed back.
    expect(manualToggle()).toBeChecked();
  });

  it("toggles the dormant auto-detected flag independently", async () => {
    const user = userEvent.setup();
    await renderSeeded(DEFAULTS, "Capture");
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
    await renderSeeded(DEFAULTS, "Capture");
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
    await renderSeeded(DEFAULTS, "Capture");

    // The setting is user-changeable now but has nothing to act on, and the
    // copy has to say so rather than implying Kodabi is watching for meetings.
    expect(
      screen.getByText(/does not detect meetings on its own yet/i),
    ).toBeInTheDocument();
  });
});

describe("SettingsView tabs", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("names itself Settings and offers the three categories as tabs", async () => {
    // Horizontal tabs, not a vertical list: a second column of destinations
    // beside the sidebar reads as navigation, and these filter the page.
    await renderSeeded();

    expect(
      screen.getByRole("heading", { name: "Settings", level: 2 }),
    ).toBeInTheDocument();
    for (const category of ["Privacy", "Appearance", "Capture"]) {
      expect(screen.getByRole("tab", { name: category })).toBeInTheDocument();
    }
  });

  it("filters: only the active category's settings are on the page", async () => {
    const user = userEvent.setup();
    await renderSeeded();

    // Privacy is the landing tab.
    expect(screen.getByText("Recording consent")).toBeInTheDocument();
    expect(screen.queryByText("Reduce motion")).not.toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: "Appearance" }));

    expect(screen.getByText("Reduce motion")).toBeInTheDocument();
    expect(screen.queryByText("Recording consent")).not.toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Appearance" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  it("shows a read-only value as plain text, with no control to press", async () => {
    // "You can change this" and "this is how it is" are told apart by shape:
    // an editable value is a chip with a chevron, a stated one is not.
    await renderSeeded();

    expect(screen.getByText("Acknowledged")).toBeInTheDocument();
    expect(
      screen.queryByRole("combobox", { name: /Recording consent/ }),
    ).not.toBeInTheDocument();
  });
});

describe("SettingsView appearance", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("reflects the stored theme", async () => {
    await renderSeeded({ ...DEFAULTS, appearance: { theme: "dark" } }, "Appearance");

    expect(screen.getByRole("combobox", { name: /Theme/ })).toHaveTextContent("Dark");
  });

  it("persists a chosen theme and adopts the echoed result", async () => {
    const user = userEvent.setup();
    const stored: Settings = { ...DEFAULTS, appearance: { theme: "dark" } };
    onCommand("set_appearance", () => stored);
    await renderSeeded(DEFAULTS, "Appearance");

    await user.click(screen.getByRole("combobox", { name: /Theme/ }));
    await user.click(screen.getByRole("option", { name: "Dark" }));

    expect(invoke).toHaveBeenCalledWith("set_appearance", {
      appearance: { theme: "dark" },
    });
    expect(screen.getByRole("combobox", { name: /Theme/ })).toHaveTextContent("Dark");
  });

  it("keeps the stored theme showing when the save fails", async () => {
    const user = userEvent.setup();
    onCommand("set_appearance", () => {
      throw "the settings file is read only";
    });
    await renderSeeded(DEFAULTS, "Appearance");

    await user.click(screen.getByRole("combobox", { name: /Theme/ }));
    await user.click(screen.getByRole("option", { name: "Dark" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      /the settings file is read only/,
    );
    // Not flipped optimistically: the control still shows what is on disk.
    expect(screen.getByRole("combobox", { name: /Theme/ })).toHaveTextContent(
      "Match the system",
    );
  });

  it("applies reduce motion to the document, so every window holds still", async () => {
    // The OS setting is honoured app-wide already; this is the in-app override
    // for people who do not want to change a system-wide preference. It has to
    // reach the <html> element or the CSS floor never fires.
    const user = userEvent.setup();
    await renderSeeded(DEFAULTS, "Appearance");

    await user.click(screen.getByRole("switch", { name: "Reduce motion" }));

    expect(document.documentElement).toHaveAttribute("data-reduce-motion", "on");

    await user.click(screen.getByRole("switch", { name: "Reduce motion" }));

    expect(document.documentElement).not.toHaveAttribute("data-reduce-motion");
  });
});
