import { useLayoutEffect, useRef, useState, type RefObject } from "react";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal, type ITheme } from "@xterm/xterm";
import { backendCopy } from "./errorCopy";
// xterm.js ships its own required stylesheet — the terminal does not render
// without it, and it is a vendor file we neither write nor migrate.
// eslint-disable-next-line no-restricted-syntax -- third-party stylesheet, not ours to replace
import "@xterm/xterm/css/xterm.css";
import { TERMINAL_EXIT_EVENT, TERMINAL_OUTPUT_EVENT } from "./events";
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

/**
 * What the hosted session is doing, for the view's header line. Three states
 * and no "starting": `terminal_open` is idempotent and resolves in a frame or
 * two against an already-running PTY, so a fourth state would exist only to be
 * rendered on the way past.
 */
export type TerminalStatus = "running" | "exited" | "failed";

export type XtermHandle = {
  /** Non-null once `claude` has exited; the view then shows a restart affordance. */
  exit: TerminalExit | null;
  /**
   * Why the session could not start, whenever `status` is "failed" — the
   * backend's message, for the view's error state.
   *
   * The failure is painted into the xterm buffer in red as well, but buffer
   * text is not a view state: it is not in the accessibility tree, so it is
   * announced to nobody, and it vanishes with a clear. This field is what lets
   * the view raise a real one (docs/DESIGN_SYSTEM.md §3), mirroring
   * `useChatSession`'s `startError`.
   */
  startError: string | null;
  /** The session's state, for the view's status line. */
  status: TerminalStatus;
  /** Reap the exited/old session and spawn a fresh one, clearing the screen. */
  restart: () => void;
};

/** The terminal's size, in the number xterm wants rather than a CSS length.
 * Grove sets type sizes at the call site rather than in tokens, and this is
 * that call site: 12.5px is the Terminal screen's step in the Grove spec.
 *
 * Nothing sets `lineHeight` alongside it, deliberately — xterm's default of 1
 * is what keeps box-drawing characters joined, and the hosted `claude` draws
 * its whole interface out of them. */
const TERMINAL_FONT_SIZE = 12.5;

/** xterm's `background` when the surface behind it is doing the painting. A
 * fully transparent fill (paired with `allowTransparency`) lets the Grove well
 * — `glass-term`, which re-themes itself for `.day` and `.hc` in CSS — show
 * through, so the one plane we would otherwise have to composite in JS on
 * every theme change is simply not ours to hold. */
const TRANSPARENT = "#00000000";

/** The `::selection` wash from src/index.css, spelled identically on purpose:
 * marking words in the terminal is the same act as marking them in a note, so
 * the two read alike and re-theme together. It goes through `resolveColour`
 * because xterm cannot parse `color-mix()`. */
const SELECTION = "color-mix(in srgb, var(--color-kodama) 25%, transparent)";

/** `color(srgb r g b / a)` — how Chromium serialises a resolved `color-mix()`.
 * Matched strictly rather than loosely so an engine that ever hands back some
 * other form falls through to the value as-is instead of being mis-parsed. */
const RESOLVED_SRGB = /^color\(srgb ([\d.]+) ([\d.]+) ([\d.]+)(?: \/ ([\d.]+))?\)$/;

/**
 * A colour in a form xterm's own parser can actually read.
 *
 * Neither of the two things this has to swallow is plain. `getComputedStyle` on
 * an unregistered custom property returns the substituted token STREAM rather
 * than a resolved colour (which is why `token()` below needs its `.trim()` at
 * all), and a `color-mix()` is not resolved by reading it at all: xterm
 * substring-matches the first `rgb(` it finds, so handed a mix it would quietly
 * take one half of it and report nothing wrong.
 *
 * Resolving a mix takes the engine, so the value goes through a real colour
 * property on a live element. Chromium returns `color(srgb …)` for that, which
 * xterm cannot read either, hence the conversion back to the legacy form. A
 * value that is already legacy passes through untouched.
 */
function resolveColour(probe: HTMLElement, value: string): string {
  probe.style.color = "";
  probe.style.color = value;
  const computed = getComputedStyle(probe).color;
  probe.style.color = "";

  const match = RESOLVED_SRGB.exec(computed);
  if (!match) return computed || value;
  const [, red, green, blue, alpha] = match;
  const channel = (part: string) => Math.round(Number(part) * 255);
  return `rgba(${channel(red)}, ${channel(green)}, ${channel(blue)}, ${alpha ?? "1"})`;
}

/**
 * The current theme's terminal colours, read from the Grove theme so xterm
 * matches the app and re-themes with it. xterm's theme is set in JS, so this
 * is where the tokens are consumed rather than in a stylesheet.
 *
 * The base planes are Grove's. The background is not a colour at all: the
 * `glass-term` well behind the mount paints it, which is what lets `.day` and
 * `.hc` reach the surface through CSS instead of through this function. The
 * rest are the ink-at-rest step, the kodama for the caret (one of green's
 * sanctioned uses — DESIGN_SYSTEM §2), and the ground under the block cursor,
 * which inverts with the ground exactly as it should.
 *
 * The 16 ANSI colours are the `--color-ansi-*` group in the `@theme` block,
 * remapped by `.day` like every other Grove colour.
 *
 * `probe` is any mounted element, borrowed for one synchronous style read to
 * resolve the selection wash's `color-mix()`. The ANSI palette deliberately
 * stays ungated by the contrast switch (docs/DESIGN_SYSTEM.md §6 — xterm's own
 * `minimumContrastRatio` is the right lever for a hosted TUI, not a palette
 * swap), which is why it carries no `.hc` pair.
 */
function readTerminalTheme(styles: CSSStyleDeclaration, probe: HTMLElement): ITheme {
  const token = (name: string) => styles.getPropertyValue(name).trim();
  return {
    background: TRANSPARENT,
    foreground: token("--color-ink-read"),
    cursor: token("--color-kodama"),
    cursorAccent: token("--color-ground"),
    selectionBackground: resolveColour(probe, SELECTION),
    black: token("--color-ansi-black"),
    red: token("--color-ansi-red"),
    green: token("--color-ansi-green"),
    yellow: token("--color-ansi-yellow"),
    blue: token("--color-ansi-blue"),
    magenta: token("--color-ansi-magenta"),
    cyan: token("--color-ansi-cyan"),
    white: token("--color-ansi-white"),
    brightBlack: token("--color-ansi-bright-black"),
    brightRed: token("--color-ansi-bright-red"),
    brightGreen: token("--color-ansi-bright-green"),
    brightYellow: token("--color-ansi-bright-yellow"),
    brightBlue: token("--color-ansi-bright-blue"),
    brightMagenta: token("--color-ansi-bright-magenta"),
    brightCyan: token("--color-ansi-bright-cyan"),
    brightWhite: token("--color-ansi-bright-white"),
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
 * keeps the palette matched to the theme (MutationObserver → readTerminalTheme),
 * and replays the backend scrollback on mount so a view switch or hide-to-tray
 * re-hydrates the same session. The two PTY subscriptions compose the blessed
 * `useTauriEvent`; this hook is itself blessed, since it calls `useLayoutEffect`
 * to attach to a real DOM node — see .claude/rules/no-use-effect.md.
 *
 * It is one external system by the bridge-hook rule even at two subscriptions
 * and two observers: every one of them attaches to, and tears down with, the
 * same `Terminal`.
 */
export function useXterm(container: RefObject<HTMLDivElement | null>): XtermHandle {
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const [exit, setExit] = useState<TerminalExit | null>(null);
  const [startError, setStartError] = useState<string | null>(null);
  const [status, setStatus] = useState<TerminalStatus>("running");

  useTauriEvent<TerminalOutputEvent>(TERMINAL_OUTPUT_EVENT, (payload) => {
    termRef.current?.write(base64ToBytes(payload.data));
  });
  useTauriEvent<TerminalExitEvent>(TERMINAL_EXIT_EVENT, (payload) => {
    setExit({ code: payload.code });
    setStatus("exited");
  });

  useLayoutEffect(() => {
    const node = container.current;
    if (!node) return;

    const styles = getComputedStyle(document.documentElement);
    const term = new Terminal({
      theme: readTerminalTheme(styles, node),
      // `allowTransparency` is what the theme's transparent background needs to
      // mean anything: without it xterm composites every cell against an opaque
      // plane of its own and the Grove well never shows through.
      allowTransparency: true,
      fontFamily: styles.getPropertyValue("--font-data").trim() || "monospace",
      fontSize: TERMINAL_FONT_SIZE,
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
        //
        // The buffer line is the detail, not the state: `startError` is what
        // the view raises and a screen reader hears (see `XtermHandle`).
        if (!active) return;
        // Worded once and used twice: the buffer line and the state a screen
        // reader hears must be the same sentence, and neither may be the raw
        // rejection (docs/DESIGN_SYSTEM.md §3).
        const message = backendCopy(
          error,
          "Couldn't start Claude Code. Check that the claude command is installed, then press Restart to try again.",
        );
        term.writeln(`\x1b[31m${message}\x1b[0m`);
        setStartError(message);
        setStatus("failed");
      });

    const observer = new ResizeObserver(() => {
      if (!node.isConnected) return;
      fit.fit();
      void resizeTerminal(term.cols, term.rows);
    });
    observer.observe(node);

    // Grove re-themes by CLASS on <html> (`.day` and `.hc`, set by src/theme.ts
    // and src/contrast.ts), and xterm's palette is JS read once rather than CSS
    // resolved live — so something has to carry a theme change across. An
    // observer covers every route the class can change by: the in-app
    // appearance setting, the OS scheme flipping while the theme is "system"
    // (theme.ts re-applies the class directly, emitting nothing), and the
    // contrast toggle. The last two used to leave the pane stale.
    //
    // No rAF: the callback runs after the attribute has landed, and
    // `getComputedStyle` forces the recalc, so the tokens read here are the new
    // ones. (The event listener this replaced needed one — it raced the write.)
    const themeObserver = new MutationObserver(() => {
      term.options.theme = readTerminalTheme(getComputedStyle(document.documentElement), node);
    });
    themeObserver.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class"],
    });

    return () => {
      active = false;
      observer.disconnect();
      themeObserver.disconnect();
      dataSubscription.dispose();
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
  }, [container]);

  const restart = () => {
    void restartTerminal()
      .then((snapshot) => {
        const term = termRef.current;
        const fit = fitRef.current;
        if (!term || !fit) return;
        term.reset();
        if (snapshot.scrollback) term.write(base64ToBytes(snapshot.scrollback));
        fit.fit();
        void resizeTerminal(term.cols, term.rows);
        setExit(null);
        setStartError(null);
        setStatus("running");
      })
      .catch((error: unknown) => {
        // Same shape as the open failure above: a respawn can fail for the same
        // reasons the first spawn could, and an unreported rejection would leave
        // the pane sitting on a dead session with no account of why.
        //
        // `exit` is cleared with it. The old exit is stale the moment a restart
        // is attempted, and leaving it set would let the view go on reporting
        // "Session ended" over a failure that has its own thing to say.
        const message = backendCopy(
          error,
          "Couldn't restart Claude Code. Check that the claude command is installed, then press Restart to try again.",
        );
        termRef.current?.writeln(`\x1b[31m${message}\x1b[0m`);
        setExit(null);
        setStartError(message);
        setStatus("failed");
      });
  };

  return { exit, startError, status, restart };
}
