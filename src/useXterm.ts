import { useLayoutEffect, useRef, useState, type RefObject } from "react";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal, type ITheme } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import {
  SETTINGS_CHANGED_EVENT,
  TERMINAL_EXIT_EVENT,
  TERMINAL_OUTPUT_EVENT,
} from "./events";
import {
  openTerminal,
  resizeTerminal,
  restartTerminal,
  writeTerminal,
  type TerminalExitEvent,
  type TerminalOutputEvent,
  type TerminalSnapshot,
} from "./terminal";
import { useTauriEvent } from "./useTauriEvent";

/** The hosted process's exit, surfaced so the view can offer a restart. */
export type TerminalExit = { code: number | null };

export type XtermHandle = {
  /** Non-null once `claude` has exited; the view then shows a restart affordance. */
  exit: TerminalExit | null;
  /** Reap the exited/old session and spawn a fresh one, clearing the screen. */
  restart: () => void;
};

/** xterm's font size, read from a token and parsed to the number it wants. */
function readSizeToken(styles: CSSStyleDeclaration, name: string, fallback: number): number {
  const parsed = Number.parseFloat(styles.getPropertyValue(name));
  return Number.isFinite(parsed) ? parsed : fallback;
}

/**
 * The current theme's terminal colours, read from the design tokens so xterm
 * matches the app and re-themes with it. The base planes reuse existing
 * semantic tokens (`--surface`, `--text-read`, the reserved-green `--accent-dot`
 * caret); the 16 ANSI colours come from the `--ansi-*` group in
 * design/tokens.css. xterm's theme is set in JS, so this is where the tokens are
 * consumed — not a `.css` file the token guard would police.
 */
function readTerminalTheme(styles: CSSStyleDeclaration): ITheme {
  const token = (name: string) => styles.getPropertyValue(name).trim();
  return {
    background: token("--surface"),
    foreground: token("--text-read"),
    cursor: token("--accent-dot"),
    cursorAccent: token("--surface"),
    selectionBackground: token("--highlight"),
    black: token("--ansi-black"),
    red: token("--ansi-red"),
    green: token("--ansi-green"),
    yellow: token("--ansi-yellow"),
    blue: token("--ansi-blue"),
    magenta: token("--ansi-magenta"),
    cyan: token("--ansi-cyan"),
    white: token("--ansi-white"),
    brightBlack: token("--ansi-bright-black"),
    brightRed: token("--ansi-bright-red"),
    brightGreen: token("--ansi-bright-green"),
    brightYellow: token("--ansi-bright-yellow"),
    brightBlue: token("--ansi-bright-blue"),
    brightMagenta: token("--ansi-bright-magenta"),
    brightCyan: token("--ansi-bright-cyan"),
    brightWhite: token("--ansi-bright-white"),
  };
}

/** Decodes base64 PTY bytes to the `Uint8Array` xterm's decoder reassembles
 * correctly across writes (a per-chunk UTF-8 decode would corrupt splits). */
function base64ToBytes(base64: string): Uint8Array {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

/**
 * Owns the xterm.js `Terminal` bound to `container`: builds it themed from the
 * design tokens, streams keystrokes to the PTY (`onData` → terminal_write),
 * keeps the PTY grid matched to the viewport (ResizeObserver → terminal_resize),
 * and replays the backend scrollback on mount so a view switch or hide-to-tray
 * re-hydrates the same session. The two PTY subscriptions compose the blessed
 * `useTauriEvent`; this hook is itself blessed, since it calls `useLayoutEffect`
 * to attach to a real DOM node — see .claude/rules/no-use-effect.md.
 */
export function useXterm(container: RefObject<HTMLDivElement | null>): XtermHandle {
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const [exit, setExit] = useState<TerminalExit | null>(null);

  useTauriEvent<TerminalOutputEvent>(TERMINAL_OUTPUT_EVENT, (payload) => {
    termRef.current?.write(base64ToBytes(payload.data));
  });
  useTauriEvent<TerminalExitEvent>(TERMINAL_EXIT_EVENT, (payload) => {
    setExit({ code: payload.code });
  });
  // Re-read the palette after an in-app appearance change. The rAF lets the
  // theme attribute land on <html> (src/theme.ts) before we read the tokens.
  useTauriEvent(SETTINGS_CHANGED_EVENT, () => {
    requestAnimationFrame(() => {
      const term = termRef.current;
      if (term) {
        term.options.theme = readTerminalTheme(getComputedStyle(document.documentElement));
      }
    });
  });

  useLayoutEffect(() => {
    const node = container.current;
    if (!node) return;

    const styles = getComputedStyle(document.documentElement);
    const term = new Terminal({
      theme: readTerminalTheme(styles),
      fontFamily: styles.getPropertyValue("--mono").trim() || "monospace",
      // --fs-action is the 13px mono metadata step; a comfortable terminal size.
      fontSize: readSizeToken(styles, "--fs-action", 13),
      cursorBlink: true,
      scrollback: 5000,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(node);
    fit.fit();
    term.focus();
    termRef.current = term;
    fitRef.current = fit;

    const dataSubscription = term.onData((data) => {
      void writeTerminal(data);
    });

    const applySnapshot = (snapshot: TerminalSnapshot) => {
      if (snapshot.scrollback) term.write(base64ToBytes(snapshot.scrollback));
      fit.fit();
      void resizeTerminal(term.cols, term.rows);
    };

    let active = true;
    void openTerminal()
      .then((snapshot) => {
        if (active) applySnapshot(snapshot);
      })
      .catch((error: unknown) => {
        // A spawn failure (e.g. no `claude` on PATH) rejects the command; show
        // it in the pane rather than leaving a blank terminal. \x1b[31m…\x1b[0m
        // is the ANSI red the terminal already renders.
        if (active) term.writeln(`\x1b[31mCould not start Claude Code: ${String(error)}\x1b[0m`);
      });

    const observer = new ResizeObserver(() => {
      if (!node.isConnected) return;
      fit.fit();
      void resizeTerminal(term.cols, term.rows);
    });
    observer.observe(node);

    return () => {
      active = false;
      observer.disconnect();
      dataSubscription.dispose();
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
  }, [container]);

  const restart = () => {
    void restartTerminal().then((snapshot) => {
      const term = termRef.current;
      const fit = fitRef.current;
      if (!term || !fit) return;
      term.reset();
      if (snapshot.scrollback) term.write(base64ToBytes(snapshot.scrollback));
      fit.fit();
      void resizeTerminal(term.cols, term.rows);
      setExit(null);
    });
  };

  return { exit, restart };
}
