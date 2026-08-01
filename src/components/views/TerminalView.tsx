import { useRef } from "react";
import { useXterm } from "../../useXterm";
import { Button } from "../ui/Button";
import { ViewFrame } from "../ui/ViewFrame";
// eslint-disable-next-line no-restricted-syntax -- pre-Grove; this view's Grove ticket deletes it
import "./TerminalView.css";

/**
 * The embedded Claude Code terminal (Phase 3, FOUNDING_DOC §4): an xterm.js pane
 * hosting the interactive `claude` CLI with the `kodabi` MCP server already
 * wired, so chat-over-the-knowledge-base works with zero setup. The PTY lives in
 * the Rust shell and survives view switches; `useXterm` owns the xterm instance
 * bound to the mount below and re-hydrates it from the backend scrollback.
 */
export function TerminalView() {
  const mount = useRef<HTMLDivElement>(null);
  const { exit, restart } = useXterm(mount);

  return (
    <ViewFrame variant="terminal" eyebrow="Claude Code" title="Terminal">
      <div className="terminal-view">
        <div ref={mount} className="terminal-view__mount" />
        {exit && (
          <div className="terminal-view__ended" role="status">
            <span className="text-label text-text-soft">
              Session ended
              {exit.code != null && exit.code !== 0 ? ` (exit ${exit.code})` : ""}.
            </span>
            <Button onClick={restart}>Restart</Button>
          </div>
        )}
      </div>
    </ViewFrame>
  );
}
