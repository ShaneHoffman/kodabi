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
      onDataHandler: ((data: string) => void) | null;
      writes: unknown[];
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
    constructor(options: Record<string, unknown>) {
      this.options = options;
      xtermRegistry.instances.push(this);
    }
    loadAddon(): void {}
    open(): void {}
    focus(): void {}
    reset(): void {}
    dispose(): void {}
    write(data: unknown): void {
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
  });
});
