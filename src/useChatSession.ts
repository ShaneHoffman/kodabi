import { useEffect, useRef, useState } from "react";
import {
  cancelChatTurn,
  openChat,
  respondChatPermission,
  restartChat,
  sendChatMessage,
  type ChatEntry,
  type ChatEventPayload,
  type ChatSnapshot,
  type PendingPermission,
} from "./chat";
import { CHAT_EVENT } from "./events";
import { useTauriEvent } from "./useTauriEvent";

/**
 * Bridge hook for the designed chat view's backend session — the one external
 * system it owns is the chat session living in `src-tauri/src/chat_cmds.rs`
 * (opened over IPC on mount, streamed back over `chat:event`).
 *
 * Mounting ensures the session is live (`chat_open` is idempotent, so a view
 * switch resumes the conversation) and hydrates from its structured snapshot,
 * mid-stream text and a still-open permission card included. Events for any
 * other `chat_id` are dropped: after a restart, the old session's stragglers
 * must not bleed into the new conversation.
 *
 * There is nothing to undo on unmount by design — the session outlives the
 * view (like the terminal's PTY), and `useTauriEvent` owns its own cleanup.
 */

type ChatSessionValue = {
  /** The first snapshot landed — until then the view renders nothing
   * (loading renders nothing, docs/DESIGN_SYSTEM.md §3). */
  ready: boolean;
  /** The conversation so far, oldest first. */
  entries: ChatEntry[];
  /** Assistant text still streaming in (not yet a completed entry). */
  streamingText: string;
  /** The open permission card, if the turn is blocked on one. */
  pending: PendingPermission | null;
  /** A turn is in flight (send accepted, `turn_done` not yet seen). */
  turnActive: boolean;
  /** The session's process exited; `restart` is the way forward. */
  exited: boolean;
  /** The session could not start at all ("Couldn't start chat" copy). */
  startError: string | null;
  send: (text: string) => void;
  stop: () => void;
  respondPermission: (requestId: string, allow: boolean) => void;
  restart: () => void;
};

type ChatSessionState = {
  chatId: string | null;
  entries: ChatEntry[];
  streamingText: string;
  pending: PendingPermission | null;
  turnActive: boolean;
  exited: boolean;
  startError: string | null;
};

const INITIAL_STATE: ChatSessionState = {
  chatId: null,
  entries: [],
  streamingText: "",
  pending: null,
  turnActive: false,
  exited: false,
  startError: null,
};

function fromSnapshot(snapshot: ChatSnapshot): ChatSessionState {
  return {
    chatId: snapshot.chat_id,
    entries: snapshot.entries,
    streamingText: snapshot.streaming_text ?? "",
    pending: snapshot.pending_permission,
    // A snapshot mid-stream implies an in-flight turn; a pending card also
    // means the turn is blocked (in flight) on it.
    turnActive:
      snapshot.running &&
      (snapshot.streaming_text !== null || snapshot.pending_permission !== null),
    exited: !snapshot.running,
    startError: null,
  };
}

/** Marks the last unresolved permission entry resolved — there is at most one
 * (the CLI blocks on its own request), so "the last" is "the one". */
function resolvePermissionEntry(
  entries: ChatEntry[],
  allowed: boolean,
  resolution: string,
): ChatEntry[] {
  let index = -1;
  for (let i = entries.length - 1; i >= 0; i--) {
    const candidate = entries[i];
    if (candidate.kind === "permission" && candidate.resolution === null) {
      index = i;
      break;
    }
  }
  if (index < 0) return entries;
  const entry = entries[index];
  if (entry.kind !== "permission") return entries;
  return [
    ...entries.slice(0, index),
    { ...entry, allowed, resolution },
    ...entries.slice(index + 1),
  ];
}

function applyEvent(
  state: ChatSessionState,
  payload: ChatEventPayload,
): ChatSessionState {
  switch (payload.type) {
    case "delta":
      return {
        ...state,
        streamingText: state.streamingText + payload.text,
        turnActive: true,
      };
    case "assistant_done":
      return {
        ...state,
        streamingText: "",
        entries: [...state.entries, { kind: "assistant", text: payload.text }],
      };
    case "tool_use":
      return {
        ...state,
        entries: [...state.entries, { kind: "tool_use", summary: payload.summary }],
      };
    case "permission_request":
      return {
        ...state,
        pending: {
          request_id: payload.request_id,
          tool: payload.tool,
          question: payload.question,
        },
        entries: [
          ...state.entries,
          {
            kind: "permission",
            question: payload.question,
            tool: payload.tool,
            allowed: null,
            resolution: null,
          },
        ],
      };
    case "permission_resolved":
      return {
        ...state,
        pending:
          state.pending?.request_id === payload.request_id ? null : state.pending,
        entries: resolvePermissionEntry(
          state.entries,
          payload.allowed,
          payload.resolution,
        ),
      };
    case "turn_done":
      return {
        ...state,
        turnActive: false,
        // A stopped or failed turn's partial stream is discarded — the
        // transcript keeps only completed blocks, and the view mirrors it.
        streamingText: "",
        entries: payload.error
          ? [
              ...state.entries,
              {
                kind: "error",
                message: `Couldn't finish the answer: ${payload.error}`,
              },
            ]
          : state.entries,
      };
    case "exited":
      return {
        ...state,
        exited: true,
        turnActive: false,
        streamingText: "",
        pending: null,
      };
    default: {
      const exhausted: never = payload;
      return exhausted;
    }
  }
}

export function useChatSession(): ChatSessionValue {
  const [state, setState] = useState<ChatSessionState>(INITIAL_STATE);
  // The event filter reads the live chat id through a ref assigned during
  // render, so the `useTauriEvent` subscription never churns on state changes.
  const chatIdRef = useRef<string | null>(null);
  chatIdRef.current = state.chatId;

  useTauriEvent<ChatEventPayload>(CHAT_EVENT, (payload) => {
    if (payload.chat_id !== chatIdRef.current) return;
    setState((prev) => applyEvent(prev, payload));
  });

  useEffect(() => {
    let active = true;
    openChat()
      .then((snapshot) => {
        if (active) setState(fromSnapshot(snapshot));
      })
      .catch((error: unknown) => {
        if (active) {
          setState((prev) => ({ ...prev, startError: String(error) }));
        }
      });
    return () => {
      active = false;
    };
  }, []);

  const send = (text: string) => {
    setState((prev) => ({
      ...prev,
      entries: [...prev.entries, { kind: "user", text }],
      turnActive: true,
    }));
    sendChatMessage(text).catch((error: unknown) => {
      setState((prev) => ({
        ...prev,
        turnActive: false,
        entries: [
          ...prev.entries,
          { kind: "error", message: `Couldn't send the message: ${String(error)}` },
        ],
      }));
    });
  };

  const stop = () => {
    void cancelChatTurn();
  };

  const respondPermission = (requestId: string, allow: boolean) => {
    // The card resolves through the `permission_resolved` event, so a race
    // with a stop or exit settles on whichever resolution the backend chose.
    void respondChatPermission(requestId, allow);
  };

  const restart = () => {
    restartChat()
      .then((snapshot) => setState(fromSnapshot(snapshot)))
      .catch((error: unknown) => {
        setState((prev) => ({ ...prev, startError: String(error) }));
      });
  };

  return {
    ready: state.chatId !== null,
    entries: state.entries,
    streamingText: state.streamingText,
    pending: state.pending,
    turnActive: state.turnActive,
    exited: state.exited,
    startError: state.startError,
    send,
    stop,
    respondPermission,
    restart,
  };
}
