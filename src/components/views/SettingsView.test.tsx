import { render, screen, within } from "@testing-library/react";
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
  mic_check: null,
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

/** The two pill toggles. Each one's accessible name IS its visible row label —
 * exact strings rather than a loose regex, because that parity is the thing the
 * rename bought: the "Capture pill" group supplies the context the old
 * "Show the capture pill during captures you start" had to carry alone. */
function manualToggle(): HTMLElement {
  return screen.getByRole("switch", { name: "Captures you start" });
}

function autoToggle(): HTMLElement {
  return screen.getByRole("switch", { name: "Auto detected captures" });
}

/** The pill cluster's `role="group"`, which is where its rows and its one error
 * slot live. */
function pillGroup(): HTMLElement {
  return screen.getByRole("group", { name: "Capture pill" });
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

    // Inside the group, not merely somewhere on the page. It used to render at
    // the foot of the tab, below Echo check and Search index, where it read as
    // THEIR failure.
    expect(await within(pillGroup()).findByRole("alert")).toHaveTextContent(
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

  it("reads the two toggles as one concept: a named group with a heading", async () => {
    await renderSeeded(DEFAULTS, "Capture");

    // The relationship is in the accessibility tree, not only in the indent.
    expect(within(pillGroup()).getAllByRole("switch")).toHaveLength(2);
    // "Capture pill" is a concept and owns no control of its own, so the group
    // draws it as a heading rather than borrowing one of its two rows — which is
    // also what stops the indent claiming one toggle depends on the other.
    expect(within(pillGroup()).getByText("Capture pill")).toBeInTheDocument();
  });

  it("gives each toggle the same accessible name as the row you can see", async () => {
    await renderSeeded(DEFAULTS, "Capture");

    // These used to diverge: the visible row said "Capture pill" while the
    // switch answered to "Show the capture pill during captures you start", so
    // what someone reads was not what they could say to a voice-control tool.
    for (const label of ["Captures you start", "Auto detected captures"]) {
      expect(screen.getByRole("switch", { name: label })).toBeInTheDocument();
      expect(screen.getByText(label)).toBeInTheDocument();
    }
  });
});

describe("SettingsView retention", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("nests the day count under the policy it depends on", async () => {
    await renderSeeded({ ...DEFAULTS, retention: { policy: "keep_days", days: 30 } });

    // The policy owns a control, so its row heads the group and the group draws
    // no heading of its own. That is what licenses the short child label: the
    // group's name carries the rest, so "Days" and not "Days to keep".
    const group = screen.getByRole("group", { name: "Retention" });
    expect(within(group).getByRole("combobox", { name: /Retention/ })).toBeInTheDocument();
    expect(within(group).getByRole("spinbutton", { name: "Days" })).toHaveValue(30);
  });

  it("has no day count while the policy does not keep days", async () => {
    await renderSeeded();

    expect(screen.queryByRole("spinbutton", { name: "Days" })).not.toBeInTheDocument();
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

describe("SettingsView mic test", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  function runTestButton(): HTMLElement {
    return screen.getByRole("button", { name: "Run test" });
  }

  it("shows no result line until a test has ever run", async () => {
    await renderSeeded(DEFAULTS, "Capture");

    expect(screen.queryByText(/Checked/)).not.toBeInTheDocument();
  });

  it("reports headphones and adopts the echoed settings", async () => {
    const user = userEvent.setup();
    const stored: Settings = {
      ...DEFAULTS,
      mic_check: { outcome: "headphones", measured_at: "2026-07-22T00:48:18Z" },
    };
    onCommand("run_mic_test", () => stored);
    await renderSeeded(DEFAULTS, "Capture");

    await user.click(runTestButton());

    expect(invoke).toHaveBeenCalledWith("run_mic_test");
    expect(
      await screen.findByText(/Headphones detected\. Your microphone and speaker channels stay separate\./),
    ).toBeInTheDocument();
  });

  it("reports speakers with the measured echo and delay", async () => {
    const user = userEvent.setup();
    const stored: Settings = {
      ...DEFAULTS,
      mic_check: {
        outcome: "speakers",
        echo_db: 12.5,
        delay_ms: 85,
        measured_at: "2026-07-22T00:48:18Z",
      },
    };
    onCommand("run_mic_test", () => stored);
    await renderSeeded(DEFAULTS, "Capture");

    await user.click(runTestButton());

    expect(
      await screen.findByText(/Speakers detected\. Your microphone hears them \(about 12\.5 dB, 85 ms delay\)\./),
    ).toBeInTheDocument();
  });

  it("reports mic silent", async () => {
    const user = userEvent.setup();
    const stored: Settings = {
      ...DEFAULTS,
      mic_check: { outcome: "mic_silent", measured_at: "2026-07-22T00:48:18Z" },
    };
    onCommand("run_mic_test", () => stored);
    await renderSeeded(DEFAULTS, "Capture");

    await user.click(runTestButton());

    expect(
      await screen.findByText(/No signal from your microphone\./),
    ).toBeInTheDocument();
  });

  it("surfaces a failure without discarding a previously stored result", async () => {
    const user = userEvent.setup();
    const seeded: Settings = {
      ...DEFAULTS,
      mic_check: { outcome: "headphones", measured_at: "2026-07-22T00:48:18Z" },
    };
    onCommand("run_mic_test", () => {
      throw "stop the current capture before running the mic test";
    });
    await renderSeeded(seeded, "Capture");

    await user.click(runTestButton());

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "stop the current capture before running the mic test",
    );
    // The last real result is still what's shown underneath the error.
    expect(screen.getByText(/Headphones detected/)).toBeInTheDocument();
  });
});
