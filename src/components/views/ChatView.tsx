import { useState, type FormEvent, type KeyboardEvent } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { ChatEntry, PendingPermission } from "../../chat";
import { useChatSession } from "../../useChatSession";
import { useScrollIntoView } from "../../useScrollIntoView";
import { Button } from "../ui/Button";
import { StatusMessage } from "../ui/StatusMessage";
import { ViewFrame } from "../ui/ViewFrame";
import "../markdownReading.css";
import "./ChatView.css";

/** The bottom sentinel the log keeps in view as the conversation grows. */
const LOG_END_ID = "chat-view-end";

/**
 * The designed chat over the knowledge base (FOUNDING_DOC §4: the front door;
 * the terminal is the escape hatch). Claude Code runs headless underneath,
 * grounded through the kodabi MCP tools; this view renders the conversation
 * in the app's two voices — your messages in the interface sans, answers on
 * the reading ramp (`.md-reading` serif, the same surface a note's body uses)
 * — with tool use as quiet mono lines and MCP write approvals as inline
 * cards.
 *
 * Streamed tokens just appear: no animation (docs/DESIGN_SYSTEM.md §4 — a
 * token feed is not a licence to animate), and the streamed prose is not a
 * live region; one offscreen `role="status"` line narrates the turn instead.
 */
export function ChatView() {
  const chat = useChatSession();
  const [draft, setDraft] = useState("");

  useScrollIntoView(
    LOG_END_ID,
    // Content growth is what should keep the end in view: new entries, more
    // streamed text, a card appearing, or the exit notice.
    `${chat.entries.length}:${chat.streamingText.length}:${
      chat.pending?.request_id ?? ""
    }:${chat.exited}`,
  );

  const submit = (event: FormEvent) => {
    event.preventDefault();
    const text = draft.trim();
    if (!text || chat.turnActive || chat.exited) return;
    chat.send(text);
    setDraft("");
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
            <div className="md-reading font-serif">
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
