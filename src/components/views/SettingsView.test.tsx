import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SettingsView } from "./SettingsView";
import type { OverlaySettings, Settings } from "../../useSettings";
import { INDEX_STATE_EVENT } from "../../events";
import { emitFromBackend, invoke, onCommand, resetTauriMocks } from "../../test/tauri";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

/**
 * The two display preferences write to localStorage and to <html>, and neither
 * is torn down for us: jsdom is per FILE, and `src/test/setup.ts` resets only
 * the RTL tree. Left alone, a toggle flipped in one test is still flipped in
 * the next, and the next Appearance render seeds its switch from it — so a
 * later toggle-twice test would run backwards and assert the opposite of what
 * it reads. The reduce-motion test below stayed honest only by ending clean;
 * this makes that a property of the file rather than of each test.
 */
afterEach(() => {
  const root = document.documentElement;
  for (const key of ["kodabi:reduce-motion", "kodabi:contrast"]) {
    window.localStorage.removeItem(key);
  }
  root.removeAttribute("data-reduce-motion");
  root.removeAttribute("data-contrast");
});

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
 * Render with `get_settings` seeded and wait for the load to land.
 *
 * There is no tab to open any more: the four cards are all on the page at once,
 * so every control is reachable the moment the settings arrive. The wait is on
 * a card rather than the view's own heading, because the heading renders before
 * `get_settings` resolves and would let an assertion run against an empty page.
 */
async function renderSeeded(settings: Settings = DEFAULTS) {
  onCommand("get_settings", () => settings);
  const result = render(<SettingsView />);
  await screen.findByRole("region", { name: "Privacy" });
  return result;
}

/** A card, by the name its `<section aria-label>` gives it. Each one is a real
 * region landmark, which is what replaced the `role="group"` clusters: the
 * hierarchy on this screen is card-then-rows, with no indent below that. */
function card(name: string): HTMLElement {
  return screen.getByRole("region", { name });
}

/** The two pill switches. Each one's accessible name IS its visible row label —
 * exact strings rather than a loose regex, because that parity is what let the
 * "Capture pill" group go: the labels now carry their own context instead of
 * borrowing it from a heading above them. */
function manualToggle(): HTMLElement {
  return screen.getByRole("switch", { name: "Pill for captures you start" });
}

function autoToggle(): HTMLElement {
  return screen.getByRole("switch", { name: "Pill for auto detected captures" });
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
    await renderSeeded(settingsWith({ manual_captures: true, auto_captures: false }));

    expect(manualToggle()).toBeChecked();
    expect(autoToggle()).not.toBeChecked();
  });

  it("shows the shipped defaults: manual off, auto detected on", async () => {
    await renderSeeded(DEFAULTS);

    expect(manualToggle()).not.toBeChecked();
    expect(autoToggle()).toBeChecked();
  });

  it("sends both flags when one is toggled, and adopts the echoed result", async () => {
    const user = userEvent.setup();
    await renderSeeded(DEFAULTS);
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
    await renderSeeded(DEFAULTS);
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
    await renderSeeded(DEFAULTS);
    onCommand("set_capture_overlay", () => {
      throw "settings.toml is read only";
    });

    await user.click(manualToggle());

    // Inside the Capture card, not merely somewhere on the page. It used to
    // render at the foot of the tab, below Echo check and Search index, where
    // it read as THEIR failure.
    expect(await within(card("Capture")).findByRole("alert")).toHaveTextContent(
      "settings.toml is read only",
    );
    // Nothing was persisted, so the control must not imply otherwise.
    expect(manualToggle()).not.toBeChecked();
  });

  it("explains that auto detection does not exist yet", async () => {
    await renderSeeded(DEFAULTS);

    // The setting is user-changeable now but has nothing to act on, and the
    // copy has to say so rather than implying Kodabi is watching for meetings.
    expect(
      screen.getByText(/does not detect meetings on its own yet/i),
    ).toBeInTheDocument();
  });

  it("keeps both pill switches in the Capture card, with no group between them", async () => {
    await renderSeeded(DEFAULTS);

    // Two switches, in the card that names the concern. The "Capture pill"
    // group they used to share is gone on purpose: it existed to supply context
    // the short labels lacked, and the labels carry it themselves now, so the
    // indent that came with it claimed a dependency that was never there.
    expect(within(card("Capture")).getAllByRole("switch")).toHaveLength(2);
    expect(screen.queryByRole("group", { name: "Capture pill" })).not.toBeInTheDocument();
  });

  it("gives each toggle the same accessible name as the row you can see", async () => {
    await renderSeeded(DEFAULTS);

    // These used to diverge: the visible row said "Capture pill" while the
    // switch answered to "Show the capture pill during captures you start", so
    // what someone reads was not what they could say to a voice-control tool.
    // The labels are self-naming now, which is what carries that parity without
    // a heading standing over them.
    for (const label of ["Pill for captures you start", "Pill for auto detected captures"]) {
      expect(screen.getByRole("switch", { name: label })).toBeInTheDocument();
      expect(screen.getByText(label)).toBeInTheDocument();
    }
  });
});

describe("SettingsView retention", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("sits the day count inline beside the policy it depends on", async () => {
    await renderSeeded({ ...DEFAULTS, retention: { policy: "keep_days", days: 30 } });

    // Both controls in one row of the Privacy card, rather than the field being
    // indented into a row of its own under a `role="group"`. Sitting beside the
    // control that summoned it is the dependency claim now, and it is also what
    // licenses the short label: the policy is a word away, so "Days" and not
    // "Days to keep".
    const privacy = card("Privacy");
    expect(within(privacy).getByRole("combobox", { name: /Retention/ })).toBeInTheDocument();
    expect(within(privacy).getByRole("spinbutton", { name: "Days" })).toHaveValue(30);
  });

  it("has no day count while the policy does not keep days", async () => {
    await renderSeeded();

    expect(screen.queryByRole("spinbutton", { name: "Days" })).not.toBeInTheDocument();
  });

  it("acknowledges the day field's own commit", async () => {
    const user = userEvent.setup();
    const stored: Settings = { ...DEFAULTS, retention: { policy: "keep_days", days: 45 } };
    onCommand("set_retention_policy", () => stored);
    await renderSeeded({ ...DEFAULTS, retention: { policy: "keep_days", days: 30 } });

    const field = screen.getByRole("spinbutton", { name: "Days" });
    await user.clear(field);
    await user.type(field, "45{Enter}");

    expect(invoke).toHaveBeenCalledWith("set_retention_policy", {
      policy: { policy: "keep_days", days: 45 },
    });
    expect(await screen.findByRole("status")).toHaveTextContent("Saved.");
  });

  it("does not acknowledge the day field when it was the policy that changed", async () => {
    // "Saved." reports the day field's commit, so only the day field may raise
    // it. Choosing a policy used to print it without the field having been
    // touched. The Select is committed-on-select, so the value it now shows is
    // its confirmation, exactly as Theme's is.
    const user = userEvent.setup();
    const stored: Settings = { ...DEFAULTS, retention: { policy: "keep_days", days: 30 } };
    onCommand("set_retention_policy", () => stored);
    await renderSeeded();

    await user.click(screen.getByRole("combobox", { name: /Retention/ }));
    await user.click(screen.getByRole("option", { name: "Keep for a number of days" }));

    // The policy did persist, and the day row it reveals is the visible result.
    expect(invoke).toHaveBeenCalledWith("set_retention_policy", {
      policy: { policy: "keep_days", days: 30 },
    });
    expect(await screen.findByRole("spinbutton", { name: "Days" })).toBeInTheDocument();
    expect(screen.queryByText("Saved.")).not.toBeInTheDocument();
  });
});

describe("SettingsView layout", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("names itself Settings and lays the four concerns out as cards", async () => {
    await renderSeeded();

    expect(
      screen.getByRole("heading", { name: "Settings", level: 2 }),
    ).toBeInTheDocument();
    // Named regions, so the four concerns are landmarks a screen reader can
    // jump between rather than tabs a pointer has to find.
    for (const concern of ["Privacy", "Appearance", "Capture", "Maintenance"]) {
      expect(screen.getByRole("region", { name: concern })).toBeInTheDocument();
    }
    // The rail is gone with them: it filtered the page, so three quarters of
    // the settings were somewhere you had to remember to look.
    expect(screen.queryAllByRole("tab")).toHaveLength(0);
  });

  it("puts every setting on the page at once, with nothing behind a filter", async () => {
    await renderSeeded();

    // The pairs the tab rail used to keep apart: a Privacy row and an
    // Appearance row, both readable without a click.
    expect(screen.getByText("Recording consent")).toBeInTheDocument();
    expect(screen.getByText("Reduce motion")).toBeInTheDocument();
    expect(screen.getByText("Echo check")).toBeInTheDocument();
    expect(screen.getByText("Search index")).toBeInTheDocument();
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
    await renderSeeded({ ...DEFAULTS, appearance: { theme: "dark" } });

    expect(screen.getByRole("combobox", { name: /Theme/ })).toHaveTextContent("Dark");
  });

  it("persists a chosen theme and adopts the echoed result", async () => {
    const user = userEvent.setup();
    const stored: Settings = { ...DEFAULTS, appearance: { theme: "dark" } };
    onCommand("set_appearance", () => stored);
    await renderSeeded(DEFAULTS);

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
    await renderSeeded(DEFAULTS);

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
    await renderSeeded(DEFAULTS);

    await user.click(screen.getByRole("switch", { name: "Reduce motion" }));

    expect(document.documentElement).toHaveAttribute("data-reduce-motion", "on");

    await user.click(screen.getByRole("switch", { name: "Reduce motion" }));

    expect(document.documentElement).not.toHaveAttribute("data-reduce-motion");
  });

  it("applies more contrast to the document, so every window sharpens", async () => {
    // Same shape as reduce motion, and it carries more weight: on Windows the
    // OS query is reached only through a Contrast theme, which takes the
    // palette over wholesale, so this override is the branch that actually
    // delivers the high-contrast palette (src/contrast.ts).
    const user = userEvent.setup();
    await renderSeeded(DEFAULTS);

    await user.click(screen.getByRole("switch", { name: "Increase contrast" }));

    // The `.hc` class on :root, which is what the Grove token remap keys off.
    expect(document.documentElement).toHaveClass("hc");

    await user.click(screen.getByRole("switch", { name: "Increase contrast" }));

    expect(document.documentElement).not.toHaveClass("hc");
  });

  it("shows a stored display preference without an effect", async () => {
    // Seeded from storage during render (src/contrast.ts), which is what lets
    // the preference be a plain synchronous read rather than a bridge hook.
    window.localStorage.setItem("kodabi:contrast", "more");

    await renderSeeded(DEFAULTS);

    expect(screen.getByRole("switch", { name: "Increase contrast" })).toBeChecked();
  });
});

describe("SettingsView search index", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("reports what the rebuild event says, not what the click hoped", async () => {
    // The command returns as soon as the job is QUEUED, so the outcome only
    // ever arrives on `index:state`. Emitting it is the whole test: a control
    // that reported success on the click would be lying about a job still
    // running.
    const user = userEvent.setup();
    onCommand("rebuild_index", () => null);
    await renderSeeded();

    await user.click(screen.getByRole("button", { name: "Rebuild" }));
    expect(invoke).toHaveBeenCalledWith("rebuild_index");

    act(() => emitFromBackend(INDEX_STATE_EVENT, { status: "ready", notes: 3 }));

    expect(await screen.findByRole("status")).toHaveTextContent(
      "Index rebuilt. 3 notes indexed.",
    );
  });

  it("surfaces a command-level failure, which emits no event of its own", async () => {
    const user = userEvent.setup();
    onCommand("rebuild_index", () => {
      throw "the index is unavailable this session";
    });
    await renderSeeded();

    await user.click(screen.getByRole("button", { name: "Rebuild" }));

    expect(await within(card("Maintenance")).findByRole("alert")).toHaveTextContent(
      "the index is unavailable this session",
    );
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
    await renderSeeded(DEFAULTS);

    expect(screen.queryByText(/Checked/)).not.toBeInTheDocument();
  });

  it("reports headphones and adopts the echoed settings", async () => {
    const user = userEvent.setup();
    const stored: Settings = {
      ...DEFAULTS,
      mic_check: { outcome: "headphones", measured_at: "2026-07-22T00:48:18Z" },
    };
    onCommand("run_mic_test", () => stored);
    await renderSeeded(DEFAULTS);

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
    await renderSeeded(DEFAULTS);

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
    await renderSeeded(DEFAULTS);

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
    await renderSeeded(seeded);

    await user.click(runTestButton());

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "stop the current capture before running the mic test",
    );
    // The last real result is still what's shown underneath the error.
    expect(screen.getByText(/Headphones detected/)).toBeInTheDocument();
  });
});
