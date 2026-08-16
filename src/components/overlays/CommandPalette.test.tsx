import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CommandPalette } from "./CommandPalette";
import { NavigationProvider } from "../providers/NavigationProvider";
import { emitFromBackend, invokedCommands, onCommand, resetTauriMocks } from "../../test/tauri";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

function serveVault(): void {
  onCommand("list_projects", () => ({
    inbox_note_count: 0,
    projects: [
      {
        id: "briarwood-golf",
        slug: "briarwood-golf",
        display_name: "Briarwood Golf",
        parent: null,
        note_count: 2,
        meeting_count: 0,
      },
    ],
  }));
  onCommand("list_failed_sessions", () => []);
  // The capture row keys off the phase, so the seed the registry reads has to
  // be routed like any other command: an unstubbed one rejects, which
  // `useCaptureState` survives by staying idle, but then the idle row would be
  // passing for the wrong reason.
  onCommand("capture_phase", () => ({
    phase: "idle",
    sources: { loopback: "off", microphone: "off" },
  }));
  onCommand("start_capture", () => ({}));
  onCommand("stop_capture", () => ({}));
}

/** Puts the backend into a running capture, as a start would. */
function goLive(): void {
  act(() => {
    emitFromBackend("capture:state", {
      phase: "listening",
      sources: { loopback: "live", microphone: "live" },
    });
  });
}

async function renderPalette() {
  const result = render(
    <NavigationProvider>
      <CommandPalette open onClose={() => {}} />
    </NavigationProvider>,
  );
  await screen.findByRole("option", { name: /briarwood-golf/ });
  return result;
}

function input(): HTMLElement {
  return screen.getByRole("combobox");
}

/** The row cmdk would run on Enter. */
function selectedOption(): HTMLElement {
  const selected = screen
    .getAllByRole("option")
    .find((option) => option.getAttribute("aria-selected") === "true");
  if (!selected) throw new Error("no option is selected");
  return selected;
}

describe("CommandPalette", () => {
  beforeEach(() => {
    resetTauriMocks();
    serveVault();
  });

  it("groups an unfiltered list into places, folders and verbs", async () => {
    // Somewhere to go, a folder to open and something to do are three different
    // questions. The headings are screen-reader only — sighted users get the
    // hairlines — so a group with no accessible name would be a group a screen
    // reader could not describe.
    await renderPalette();

    expect(screen.getByRole("group", { name: "Jump to" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Folders" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Actions" })).toBeInTheDocument();
  });

  it("names the list itself, rather than taking the library's default", async () => {
    // cmdk spreads the caller's props before its own aria-label, so the name
    // has to arrive through `label` — passed any other way it is silently
    // replaced by the word "Suggestions", which is not what this list is.
    await renderPalette();

    expect(screen.getByRole("listbox", { name: "Commands" })).toBeInTheDocument();
  });

  it("tags each row with the kind of thing it is", async () => {
    await renderPalette();

    expect(screen.getByRole("option", { name: /^Inbox/ })).toHaveTextContent("navigate");
    expect(screen.getByRole("option", { name: /briarwood-golf/ })).toHaveTextContent("folder");
    expect(screen.getByRole("option", { name: /New note/ })).toHaveTextContent("action");
  });

  it("gives a folder row its project's identity hue", async () => {
    // The dot is decoration for the name beside it, so it is hidden from the
    // accessible name rather than read out as a colour.
    await renderPalette();

    const dot = screen
      .getByRole("option", { name: /briarwood-golf/ })
      .querySelector("[aria-hidden='true']");

    expect(dot).toHaveClass("bg-coral");
  });

  it("teaches the global shortcut of a command that has one, and invents none", async () => {
    // The palette is where people learn the bindings, so a hint here has to be
    // a real accelerator the backend registered. Every other row shows its kind
    // rather than a label dressed up as a key.
    await renderPalette();

    expect(screen.getByRole("option", { name: /Quick capture/ })).toHaveTextContent(
      "Ctrl+Alt+Space",
    );
    expect(screen.getByRole("option", { name: /Start capture/ })).toHaveTextContent(
      "Ctrl+Shift+K",
    );
    expect(screen.getByRole("option", { name: /New note/ })).not.toHaveTextContent("Ctrl");
  });

  it("offers the needs-attention jump while only dismissed captures remain", async () => {
    // The sidebar row hides once everything is dismissed, so this jump is the
    // one way back to the dismissed shelf — it keys on the full listing,
    // dismissed included, or dismissal would stop being reversible.
    serveVault();
    onCommand("list_failed_sessions", () => [
      {
        path: "sessions/2026-07-01T10-00-00Z-team-sync.jsonl",
        file_name: "2026-07-01T10-00-00Z-team-sync.jsonl",
        slug: "team-sync",
        captured_at: "2026-07-01T10:00:00Z",
        dismissed: true,
      },
    ]);

    await renderPalette();

    expect(await screen.findByRole("option", { name: /Needs attention/ })).toBeInTheDocument();
  });

  it("walks the arrow keys straight across a group boundary", async () => {
    // The groups are visual only: the highlight must not stall or skip where
    // one ends and the hairline begins.
    const user = userEvent.setup();
    await renderPalette();
    const options = screen.getAllByRole("option");
    // Inbox, Search notes, Open chat, Open terminal, Settings |
    // briarwood-golf | New note, Quick capture, Start capture
    expect(options).toHaveLength(9);
    expect(selectedOption()).toHaveTextContent("Inbox");

    await user.keyboard("{ArrowDown}{ArrowDown}{ArrowDown}{ArrowDown}{ArrowDown}");

    // The sixth row is the first folder, reached with no extra keypress for the
    // separator in between.
    expect(selectedOption()).toHaveTextContent("briarwood-golf");
  });

  it("matches on more than a substring", async () => {
    // The hand-rolled filter this replaced was `includes` over the title, so a
    // gapped abbreviation found nothing at all.
    const user = userEvent.setup();
    await renderPalette();

    await user.type(input(), "trm");

    expect(screen.getByRole("option", { name: /Open terminal/ })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /^Inbox/ })).not.toBeInTheDocument();
  });

  it("drops the group scaffolding while filtering", async () => {
    // A filtered list is a set of matches, not a table of contents.
    const user = userEvent.setup();
    await renderPalette();

    await user.type(input(), "briarwood");

    expect(screen.queryByRole("group", { name: "Jump to" })).not.toBeInTheDocument();
    expect(screen.getAllByRole("option")).toHaveLength(1);
  });

  it("turns a query that matches nothing into a search", async () => {
    // Typed text always leads somewhere, so there is no empty state to reach.
    // The fallback has to be *selected*, not merely present: it is the row
    // Enter runs, and it is force-mounted past the filter that hid the rest.
    const user = userEvent.setup();
    await renderPalette();

    await user.type(input(), "zzzz");

    const [fallback] = screen.getAllByRole("option");
    expect(screen.getAllByRole("option")).toHaveLength(1);
    expect(fallback).toHaveTextContent("Search for “zzzz”");
    expect(fallback).toHaveAttribute("aria-selected", "true");
  });

  it("runs the selected command and closes", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(
      <NavigationProvider>
        <CommandPalette open onClose={onClose} />
      </NavigationProvider>,
    );
    await screen.findByRole("option", { name: /briarwood-golf/ });

    await user.type(input(), "terminal");
    await user.keyboard("{Enter}");

    expect(onClose).toHaveBeenCalledOnce();
  });

  it("starts a capture from inside the main window", async () => {
    // The window had no start-capture control at all before this row: the pill
    // is presentational and quick capture is a different window, so the palette
    // is the searchable way in.
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(
      <NavigationProvider>
        <CommandPalette open onClose={onClose} />
      </NavigationProvider>,
    );
    await screen.findByRole("option", { name: /briarwood-golf/ });

    await user.click(screen.getByRole("option", { name: /Start capture/ }));

    expect(invokedCommands()).toContain("start_capture");
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("offers the stop half of the toggle once a capture is running", async () => {
    // The chord toggles, so the row and its hint have to agree with what a press
    // would actually do: printing "Start capture / Ctrl+Shift+K" over a live
    // session would teach the chord starts one when it would stop one.
    const user = userEvent.setup();
    await renderPalette();

    goLive();

    expect(screen.queryByRole("option", { name: /Start capture/ })).not.toBeInTheDocument();
    const stop = screen.getByRole("option", { name: /Stop capture/ });
    expect(stop).toHaveTextContent("Ctrl+Shift+K");

    await user.click(stop);

    expect(invokedCommands()).toContain("stop_capture");
  });

  it("survives a capture command the backend refuses", async () => {
    // Consent-missing is the everyday case: the backend rejects and emits
    // `consent:required`, which opens the nudge over the shell. The palette has
    // already closed by then, so it must not surface (or trip over) the
    // rejection itself.
    const user = userEvent.setup();
    onCommand("start_capture", () => {
      throw "Recording needs your consent first. Answer the consent prompt and capture will start.";
    });
    await renderPalette();

    await user.click(screen.getByRole("option", { name: /Start capture/ }));

    expect(invokedCommands()).toContain("start_capture");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("hands focus back to whatever opened it", async () => {
    // base-ui owns the trap and the restore; what this pins is that the palette
    // is wired to it at all, since the hand-rolled version it replaces had to
    // save and restore the opener itself.
    const user = userEvent.setup();
    function Harness() {
      const [open, setOpen] = useState(false);
      return (
        <NavigationProvider>
          <button onClick={() => setOpen(true)}>Commands</button>
          <CommandPalette open={open} onClose={() => setOpen(false)} />
        </NavigationProvider>
      );
    }
    render(<Harness />);
    const opener = screen.getByRole("button", { name: "Commands" });

    await user.click(opener);
    // The query field takes initial focus, which base-ui moves a tick after the
    // dialog mounts.
    await waitFor(() => {
      expect(screen.getByRole("combobox")).toHaveFocus();
    });

    await user.keyboard("{Escape}");

    expect(opener).toHaveFocus();
  });
});
