import { invoke } from "@tauri-apps/api/core";

/**
 * Typed callers for the designed chat view's Tauri commands. Wire types mirror
 * the serde DTOs in `src-tauri/src/chat_cmds.rs`; the invoke strings equal the
 * Rust command names exactly (`.claude/rules/tauri-command-parity.md`).
 */

/** Mirrors `ChatEntryDto` in `src-tauri/src/chat_cmds.rs`. One rendered entry
 * of the conversation log. A `permission` entry's `allowed`/`resolution` are
 * null while the card waits (the live card itself is
 * `ChatSnapshot.pending_permission`) and set once it resolves. */
export type ChatEntry =
  | { kind: "user"; text: string }
  | { kind: "assistant"; text: string }
  | { kind: "tool_use"; summary: string }
  | {
      kind: "permission";
      question: string;
      tool: string;
      allowed: boolean | null;
      resolution: string | null;
    }
  | { kind: "error"; message: string };

/** Mirrors `PendingPermissionDto` in `src-tauri/src/chat_cmds.rs`. */
export type PendingPermission = {
  request_id: string;
  tool: string;
  question: string;
};

/** Mirrors `ChatSnapshot` in `src-tauri/src/chat_cmds.rs`. Seeds a freshly
 * mounted chat view, including mid-stream text and a still-open card. */
export type ChatSnapshot = {
  chat_id: string;
  running: boolean;
  entries: ChatEntry[];
  streaming_text: string | null;
  pending_permission: PendingPermission | null;
};

/** How a permission card was resolved. Mirrors
 * `kodabi_core::chat::PermissionResolution`. */
export type PermissionResolution = "user" | "cancelled" | "session_closed";

/** Mirrors `ChatEventPayload` in `src-tauri/src/chat_cmds.rs`. */
export type ChatEventPayload =
  | { type: "delta"; chat_id: string; text: string }
  | { type: "assistant_done"; chat_id: string; text: string }
  | { type: "tool_use"; chat_id: string; summary: string }
  | {
      type: "permission_request";
      chat_id: string;
      request_id: string;
      tool: string;
      question: string;
    }
  | {
      type: "permission_resolved";
      chat_id: string;
      request_id: string;
      allowed: boolean;
      resolution: PermissionResolution;
    }
  | { type: "turn_done"; chat_id: string; stopped: boolean; error: string | null }
  | { type: "exited"; chat_id: string; code: number | null };

/** Ensures a live chat session and returns a snapshot to hydrate the view.
 * Idempotent: reuses the running session so a view switch does not restart
 * the conversation. */
export function openChat(): Promise<ChatSnapshot> {
  return invoke<ChatSnapshot>("chat_open");
}

/** Sends a user message; the answer streams back as `chat:event`s. */
export function sendChatMessage(text: string): Promise<void> {
  return invoke<void>("chat_send", { text });
}

/** Stops the in-flight turn (any open permission card resolves to deny). */
export function cancelChatTurn(): Promise<void> {
  return invoke<void>("chat_cancel");
}

/** Answers the pending permission card. */
export function respondChatPermission(
  requestId: string,
  allow: boolean,
): Promise<void> {
  return invoke<void>("chat_permission_respond", { requestId, allow });
}

/** Reaps the current session and starts a fresh conversation (the "Start a
 * new chat" action after an exit). */
export function restartChat(): Promise<ChatSnapshot> {
  return invoke<ChatSnapshot>("chat_restart");
}
