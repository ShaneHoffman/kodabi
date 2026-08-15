import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

// A registry the fake xterm pushes each instance into, so the test can drive
// `onData` and inspect `write`. Hoisted so the mock factory below can close over
// it; the fake Terminal is defined INSIDE the factory to stay hoist-safe.
const { xtermRegistry } = vi.hoisted(() => ({
  xtermRegistry: {
    instances: [] as Array<{
      options: Record<string, unknown>;
      onDataHandler: ((data: string) => void) | null;
      writes: unknown[];
      openedIn: HTMLElement | null;
    }>,
  },
}));

vi.mock("@xterm/xterm", () => {
  class FakeTerminal {
    cols = 80;
    rows = 24;
    options: Record<string, unknown>;
    onDataHandler: ((data: string) => void) | null = null;
    writes: unknown[] = [];
    openedIn: HTMLElement | null = null;
    constructor(options: Record<string, unknown>) {
      this.options = options;
      xtermRegistry.instances.push(this);
    }
    loadAddon(): void {}
    open(node: HTMLElement): void {
      this.openedIn = node;
    }
    focus(): void {}
    reset(): void {}
    dispose(): void {}
    write(data: unknown): void {
      this.writes.push(data);
    }
    // The failure paths report into the buffer with `writeln` before setting
    // the state the view actually raises; without it here they throw instead.
    writeln(data: unknown): void {
      this.writes.push(data);
    }
    onData(callback: (data: string) => void): { dispose(): void } {
      this.onDataHandler = callback;
      return { dispose() {} };
    }
  }
  return { Terminal: FakeTerminal };
});

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit(): void {}
  },
}));

import {
  emitFromBackend,
  invoke,
  invokedCommands,
  onCommand,
  resetTauriMocks,
} from "../../test/tauri";
import { TerminalView } from "./TerminalView";

const SNAPSHOT = { running: true, scrollback: "", cols: 80, rows: 24 };

function latestTerminal() {
  const { instances } = xtermRegistry;
  const term = instances[instances.length - 1];
  if (!term) throw new Error("no xterm instance was created");
  return term;
}

describe("TerminalView", () => {
  beforeEach(() => {
    resetTauriMocks();
    xtermRegistry.instances.length = 0;
    onCommand("terminal_open", () => SNAPSHOT);
    onCommand("terminal_write", () => undefined);
    onCommand("terminal_resize", () => undefined);
    onCommand("terminal_restart", () => SNAPSHOT);
  });

  it("mounts in the view frame and opens the session", () => {
    render(<TerminalView />);
    expect(screen.getByRole("region", { name: "Terminal" })).toBeInTheDocument();
    expect(invokedCommands()).toContain("terminal_open");
  });

  // The Grove spec's two type facts about the pane. `allowTransparency` is the
  // load-bearing half: without it the theme's transparent background is
  // composited away and the glass-term well behind the mount never shows.
  it("builds xterm at the Grove step, over a transparent background", () => {
    render(<TerminalView />);
    const { options } = latestTerminal();
    expect(options.fontSize).toBe(12.5);
    expect(options.allowTransparency).toBe(true);
  });

  // The fit addon reads the mount's computed size to size the PTY grid and
  // subtracts only the `.xterm` element's padding, never the mount's — and under
  // `box-sizing: border-box` that read is the padding box. So padding on the
  // mount becomes grid space the well then clips away (the bug this pins: the
  // TUI's bottom row and rightmost columns were drawn outside the visible area).
  // The inset belongs one level up, on the well.
  //
  // The size half is the same invariant from the other side, and needs its own
  // assertion: a mount without `h-full` is a block at its content height, which
  // IS the rendered grid's height, so fit would read its own last answer back
  // and the grid could never shrink when the window does.
  it("opens xterm into a bare mount that fills the inset well", () => {
    render(<TerminalView />);
    const mount = latestTerminal().openedIn;
    expect(mount).not.toBeNull();
    expect(mount?.className).not.toMatch(/(^|[\s:])p[xytblrse]?-/);
    expect(mount?.className).toContain("h-full");
    expect(mount?.className).toContain("w-full");
    expect(mount?.parentElement?.className).toContain("glass-term");
  });

  it("reads the session's state in the header, and flips it on exit", () => {
    render(<TerminalView />);
    expect(screen.getByText("claude · kodabi mcp connected")).toBeInTheDocument();

    act(() => {
      emitFromBackend("terminal:exit", { code: 0 });
    });
    expect(screen.getByText("claude · exited")).toBeInTheDocument();
    expect(
      screen.queryByText("claude · kodabi mcp connected"),
    ).not.toBeInTheDocument();
  });

  it("streams keystrokes to the PTY via terminal_write", () => {
    render(<TerminalView />);
    act(() => {
      latestTerminal().onDataHandler?.("ls\r");
    });
    expect(invoke).toHaveBeenCalledWith("terminal_write", { data: "ls\r" });
  });

  it("writes backend output into the terminal", () => {
    render(<TerminalView />);
    const term = latestTerminal();
    const before = term.writes.length;
    act(() => {
      emitFromBackend("terminal:output", { data: btoa("hello") });
    });
    expect(term.writes.length).toBeGreaterThan(before);
  });

  it("offers a restart after the session exits", async () => {
    render(<TerminalView />);
    act(() => {
      emitFromBackend("terminal:exit", { code: 1 });
    });
    expect(screen.getByText(/session ended/i)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Restart" }));
    expect(invokedCommands()).toContain("terminal_restart");
    await waitFor(() =>
      expect(screen.queryByText(/session ended/i)).not.toBeInTheDocument(),
    );
    expect(screen.getByText("claude · kodabi mcp connected")).toBeInTheDocument();
  });

  // The bug this pins: the failed-to-start path set `status` but left `exit`
  // null, and the state block was gated on `exit` alone — so a session that
  // could not start raised nothing. No announcement, and no way back, at
  // exactly the moment recovery is the only thing the user wants. The red line
  // in the xterm buffer is not a substitute: it is not in the accessibility
  // tree. `role="alert"` is asserted rather than the text alone, because being
  // announced is the half that was missing.
  it("announces a failure to start, and offers a way back from it", async () => {
    onCommand("terminal_open", () => {
      throw "claude is not on PATH";
    });
    render(<TerminalView />);

    const failure = await screen.findByRole("alert");
    expect(failure).toHaveTextContent(/couldn't start claude code/i);
    expect(failure).toHaveTextContent("claude is not on PATH");
    expect(screen.getByText("claude · could not start")).toBeInTheDocument();

    onCommand("terminal_open", () => SNAPSHOT);
    await userEvent.click(screen.getByRole("button", { name: "Restart" }));
    expect(invokedCommands()).toContain("terminal_restart");
    await waitFor(() =>
      expect(screen.queryByRole("alert")).not.toBeInTheDocument(),
    );
    expect(screen.getByText("claude · kodabi mcp connected")).toBeInTheDocument();
  });

  // A restart that itself fails used to leave `exit` set, so the view kept
  // reporting the old exit while the session was in fact failed — the stale
  // half of the same gate.
  it("replaces the exit state when the restart itself fails", async () => {
    render(<TerminalView />);
    act(() => {
      emitFromBackend("terminal:exit", { code: 1 });
    });
    expect(screen.getByText(/session ended/i)).toBeInTheDocument();
    const ended = screen.getByRole("status");

    onCommand("terminal_restart", () => {
      throw "claude is not on PATH";
    });
    await userEvent.click(screen.getByRole("button", { name: "Restart" }));

    const failure = await screen.findByRole("alert");
    expect(failure).toHaveTextContent(/couldn't start claude code/i);
    expect(screen.queryByText(/session ended/i)).not.toBeInTheDocument();
    // And it is a NEW element, not the exited row with its `role` rewritten.
    // Both branches are the same component at the same position, so unkeyed
    // they reconcile into one `<p>` that merely swaps "status" for "alert" —
    // and a live region carries the politeness it was registered with when it
    // was inserted, so the assertive role would never take effect. Asserting
    // the attribute alone cannot tell the two apart; node identity can.
    expect(failure).not.toBe(ended);
  });
});
