import { useRef } from "react";
import { useXterm, type TerminalStatus } from "../../useXterm";
import { Button } from "../ui/Button";
import { StatusMessage } from "../ui/StatusMessage";
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
 * of their own: the terminal variant's column is already the flex column that
 * gives the well its height (ViewFrame.tsx), and a second one only repeated it.
 */
export function TerminalView() {
  const mount = useRef<HTMLDivElement>(null);
  const { exit, startError, status, restart } = useXterm(mount);

  return (
    <ViewFrame
      variant="terminal"
      eyebrow="Claude Code"
      title="Terminal"
      summary={STATUS_LINE[status]}
    >
      {/* Two elements, not one, and the split is load-bearing. The well owns the
          surface and the inset; the mount inside it owns nothing but size.

          The fit addon sizes the PTY grid from `getComputedStyle(mount)` and
          subtracts only the `.xterm` element's own padding — never the mount's.
          Under Tailwind's global `box-sizing: border-box` that read resolves to
          the PADDING box, so any padding on the mount is handed to fit as grid
          space: the rows and columns that land in it are drawn outside the
          visible area and clipped away by `overflow-hidden`. Measured against
          this well, that was 40x36px of padding, about three columns and two
          rows off the right and bottom edges.

          `min-h-0` lets the well shrink under its own content, which is what
          lets fit size the grid to real pixels instead of to a page that keeps
          growing. The mount then tracks it exactly at `h-full w-full`, so the
          ResizeObserver in `useXterm` still sees every change. */}
      <div className="glass-term mt-6 min-h-0 flex-1 overflow-hidden px-5 py-[18px]">
        <div ref={mount} className="h-full w-full" />
      </div>
      {/* The session's state, and the one control that state raises
          (docs/UI_CONVENTIONS.md §5 — Restart belongs to the block, not to the
          frame). Gated on `status`, not on `exit`: a session that never started
          has no exit code, and gating on one left the failed path raising
          nothing at all — no announcement, and no way back.

          `StatusMessage` rather than a hand-rolled row, so the ARIA role comes
          from the variant (UI_CONVENTIONS §4): a failure is assertive, because
          the user did not ask for it, and an ordinary exit is polite.

          The keys are what make that second half true. Both branches render the
          same component at the same position, so without them React reconciles
          the exited row INTO the failed one: one `<p>` whose `role` mutates from
          "status" to "alert" in place. A live region is registered with the
          assistive tech when its node is inserted, so swapping the attribute
          afterwards downgrades a restart failure to the polite announcement the
          node was registered with. Distinct keys remount it, which is the only
          way the assertive role is the one actually in force. */}
      {status !== "running" && (
        <div className="flex items-center gap-2.5 pt-2">
          {/* `startError` is a whole sentence when there is one (`useXterm`
              words it, naming the Restart button beside this line), so it is
              rendered as-is rather than behind a prefix. The fallback covers
              the one case that has no message to show: a failure raised
              without one. */}
          {status === "failed" ? (
            <StatusMessage key="failed" variant="error" compact>
              {startError ?? "Couldn't start Claude Code. Restart to try again."}
            </StatusMessage>
          ) : (
            <StatusMessage key="exited" variant="status" compact>
              Session ended
              {exit?.code != null && exit.code !== 0 ? ` (exit ${exit.code})` : ""}.
            </StatusMessage>
          )}
          <Button onClick={restart}>Restart</Button>
        </div>
      )}
    </ViewFrame>
  );
}
