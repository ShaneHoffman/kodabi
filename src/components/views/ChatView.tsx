import {
  useState,
  type FormEvent,
  type KeyboardEvent,
  type UIEvent,
} from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { ChatEntry, PendingPermission } from "../../chat";
import { useChatSession } from "../../useChatSession";
import { useScrollIntoView } from "../../useScrollIntoView";
import { Button } from "../ui/Button";
import { StatusMessage } from "../ui/StatusMessage";
import { ViewFrame } from "../ui/ViewFrame";
// eslint-disable-next-line no-restricted-syntax -- pre-Grove; this view's Grove ticket deletes it
import "./markdownReading.css";
// eslint-disable-next-line no-restricted-syntax -- pre-Grove; this view's Grove ticket deletes it
import "./ChatView.css";

/** The bottom sentinel the log keeps in view as the conversation grows. */
const LOG_END_ID = "chat-view-end";

/** How far above the bottom edge still counts as "reading the live end".
 * Within it, growth keeps the end in view; beyond it, the user has scrolled
 * up into the backlog and streaming must not yank them back down. */
const FOLLOW_SLACK_PX = 40;

/**
 * The designed chat over the knowledge base (FOUNDING_DOC §4: the front door;
 * the terminal is the escape hatch). Claude Code runs headless underneath,
 * grounded through the kodabi MCP tools; this view renders the conversation
 * in the app's two voices — your messages in the interface sans, answers on
 * the reading ramp (`.md-reading` serif, the same surface a note's body uses)
 * — with tool use as quiet mono lines and MCP write approvals as inline
 * cards.
 *
 * A token feed is still not a licence to animate (docs/DESIGN_SYSTEM.md §4):
 * nothing here reacts to a delta. What does animate is the live answer block's
 * one-shot arrival — `.chat-view__answer` fades and rises in as it first
 * appears, because an answer materialising fully formed reads as breakage
 * rather than as arriving. The completed entry it hands off to is deliberately
 * left without that class (see `ChatEntryRow`), so the handoff is invisible.
 *
 * The streamed prose is not a live region; one offscreen `role="status"` line
 * narrates the turn instead.
 */
export function ChatView() {
  const chat = useChatSession();
  const [draft, setDraft] = useState("");
  // Whether the reader is at the live end of the log. Scrolling up into the
  // backlog unpins; scrolling back down (or sending a message) repins.
  const [pinnedToEnd, setPinnedToEnd] = useState(true);

  useScrollIntoView(
    // Content growth keeps the end in view — but only for a reader who is
    // there: yanking someone out of the backlog on every streamed delta would
    // make scrollback unreadable while a turn runs.
    pinnedToEnd ? LOG_END_ID : null,
    `${chat.entries.length}:${chat.streamingText.length}:${
      chat.pending?.request_id ?? ""
    }:${chat.exited}`,
  );

  const logScrolled = (event: UIEvent<HTMLDivElement>) => {
    const log = event.currentTarget;
    setPinnedToEnd(
      log.scrollHeight - log.scrollTop - log.clientHeight <= FOLLOW_SLACK_PX,
    );
  };

  const submit = (event: FormEvent) => {
    event.preventDefault();
    const text = draft.trim();
    if (!text || chat.turnActive || chat.exited) return;
    chat.send(text);
    setDraft("");
    // Your own message belongs on screen: sending rejoins the live end even
    // from deep in the backlog.
    setPinnedToEnd(true);
  };

  const composerKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      event.currentTarget.form?.requestSubmit();
    }
  };

  // Loading renders nothing (docs/DESIGN_SYSTEM.md §3) — but a session that
  // could not start at all must still say so.
  if (!chat.ready) {
    return (
      <ViewFrame variant="chat" eyebrow="Claude" title="Chat">
        {chat.startError && (
          <StatusMessage variant="error">
            Couldn&apos;t start chat: {chat.startError}
          </StatusMessage>
        )}
      </ViewFrame>
    );
  }

  const empty =
    chat.entries.length === 0 && !chat.streamingText && !chat.exited;

  return (
    <ViewFrame variant="chat" eyebrow="Claude" title="Chat">
      <div className="chat-view">
        <p className="sr-only" role="status">
          {chat.turnActive ? "Claude is answering" : ""}
        </p>
        <div
          className="chat-view__log"
          aria-label="Conversation"
          aria-busy={chat.turnActive || undefined}
          onScroll={logScrolled}
        >
          {empty && (
            <StatusMessage variant="empty">
              Ask about your notes. Claude can search the knowledge base, open
              notes, and file things where they belong.
            </StatusMessage>
          )}
          {chat.entries.map((entry, index) => (
            <ChatEntryRow key={index} entry={entry} />
          ))}
          {chat.streamingText && (
            <div className="chat-view__answer md-reading font-serif">
              <ReactMarkdown remarkPlugins={[remarkGfm]}>
                {chat.streamingText}
              </ReactMarkdown>
            </div>
          )}
          {chat.pending && (
            <PermissionCard
              pending={chat.pending}
              respond={chat.respondPermission}
            />
          )}
          {chat.exited && (
            <div className="chat-view__ended" role="status">
              <p className="text-body text-text-soft">
                Claude Code exited. Start a new chat to continue.
              </p>
              <Button variant="primary" onClick={chat.restart}>
                Start a new chat
              </Button>
            </div>
          )}
          <div id={LOG_END_ID} />
        </div>

        {!chat.exited && (
          <form className="chat-view__composer" onSubmit={submit}>
            <textarea
              aria-label="Message Claude"
              className="chat-view__input ui-writing ui-focus-ring"
              placeholder="Ask about your notes"
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              onKeyDown={composerKeyDown}
            />
            {chat.turnActive ? (
              <Button type="button" variant="quiet" className="px-xs py-2xs" onClick={chat.stop}>
                Stop
              </Button>
            ) : (
              <Button type="submit" variant="primary">
                Send
              </Button>
            )}
          </form>
        )}
      </div>
    </ViewFrame>
  );
}

function ChatEntryRow({ entry }: { entry: ChatEntry }) {
  switch (entry.kind) {
    case "user":
      return (
        <div>
          <p className="font-mono text-eyebrow uppercase tracking-eyebrow text-text-faint">
            You
          </p>
          <p className="chat-view__user mt-3xs text-body text-text">{entry.text}</p>
        </div>
      );
    case "assistant":
      // No `.chat-view__answer` here, and that asymmetry with the streaming
      // block above is the design, not an oversight. `assistant_done` clears
      // the streamed text and appends this entry in the same update, so the
      // block unmounts and THIS div mounts in its place holding the same
      // prose. Giving it the entrance class would fade in an answer the reader
      // has already been reading, once per turn. Mounting at rest makes the
      // handoff invisible. Every entry in the log stays still for the same
      // reason scrollback must: none of them is an arrival.
      return (
        <div className="md-reading font-serif">
          <ReactMarkdown remarkPlugins={[remarkGfm]}>{entry.text}</ReactMarkdown>
        </div>
      );
    case "tool_use":
      return (
        <p className="font-mono text-meta text-text-faint">{entry.summary}</p>
      );
    case "permission":
      // The live card renders from the snapshot's pending permission; the
      // entry keeps the exchange's place in the flow once it resolves.
      if (entry.resolution === null) return null;
      return (
        <p className="font-mono text-meta text-text-faint">
          {entry.allowed ? "Allowed" : "Not allowed"} · {entry.question}
        </p>
      );
    case "error":
      return <StatusMessage variant="error">{entry.message}</StatusMessage>;
    default: {
      const exhausted: never = entry;
      return exhausted;
    }
  }
}

/**
 * The inline approval card for an MCP write tool — the headless analog of
 * Claude Code's TTY permission prompt. It sits in the conversation flow (not
 * an overlay: the question belongs to the exchange that raised it), and it
 * waits indefinitely; every non-answer path resolves it to deny.
 */
function PermissionCard({
  pending,
  respond,
}: {
  pending: PendingPermission;
  respond: (requestId: string, allow: boolean) => void;
}) {
  return (
    <div className="chat-view__card ui-raised">
      <p className="text-body text-text">{pending.question}</p>
      <p className="mt-3xs font-mono text-meta text-text-faint">{pending.tool}</p>
      <div className="mt-xs flex gap-2xs">
        <Button
          variant="primary"
          onClick={() => respond(pending.request_id, true)}
        >
          Allow
        </Button>
        <Button
          variant="quiet"
          className="px-xs py-2xs"
          onClick={() => respond(pending.request_id, false)}
        >
          Deny
        </Button>
      </div>
    </div>
  );
}
