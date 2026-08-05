import { useRef } from "react";
import { useXterm, type TerminalStatus } from "../../useXterm";
import { Button } from "../ui/Button";
import { ViewFrame } from "../ui/ViewFrame";

/**
 * The header's line beside the title, one per session state.
 *
 * "kodabi mcp connected" is a claim about wiring, and it is one the app is
 * entitled to make: the backend writes the session's `kodabi.mcp.json` and
 * launches `claude` against it before the PTY exists at all
 * (src-tauri/src/terminal_cmds.rs), so a running session is a session with the
 * vault's MCP server attached.
 *
 * "exited" rather than "session ended": the row below already says that, and a
 * header that repeats the thing under it is a header saying nothing.
 */
const STATUS_LINE: Record<TerminalStatus, string> = {
  running: "claude · kodabi mcp connected",
  exited: "claude · exited",
  failed: "claude · could not start",
};

/**
 * The embedded Claude Code terminal (Phase 3, FOUNDING_DOC §4): an xterm.js pane
 * hosting the interactive `claude` CLI with the `kodabi` MCP server already
 * wired, so chat-over-the-knowledge-base works with zero setup. The PTY lives in
 * the Rust shell and survives view switches; `useXterm` owns the xterm instance
 * bound to the mount below and re-hydrates it from the backend scrollback.
 *
 * The chrome is Grove's — a masthead with the session's state beside it, over
 * the `glass-term` well — and everything inside the well is xterm's, themed
 * from the same tokens by `useXterm`.
 *
 * The two children sit directly in the frame's column rather than in a wrapper
 * of their own: `.view--terminal .view__column` is already the flex column that
 * gives the well its height (ViewFrame.css), and a second one only repeated it.
 */
export function TerminalView() {
  const mount = useRef<HTMLDivElement>(null);
  const { exit, status, restart } = useXterm(mount);

  return (
    <ViewFrame
      variant="terminal"
      eyebrow="Claude Code"
      title="Terminal"
      summary={STATUS_LINE[status]}
    >
      {/* The mount IS the well: xterm builds inside it, so the padding here is
          what holds the rendered rows off the surface's edge. `min-h-0` lets it
          shrink under its own content, which is what lets the fit addon size the
          PTY grid to real pixels instead of to a page that keeps growing. */}
      <div
        ref={mount}
        className="glass-term mt-6 min-h-0 flex-1 overflow-hidden px-5 py-[18px]"
      />
      {exit && (
        <div className="flex items-center gap-2.5 pt-2" role="status">
          <span className="font-data text-[11px] text-ink-dim">
            Session ended
            {exit.code != null && exit.code !== 0 ? ` (exit ${exit.code})` : ""}.
          </span>
          <Button onClick={restart}>Restart</Button>
        </div>
      )}
    </ViewFrame>
  );
}
