import {
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent,
  type ReactNode,
  type UIEvent,
} from "react";
import { clsx } from "clsx";
import type { ChatEntry, ChatNoteRef, PendingPermission } from "../../chat";
import { citationsFor } from "../../chatCitations";
import { useChatSession } from "../../useChatSession";
import { useNavigation } from "../../useNavigation";
import { INBOX_PROJECT } from "../../useNotes";
import { folderHue, type FolderHue } from "../../useProjects";
import { useScrollIntoView } from "../../useScrollIntoView";
import { Button } from "../ui/Button";
import { StatusMessage } from "../ui/StatusMessage";
import { NoteMarkdown } from "./NoteMarkdown";
import { PANEL_EYEBROW } from "./SessionPanel";

/** The bottom sentinel the log keeps in view as the conversation grows. */
const LOG_END_ID = "chat-view-end";

/** How far above the bottom edge still counts as "reading the live end".
 * Within it, growth keeps the end in view; beyond it, the user has scrolled
 * up into the backlog and streaming must not yank them back down. */
const FOLLOW_SLACK_PX = 40;

/** Written out rather than interpolated, so Tailwind's scanner sees each class
 * as a literal and emits it (the `HUE_DOT` precedent in Dock and the palette). */
const HUE_DOT: Record<FolderHue, string> = {
  coral: "bg-coral",
  cobalt: "bg-cobalt",
  teal: "bg-teal",
  plum: "bg-plum",
};

/** An answer's prose: the one thing on this screen with a measure.
 *
 * The conversation and the composer span the panel, because they are structure;
 * sentences stop at 78ch (docs/DESIGN_SYSTEM.md §2). Markdown's first block
 * loses its top margin so the gap under the eyebrow is the one set here rather
 * than that plus a paragraph's lead. */
const ANSWER_PROSE =
  "mt-2 max-w-[78ch] font-ui text-[13.5px] leading-[1.65] text-ink-read [&>*:first-child]:mt-0";

/** A quiet mono line: a tool that ran, or a permission once it resolved. Data
 * about the conversation rather than part of it, so it sits at the metadata
 * register and never at ink strength. */
const LOG_ASIDE = "font-data text-[11px] text-ink-faint";

/**
 * The designed chat over the knowledge base (FOUNDING_DOC §4: the front door;
 * the terminal is the escape hatch). Claude Code runs headless underneath,
 * grounded through the kodabi MCP tools; this view renders the conversation in
 * the app's two voices — your messages as glass bubbles on the right, answers
 * as open text on the left under the system's own eyebrow — with tool use as
 * quiet mono lines and MCP write approvals as inline cards.
 *
 * A token feed is still not a licence to animate (docs/DESIGN_SYSTEM.md §4):
 * nothing here reacts to a delta. What does animate is the live answer block's
 * one-shot arrival — the `data-live-answer` block fades and rises in as it
 * first appears, because an answer materialising fully formed reads as breakage
 * rather than as arriving. The completed entry it hands off to is deliberately
 * left unmarked (see `AnswerEntry`), so the handoff is invisible.
 *
 * The streamed prose is not a live region; one offscreen `role="status"` line
 * narrates the turn instead.
 */
export function ChatView() {
  const chat = useChatSession();
  const { navigate } = useNavigation();
  const [draft, setDraft] = useState("");
  // The composer field, so a press on the bar's padding can hand it the caret.
  const composerRef = useRef<HTMLTextAreaElement>(null);
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

  const openNote = (note: ChatNoteRef) =>
    navigate({
      kind: "noteEditor",
      noteId: note.id,
      project: note.project ?? INBOX_PROJECT,
      // Back returns to the conversation that cited it. The session outlives
      // the view (`chat_open` is idempotent), so the round trip costs nothing.
      origin: { kind: "chat" },
    });

  // Loading renders nothing (docs/DESIGN_SYSTEM.md §3) — but a session that
  // could not start at all must still say so.
  if (!chat.ready) {
    return (
      <ChatFrame>
        {chat.startError && (
          <div className="flex flex-col items-start gap-2">
            {/* The whole sentence, unprefixed: `chat_cmds` words each start
                failure (`user_errors`), and it hedges for the same reason a
                second line used to here. `chat_open` fails for more than a
                missing CLI, so the copy names the CLI as the likely cause
                without claiming it is the only one. */}
            <StatusMessage variant="error">{chat.startError}</StatusMessage>
            {/* The composer lives below this early return, so without a control
                here the screen is the app's one dead end: the same `restart`
                the exited state offers is the way out (DESIGN_SYSTEM §3 leaves
                the data reachable). */}
            <Button className="mt-1" onClick={chat.restart}>
              Try again
            </Button>
          </div>
        )}
      </ChatFrame>
    );
  }

  const empty =
    chat.entries.length === 0 && !chat.streamingText && !chat.exited;
  // Mid-turn with nothing yet to show: the model is thinking, or a read tool is
  // running. The answer's eyebrow arrives early and pulses until prose replaces
  // it — ink, never green, which belongs to the kodama and the caret.
  const thinking = chat.turnActive && !chat.streamingText && !chat.pending;

  return (
    <ChatFrame>
      <p className="sr-only" role="status">
        {chat.turnActive ? "Claude is answering" : ""}
      </p>
      <div
        className="mt-[26px] flex min-h-0 flex-1 flex-col gap-[22px] overflow-y-auto"
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
          <ChatEntryRow
            key={index}
            entry={entry}
            citations={
              entry.kind === "assistant"
                ? citationsFor(chat.entries, index)
                : []
            }
            openNote={openNote}
          />
        ))}
        {chat.streamingText && (
          <div
            data-live-answer
            className={clsx(
              "transition-[opacity,translate] duration-220 ease-out-strong",
              "starting:opacity-0 starting:translate-y-2",
              "motion-reduce:starting:translate-y-0",
            )}
          >
            <p className={PANEL_EYEBROW}>kodabi</p>
            <div className={ANSWER_PROSE}>
              <NoteMarkdown body={chat.streamingText} />
            </div>
          </div>
        )}
        {thinking && (
          <p
            data-testid="chat-thinking"
            aria-hidden="true"
            className={clsx(PANEL_EYEBROW, "animate-pending")}
          >
            kodabi
          </p>
        )}
        {chat.pending && (
          <PermissionCard
            pending={chat.pending}
            respond={chat.respondPermission}
          />
        )}
        {chat.exited && (
          // The region is the sentence, not the row: the variant carries the
          // `role="status"` this div used to hold by hand, and the restart
          // control stays outside it. Full size rather than `compact`, to sit
          // in the same register as the empty state above it — both are the
          // conversation column speaking about the session, not a row's line.
          <div className="flex items-baseline gap-3">
            <StatusMessage variant="status">
              Claude Code exited. Start a new chat to continue.
            </StatusMessage>
            <Button onClick={chat.restart}>Start a new chat</Button>
          </div>
        )}
        <div id={LOG_END_ID} />
      </div>

      {!chat.exited && (
        <form
          onSubmit={submit}
          // The bar's padding is part of the field: a press on it puts the
          // caret in the textarea rather than dying in a dead strip. Only the
          // form's own box qualifies — a press on the textarea or the button is
          // already going where it should. preventDefault first, or the press
          // hands focus to the form and takes it straight back.
          onPointerDown={(event) => {
            if (event.target !== event.currentTarget) return;
            event.preventDefault();
            composerRef.current?.focus();
          }}
          className={clsx(
            "mt-[30px] flex flex-none items-end gap-3 rounded-card border border-edge bg-wash p-2 pl-4",
            "shadow-[inset_0_1px_0_var(--color-edge-lit)]",
            // The border step and the green caret are the focus signal, as in
            // the search field — a ring around a bar this wide would be noise.
            "transition-[border-color] duration-140 ease-out-strong focus-within:border-ink-faint",
          )}
        >
          <textarea
            ref={composerRef}
            aria-label="Message Claude"
            className={clsx(
              // The vertical padding is the button's, not the field's: at one
              // line the textarea and the button are the same height, so
              // `items-end` above IS centred, and the bar's breathing room is
              // the form's `p-2`. Grow the textarea and the button stays
              // bottom-pinned at that same inset (docs/DESIGN_SYSTEM.md §2).
              "min-w-0 flex-1 resize-none bg-transparent py-1",
              // Grows with what you type, then scrolls rather than eating the
              // conversation.
              "[field-sizing:content] max-h-[200px] overflow-y-auto",
              "font-ui text-[13.5px] leading-[1.55] text-ink caret-kodama outline-hidden",
              "placeholder:text-ink-faint",
            )}
            placeholder="Ask about your notes"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={composerKeyDown}
          />
          {chat.turnActive ? (
            <Button type="button" variant="quiet" onClick={chat.stop}>
              Stop
            </Button>
          ) : (
            <Button type="submit">Send</Button>
          )}
        </form>
      )}
    </ChatFrame>
  );
}

/** The screen itself: header, then a column that gives the log every pixel the
 * composer does not take. It fills the panel rather than sitting in a measured
 * column, which is why this is not `ViewFrame` — the conversation is structure
 * (docs/DESIGN_SYSTEM.md §2), and only the answer prose stops at a measure. */
function ChatFrame({ children }: { children: ReactNode }) {
  return (
    <section
      aria-label="Chat"
      className="flex h-full min-h-0 flex-col px-10 py-[34px]"
    >
      <header className="flex flex-col gap-1.5">
        <p className={PANEL_EYEBROW}>Claude</p>
        <h2 className="text-[26px] font-semibold leading-[1.15] tracking-[-0.01em] text-ink">
          Chat
        </h2>
      </header>
      {children}
    </section>
  );
}

function ChatEntryRow({
  entry,
  citations,
  openNote,
}: {
  entry: ChatEntry;
  citations: ChatNoteRef[];
  openNote: (note: ChatNoteRef) => void;
}) {
  switch (entry.kind) {
    case "user":
      // No label: the side of the panel it sits on and the tail on its corner
      // say whose it is, and a bubble captioned "You" would say it twice.
      return (
        <p
          className={clsx(
            // `break-words` is load-bearing next to `pre-wrap`, not tidiness:
            // pre-wrap keeps the typed line breaks but leaves overflow-wrap at
            // `normal`, so one unbroken token — a pasted path or URL, which is
            // exactly what you paste into a chat about your notes — paints
            // outside the bubble's border and hands the conversation a
            // horizontal scrollbar (the log's overflow-x computes to `auto`
            // off its overflow-y). The 55% cap is what puts that within reach
            // of an ordinary message.
            "max-w-[55%] self-end whitespace-pre-wrap break-words",
            // A rectangle with one corner drawn in: the tail points back at
            // the composer the message was typed in.
            "rounded-[14px] rounded-br-[4px] border border-edge bg-wash",
            "shadow-[inset_0_1px_0_var(--color-edge-lit)]",
            "px-[15px] py-[11px] font-ui text-[13.5px] leading-[1.55] text-ink",
          )}
        >
          {entry.text}
        </p>
      );
    case "assistant":
      return <AnswerEntry text={entry.text} citations={citations} openNote={openNote} />;
    case "tool_use":
      return <p className={LOG_ASIDE}>{entry.summary}</p>;
    case "permission":
      // The live card renders from the snapshot's pending permission; the
      // entry keeps the exchange's place in the flow once it resolves.
      if (entry.resolution === null) return null;
      return (
        <p className={LOG_ASIDE}>
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

/** A settled answer: open text on the left, and the notes it drew on beneath.
 *
 * No `data-live-answer` here, and that asymmetry with the streaming block is
 * the design, not an oversight. `assistant_done` clears the streamed text and
 * appends this entry in the same update, so the block unmounts and this one
 * mounts in its place holding the same prose. Giving it the entrance would fade
 * in an answer the reader has already been reading, once per turn. Mounting at
 * rest makes the handoff invisible. Every entry in the log stays still for the
 * same reason scrollback must: none of them is an arrival. */
function AnswerEntry({
  text,
  citations,
  openNote,
}: {
  text: string;
  citations: ChatNoteRef[];
  openNote: (note: ChatNoteRef) => void;
}) {
  return (
    <div>
      <p className={PANEL_EYEBROW}>kodabi</p>
      <div className={ANSWER_PROSE}>
        <NoteMarkdown body={text} />
      </div>
      {citations.length > 0 && (
        <div className="mt-2.5 flex flex-wrap gap-2">
          {citations.map((note) => (
            // A pill, because a citation is a token that opens a thing rather
            // than an action performed on one (docs/DESIGN_SYSTEM.md §2).
            <Button
              key={note.id}
              variant="pill"
              onClick={() => openNote(note)}
            >
              <span
                aria-hidden="true"
                className={clsx(
                  "size-[7px] flex-none rounded-[2px]",
                  // The inbox is not a project and has no hue of its own.
                  note.project ? HUE_DOT[folderHue(note.project)] : "bg-ink-faint",
                )}
              />
              {note.title}
            </Button>
          ))}
        </div>
      )}
    </div>
  );
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
    <div className="glass-card self-start px-4 py-3.5">
      <p className="text-[13.5px] text-ink">{pending.question}</p>
      <p className={clsx("mt-1", LOG_ASIDE)}>{pending.tool}</p>
      <div className="mt-3 flex gap-2">
        <Button onClick={() => respond(pending.request_id, true)}>Allow</Button>
        <Button variant="quiet" onClick={() => respond(pending.request_id, false)}>
          Deny
        </Button>
      </div>
    </div>
  );
}
