import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ConsentNudge } from "./ConsentNudge";
import type { Settings } from "../../useSettings";
import {
  invoke,
  invokedCommands,
  onCommand,
  resetTauriMocks,
} from "../../test/tauri";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

const ACKNOWLEDGED: Settings = {
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
  identity: { display_name: "", aliases: [] },
};

function primaryButton(): HTMLElement {
  return screen.getByRole("button", { name: "I understand, start capture" });
}

/** Pick a retention option by its visible label (`RETENTION_OPTIONS`). */
async function chooseRetention(
  user: ReturnType<typeof userEvent.setup>,
  label: string,
): Promise<void> {
  await user.click(screen.getByRole("combobox", { name: /retention/i }));
  await user.click(screen.getByRole("option", { name: label }));
}

/** The day count, which only exists while `keep_days` is the chosen policy. */
function daysField(): HTMLInputElement {
  return screen.getByRole("spinbutton", { name: "Days to keep" });
}

describe("ConsentNudge", () => {
  beforeEach(() => {
    resetTauriMocks();
  });

  it("persists consent before starting capture, then closes", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    onCommand("acknowledge_consent", () => ACKNOWLEDGED);
    onCommand("start_capture", () => null);
    render(<ConsentNudge onClose={onClose} />);

    await user.click(primaryButton());

    // Order is the invariant, not just that both ran: consent has to be on
    // disk before the microphone opens.
    expect(invokedCommands()).toEqual(["acknowledge_consent", "start_capture"]);
    expect(invoke).toHaveBeenCalledWith("acknowledge_consent", {
      retention: { policy: "keep_all" },
      displayName: "",
    });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("seeds the name the user's own commitments file under", async () => {
    const user = userEvent.setup();
    onCommand("acknowledge_consent", () => ACKNOWLEDGED);
    onCommand("start_capture", () => null);
    render(<ConsentNudge onClose={vi.fn()} />);

    // The one gate every install passes before its first capture, which makes
    // it the last moment this answer is still ahead of the first meeting.
    await user.type(screen.getByRole("textbox", { name: /Your name/ }), "  Avery  ");
    await user.click(primaryButton());

    expect(invoke).toHaveBeenCalledWith("acknowledge_consent", {
      retention: { policy: "keep_all" },
      displayName: "Avery",
    });
  });

  it("carries the chosen retention policy into acknowledge_consent", async () => {
    const user = userEvent.setup();
    onCommand("acknowledge_consent", () => ACKNOWLEDGED);
    onCommand("start_capture", () => null);
    render(<ConsentNudge onClose={vi.fn()} />);

    await chooseRetention(user, "Discard after distilling");
    await user.click(primaryButton());

    expect(invoke).toHaveBeenCalledWith("acknowledge_consent", {
      retention: { policy: "discard_after_distill" },
      displayName: "",
    });
  });

  it("carries an edited day count into a keep_days policy", async () => {
    const user = userEvent.setup();
    onCommand("acknowledge_consent", () => ACKNOWLEDGED);
    onCommand("start_capture", () => null);
    render(<ConsentNudge onClose={vi.fn()} />);

    // The only option with a second control behind it, and the only one whose
    // policy isn't a constant.
    await chooseRetention(user, "Keep for a number of days");
    await user.clear(daysField());
    await user.type(daysField(), "7");
    await user.click(primaryButton());

    expect(invoke).toHaveBeenCalledWith("acknowledge_consent", {
      retention: { policy: "keep_days", days: 7 },
      displayName: "",
    });
  });

  it("clamps an emptied day count instead of sending an invalid policy", async () => {
    const user = userEvent.setup();
    onCommand("acknowledge_consent", () => ACKNOWLEDGED);
    onCommand("start_capture", () => null);
    render(<ConsentNudge onClose={vi.fn()} />);

    await chooseRetention(user, "Keep for a number of days");
    // The field is held as a raw string so it can be cleared mid-edit; blank
    // parses to NaN, and the backend's NonZeroU32 rejects the 0 that implies.
    await user.clear(daysField());
    await user.click(primaryButton());

    expect(invoke).toHaveBeenCalledWith("acknowledge_consent", {
      retention: { policy: "keep_days", days: 1 },
      displayName: "",
    });
  });

  it("never starts capture when persisting consent fails", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    onCommand("acknowledge_consent", () => {
      throw "Couldn't save your choice, so recording stays off. Try again.";
    });
    onCommand("start_capture", () => null);
    render(<ConsentNudge onClose={onClose} />);

    await user.click(primaryButton());

    // The load-bearing gate: a failed persist means consent was NOT granted,
    // so nothing may record.
    expect(invokedCommands()).not.toContain("start_capture");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Couldn't save your choice, so recording stays off. Try again.",
    );
    expect(onClose).not.toHaveBeenCalled();
    // Re-enabled, so the user can retry rather than being stuck.
    expect(primaryButton()).toBeEnabled();
  });

  it("surfaces a failed start with consent already granted", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    onCommand("acknowledge_consent", () => ACKNOWLEDGED);
    onCommand("start_capture", () => {
      throw "Couldn't start recording. Check your microphone in Windows sound settings, then try again.";
    });
    render(<ConsentNudge onClose={onClose} />);

    await user.click(primaryButton());

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Couldn't start recording. Check your microphone in Windows sound settings, then try again.",
    );
    expect(onClose).not.toHaveBeenCalled();
  });

  it("dismisses without touching the backend on Not now", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<ConsentNudge onClose={onClose} />);

    await user.click(screen.getByRole("button", { name: "Not now" }));

    expect(onClose).toHaveBeenCalledTimes(1);
    expect(invoke).not.toHaveBeenCalled();
  });

  it("opens focused on the primary action", async () => {
    render(<ConsentNudge onClose={vi.fn()} />);

    // Not the first control: the retention default is already the safe one,
    // and acknowledging is what the user came here to do.
    await waitFor(() => {
      expect(primaryButton()).toHaveFocus();
    });
  });

  it("dismisses on Escape without touching the backend", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<ConsentNudge onClose={onClose} />);

    await user.keyboard("{Escape}");

    expect(onClose).toHaveBeenCalledTimes(1);
    expect(invoke).not.toHaveBeenCalled();
  });

  it("lets Escape close the open retention list without closing the nudge", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<ConsentNudge onClose={onClose} />);

    await user.click(screen.getByRole("combobox", { name: /retention/i }));
    expect(screen.getByRole("listbox")).toBeInTheDocument();

    await user.keyboard("{Escape}");

    // The Select stops that Escape from bubbling, which is what keeps it from
    // reaching the dialog's own dismissal. One Escape, one thing closed.
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });
});
