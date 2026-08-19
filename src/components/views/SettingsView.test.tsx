import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SettingsView } from "./SettingsView";
import { CAPTURE_TOGGLE_SHORTCUT, type ShortcutStatus } from "../../captureControl";
import { QUICK_CAPTURE_SHORTCUT } from "../../quickCapture";
import { ModelStatusProvider } from "../providers/ModelStatusProvider";
import { NavigationProvider } from "../providers/NavigationProvider";
import { NavigationContext } from "../../useNavigation";
import { UpdaterProvider } from "../providers/UpdaterProvider";
import type { OverlaySettings, Settings } from "../../useSettings";
import { INDEX_STATE_EVENT } from "../../events";
import {
  emitFromBackend,
  invoke,
  invokedCommands,
  onCommand,
  resetTauriMocks,
} from "../../test/tauri";
import {
  failCheck,
  resetUpdaterMocks,
  setAppVersion,
  setAvailableUpdate,
} from "../../test/updater";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));
vi.mock("@tauri-apps/plugin-updater", () => import("../../test/updater"));
vi.mock("@tauri-apps/plugin-process", () => import("../../test/updater"));
vi.mock("@tauri-apps/api/app", () => import("../../test/updater"));

/**
 * The two display preferences write to localStorage and to <html>, and neither
 * is torn down for us: jsdom is per FILE, and `src/test/setup.ts` resets only
 * the RTL tree. Left alone, a toggle flipped in one test is still flipped in
 * the next, and the next Appearance render seeds its switch from it — so a
 * later toggle-twice test would run backwards and assert the opposite of what
 * it reads. The reduce-motion test below stayed honest only by ending clean;
 * this makes that a property of the file rather than of each test.
 */
/** The updater mocks are a per-file singleton like the Tauri ones, and every
 * render mounts `UpdaterProvider`. Reset here rather than in each describe's
 * own `beforeEach`, since a staged update leaking into an unrelated capture
 * test would put a live notice on the page it is asserting against. */
beforeEach(() => {
  resetUpdaterMocks();
});

afterEach(() => {
  const root = document.documentElement;
  for (const key of ["kodabi:reduce-motion", "kodabi:contrast"]) {
    window.localStorage.removeItem(key);
  }
  root.removeAttribute("data-reduce-motion");
  root.classList.remove("hc");
});

/** The shipped defaults: no pill for captures you start, one for auto-detected
 * captures (dormant until detection exists). */
const DEFAULTS: Settings = {
  consent_acknowledged: true,
  retention: { policy: "keep_all" },
  overlay: { manual_captures: false, auto_captures: true },
  appearance: { theme: "system" },
  mic_check: null,
  ledger: { aging_after_days: 14, stale_after_days: 30, conversation_autoclose: 0.8 },
  categories: {
    standup: { enrollment_default: null },
    one_on_one: { enrollment_default: null },
    client: { enrollment_default: null },
    working_session: { enrollment_default: null },
    review: { enrollment_default: null },
    all_hands: { enrollment_default: null },
    observer: { enrollment_default: null },
  },
};

function settingsWith(overlay: OverlaySettings): Settings {
  return { ...DEFAULTS, overlay };
}

/** Both shortcuts bound, which is what a machine with no clash reports. */
const SHORTCUTS_BOUND: ShortcutStatus = { captureToggle: true, quickCapture: true };

/**
 * Render with `get_settings` seeded and wait for the load to land.
 *
 * There is no tab to open any more: the cards are all on the page at once,
 * so every control is reachable the moment the settings arrive. The wait is on
 * a card rather than the view's own heading, because the heading renders before
 * `get_settings` resolves and would let an assertion run against an empty page.
 *
 * `shortcut_status` is seeded to the everything-worked answer rather than left
 * unrouted: the hook swallows a rejection either way, so both keep the rest of
 * this file green, but an unrouted command would leave every test asserting
 * against a status the real app never shows.
 */
async function renderSeeded(
  settings: Settings = DEFAULTS,
  shortcuts: ShortcutStatus = SHORTCUTS_BOUND,
) {
  onCommand("get_settings", () => settings);
  onCommand("shortcut_status", () => shortcuts);
  const result = render(
    // NavigationProvider because the Glossary card's Manage row navigates: in
    // the app this view is always mounted inside it, under MainContent.
    <NavigationProvider>
      <UpdaterProvider>
        <ModelStatusProvider>
          <SettingsView />
        </ModelStatusProvider>
      </UpdaterProvider>
    </NavigationProvider>,
  );
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

  it("writes down both global chords, not just the capture toggle", async () => {
    await renderSeeded(DEFAULTS);

    // This card is the one place the app documents its shortcuts, so both of
    // them belong in it or neither does. Quick capture's chord used to appear
    // only as a hint on a command-palette row, which is no help to the person
    // who needs it: someone who has forgotten the chord is looking for where it
    // is written down, not for the palette entry that duplicates it.
    const capture = within(card("Capture"));
    expect(capture.getByText(CAPTURE_TOGGLE_SHORTCUT)).toBeInTheDocument();
    expect(capture.getByText(QUICK_CAPTURE_SHORTCUT)).toBeInTheDocument();

    // Both are read-only by design: the backend registers each at startup and
    // offers no rebinding command, so neither may render as a field that would
    // take an edit and drop it.
    expect(capture.queryByRole("textbox")).not.toBeInTheDocument();
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

describe("SettingsView global shortcut", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("says so when the chord never bound, and names the way that works", async () => {
    // The failure this row exists for: registration is best-effort at startup,
    // so another app holding the chord leaves the key dead while this screen —
    // the one place a user would look — went on printing it as fact.
    await renderSeeded(DEFAULTS, { captureToggle: false, quickCapture: true });

    expect(await within(card("Capture")).findByRole("alert")).toHaveTextContent(
      "Unavailable: another app is using this shortcut. Use the tray menu to start a capture.",
    );
  });

  it("keeps showing the chord it could not bind", async () => {
    // Naming the taken chord is what tells someone where to go looking, so the
    // failure is additive: the row says which key, and the foot says it is not
    // answering.
    await renderSeeded(DEFAULTS, { captureToggle: false, quickCapture: true });

    expect(within(card("Capture")).getByText(CAPTURE_TOGGLE_SHORTCUT)).toBeInTheDocument();
  });

  it("stays quiet when the chord bound", async () => {
    await renderSeeded(DEFAULTS, SHORTCUTS_BOUND);

    expect(within(card("Capture")).getByText(CAPTURE_TOGGLE_SHORTCUT)).toBeInTheDocument();
    expect(within(card("Capture")).queryByRole("alert")).not.toBeInTheDocument();
  });

  it("says nothing when it could not find out", async () => {
    // A read that failed is not evidence the chord is broken. Warning here
    // would put a scare in front of every user whose status read hiccuped, on
    // the strength of nothing.
    onCommand("get_settings", () => DEFAULTS);
    onCommand("shortcut_status", () => {
      throw "unreachable";
    });
    render(
      <NavigationProvider>
        <UpdaterProvider>
          <ModelStatusProvider>
            <SettingsView />
          </ModelStatusProvider>
        </UpdaterProvider>
      </NavigationProvider>,
    );
    await screen.findByRole("region", { name: "Privacy" });

    expect(within(card("Capture")).getByText(CAPTURE_TOGGLE_SHORTCUT)).toBeInTheDocument();
    expect(within(card("Capture")).queryByRole("alert")).not.toBeInTheDocument();
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

describe("SettingsView failures", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("says what happens next when the settings file will not load", async () => {
    // Every card is gated on `settings`, so this failure blanks the screen: it
    // is the one error here with nothing else left to look at
    // (docs/DESIGN_SYSTEM.md §3).
    onCommand("get_settings", () => {
      throw new Error("settings.json is malformed");
    });
    render(
      <NavigationProvider>
        <UpdaterProvider>
          <ModelStatusProvider>
            <SettingsView />
          </ModelStatusProvider>
        </UpdaterProvider>
      </NavigationProvider>,
    );

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(
      "Couldn't load your settings. They are still saved; reopen this view to try again.",
    );
    expect(screen.queryByText(/settings\.json is malformed/)).toBeNull();
  });

});

describe("SettingsView layout", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("names itself Settings and lays the concerns out as cards", async () => {
    await renderSeeded();

    expect(
      screen.getByRole("heading", { name: "Settings", level: 2 }),
    ).toBeInTheDocument();
    // Named regions, so the concerns are landmarks a screen reader can
    // jump between rather than tabs a pointer has to find.
    for (const concern of ["Privacy", "Appearance", "Capture", "Models", "Maintenance"]) {
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
      throw "Couldn't save the theme. The previous theme still applies; try again.";
    });
    await renderSeeded(DEFAULTS);

    await user.click(screen.getByRole("combobox", { name: /Theme/ }));
    await user.click(screen.getByRole("option", { name: "Dark" }));

    // The sentence says which value is actually in force, and the row below
    // proves it: not flipped optimistically, still showing what is on disk
    // (docs/DESIGN_SYSTEM.md §3).
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Couldn't save the theme. The previous theme still applies; try again.",
    );
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

  describe("the Models card", () => {
    const MISSING = {
      ready: false,
      bytesRequired: 762_000_000,
      bytesPresent: 0,
      sets: [],
      downloading: false,
      modelsDir: "C:\\app\\.models",
    };

    function models(): HTMLElement {
      return card("Models");
    }

    it("offers the download, quoting what is left to fetch", async () => {
      onCommand("model_status", () => MISSING);
      await renderSeeded();

      expect(
        await within(models()).findByRole("button", { name: "Download 762 MB" }),
      ).toBeInTheDocument();
    });

    it("is the permanent path a dismissed first-run nudge leaves behind", async () => {
      const user = userEvent.setup();
      onCommand("model_status", () => MISSING);
      onCommand("download_models", () => null);
      await renderSeeded();

      await user.click(
        await within(models()).findByRole("button", { name: "Download 762 MB" }),
      );
      expect(invokedCommands()).toContain("download_models");
    });

    it("states installation as a fact rather than a control, and never on a timer", async () => {
      onCommand("model_status", () => ({ ...MISSING, ready: true, bytesRequired: 0 }));
      await renderSeeded();

      expect(await within(models()).findByText("Installed")).toBeInTheDocument();
      // "You can change this" must not look like "this is how it is".
      expect(
        within(models()).queryByRole("button", { name: /Download/ }),
      ).not.toBeInTheDocument();
      // Unlike the rebuild confirmation, this reports configuration, not an
      // event, so it stays put rather than clearing itself.
      vi.useFakeTimers({ shouldAdvanceTime: true });
      act(() => {
        vi.advanceTimersByTime(10_000);
      });
      expect(within(models()).getByText("Installed")).toBeInTheDocument();
      vi.useRealTimers();
    });

    it("says so plainly when a developer has pointed the app elsewhere", async () => {
      onCommand("model_status", () => ({
        ...MISSING,
        ready: true,
        bytesRequired: 0,
        sets: [
          {
            id: "parakeet-tdt-0.6b-v2-int8",
            state: "env_overridden",
            bytesTotal: 0,
            bytesPresent: 0,
            license: "CC-BY-4.0",
          },
        ],
      }));
      await renderSeeded();

      expect(await within(models()).findByText("Developer override")).toBeInTheDocument();
    });

    it("carries the CC BY attribution the speech model's licence requires", async () => {
      onCommand("model_status", () => MISSING);
      await renderSeeded();

      const attribution = within(models()).getByText(/Parakeet TDT 0\.6b v2 by NVIDIA/);
      expect(attribution).toHaveTextContent(/CC BY 4\.0/);
      // The licence obliges us to state that the files were changed.
      expect(attribution).toHaveTextContent(/quantized to int8/);
      // A paragraph, so it reads a register up from a hint — the row's `body`
      // slot, not its `hint` (docs/DESIGN_SYSTEM.md §6). Pinned because the
      // text assertions above survive a silent revert to `hint`.
      expect(attribution).toHaveClass("text-ink-dim");
    });
  });
});

describe("SettingsView glossary card", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  /**
   * The vault glossary's only entry point. Every project glossary hangs off
   * its own project; the vault-wide one belongs to no folder, so if this row
   * does not navigate there is no way to reach the glossary that actually
   * biases transcription.
   */
  it("hands off to the vault glossary view", async () => {
    const user = userEvent.setup();
    const navigate = vi.fn();
    onCommand("get_settings", () => DEFAULTS);
    render(
      <NavigationContext.Provider value={{ view: { kind: "settings" }, navigate }}>
        <UpdaterProvider>
          <ModelStatusProvider>
            <SettingsView />
          </ModelStatusProvider>
        </UpdaterProvider>
      </NavigationContext.Provider>,
    );
    await screen.findByRole("region", { name: "Privacy" });

    await user.click(within(card("Glossary")).getByRole("button", { name: "Manage" }));

    // `slug: null` is the vault scope, not a missing argument.
    expect(navigate).toHaveBeenCalledWith({ kind: "glossary", slug: null });
  });

  it("says what the vault glossary is for, and where project terms live", async () => {
    await renderSeeded();

    const glossary = card("Glossary");
    expect(
      within(glossary).getByText(/bias transcription for every capture/),
    ).toBeInTheDocument();
    expect(
      within(glossary).getByText(/Each project keeps its own glossary for routing/),
    ).toBeInTheDocument();
  });
});

describe("SettingsView About card", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  function about(): HTMLElement {
    return card("About");
  }

  it("answers 'am I updated' with the running version", async () => {
    setAppVersion("1.4.2");
    await renderSeeded();

    expect(await within(about()).findByText("1.4.2")).toBeInTheDocument();
  });

  it("confirms an up-to-date result, then clears it, because it is news and not a label", async () => {
    // Fake timers before the render, not after the click: the confirmation's
    // setTimeout is scheduled during that click, and a clock installed
    // afterwards cannot advance a timer that is already on the real one.
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      await renderSeeded();

      await user.click(within(about()).getByRole("button", { name: "Check for updates" }));
      expect(
        await within(about()).findByText("You are on the latest version."),
      ).toBeInTheDocument();

      act(() => {
        vi.advanceTimersByTime(10_000);
      });
      expect(
        within(about()).queryByText("You are on the latest version."),
      ).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("offers the download when a manual check finds one", async () => {
    setAvailableUpdate({
      version: "0.2.0",
      download: async () => {},
      install: async () => {},
    });
    const user = userEvent.setup();
    await renderSeeded();

    await user.click(within(about()).getByRole("button", { name: "Check for updates" }));
    expect(await within(about()).findByRole("button", { name: "Download 0.2.0" })).toBeInTheDocument();
  });

  it("reports a failed manual check here, which is the one place that does", async () => {
    // The corner notice deliberately stays quiet about check failures; a check
    // someone clicked has to answer for itself.
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    failCheck("no route to host");
    const user = userEvent.setup();
    await renderSeeded();

    await user.click(within(about()).getByRole("button", { name: "Check for updates" }));
    // The plugin's transport error stays in the log; the card says what failed
    // and what happens next.
    expect(
      await within(about()).findByText(
        "Couldn't check for updates. Kodabi will try again next launch.",
      ),
    ).toBeInTheDocument();
    expect(within(about()).queryByText(/no route to host/)).toBeNull();
    logged.mockRestore();
  });

  it("saves the aging thresholds and the auto-close confidence", async () => {
    const user = userEvent.setup();
    const stored: Settings = {
      ...DEFAULTS,
      ledger: { aging_after_days: 7, stale_after_days: 30, conversation_autoclose: 0.8 },
    };
    onCommand("set_ledger_tuning", () => stored);
    await renderSeeded();

    const aging = screen.getByRole("spinbutton", { name: "Days before aging" });
    await user.clear(aging);
    await user.type(aging, "7{Enter}");

    // The three go together: the backend takes one struct, and the two day
    // thresholds are read against each other.
    expect(invoke).toHaveBeenCalledWith("set_ledger_tuning", {
      ledger: { aging_after_days: 7, stale_after_days: 30, conversation_autoclose: 0.8 },
    });
    expect(await screen.findByRole("status")).toHaveTextContent("Saved.");
  });

  it("sends the auto-close confidence as the fraction the backend validates", async () => {
    const user = userEvent.setup();
    const stored: Settings = {
      ...DEFAULTS,
      ledger: { aging_after_days: 14, stale_after_days: 30, conversation_autoclose: 0.95 },
    };
    onCommand("set_ledger_tuning", () => stored);
    await renderSeeded();

    // The field reads as a percentage because that is the number people have
    // an intuition about; the wire keeps the fraction.
    const confidence = screen.getByRole("spinbutton", { name: "Confidence percent" });
    await user.clear(confidence);
    await user.type(confidence, "95{Enter}");

    expect(invoke).toHaveBeenCalledWith("set_ledger_tuning", {
      ledger: { aging_after_days: 14, stale_after_days: 30, conversation_autoclose: 0.95 },
    });
  });

  it("does not write when a commitment field is committed unchanged", async () => {
    const user = userEvent.setup();
    onCommand("set_ledger_tuning", () => DEFAULTS);
    await renderSeeded();

    // Tabbing through blurs the field, which commits it. A blur that changed
    // nothing must not announce a save it did not make.
    const aging = screen.getByRole("spinbutton", { name: "Days before aging" });
    await user.click(aging);
    await user.tab();

    expect(invoke).not.toHaveBeenCalledWith("set_ledger_tuning", expect.anything());
  });

  it("treats a cleared commitment field as unchanged rather than as zero", async () => {
    const user = userEvent.setup();
    onCommand("set_ledger_tuning", () => DEFAULTS);
    await renderSeeded();

    // Clearing a field is how people start retyping one. Committing it as 0%
    // would be the one setting that closes every claimed commitment without
    // ever asking, so a blank field means "unchanged" and snaps back.
    const confidence = screen.getByRole("spinbutton", { name: "Confidence percent" });
    await user.clear(confidence);
    await user.tab();

    expect(invoke).not.toHaveBeenCalledWith("set_ledger_tuning", expect.anything());
    expect(confidence).toHaveValue(80);
  });

  it("reports a failed save under the field that asked for it", async () => {
    const user = userEvent.setup();
    onCommand("set_ledger_tuning", () => {
      throw "settings file is locked";
    });
    await renderSeeded();

    // A `foot` is the row's own status line, so an error belongs to the
    // control that raised it, not to whichever row happens to be last.
    const aging = screen.getByRole("spinbutton", { name: "Days before aging" });
    await user.clear(aging);
    await user.type(aging, "7{Enter}");

    // Each row is its own hairline unit, and the foot sits inside it.
    const alert = await screen.findByRole("alert");
    const confidence = screen.getByRole("spinbutton", { name: "Confidence percent" });
    expect(aging.closest("div.border-t")).toContainElement(alert);
    expect(confidence.closest("div.border-t")).not.toContainElement(alert);
  });
});

describe("SettingsView meeting kinds card", () => {
  it("lists the meeting kinds without offering a control that does nothing", async () => {
    await renderSeeded();

    const kinds = card("Meeting kinds");
    // The one place the taxonomy the note view and the Inbox speak is written
    // down. It is deliberately read-only: what a per-kind setting will DO is
    // the next change's decision, so a control here could only be a guess.
    expect(
      within(kinds).getByText(
        "Stand-up · One-on-one · Client · Working session · Review · All hands · Observer",
      ),
    ).toBeInTheDocument();
    expect(within(kinds).queryByRole("button")).toBeNull();
    expect(within(kinds).queryByRole("switch")).toBeNull();
    expect(within(kinds).queryByRole("combobox")).toBeNull();
  });
});
