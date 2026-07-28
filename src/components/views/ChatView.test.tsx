import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => import("../../test/tauri"));
vi.mock("@tauri-apps/api/event", () => import("../../test/tauri"));

import {
  emitFromBackend,
  invoke,
  invokedCommands,
  onCommand,
  resetTauriMocks,
} from "../../test/tauri";
import type { ChatSnapshot } from "../../chat";
import { ChatView } from "./ChatView";

const CHAT_ID = "3c9a35a1-0000-4000-8000-000000000000";

function snapshot(overrides: Partial<ChatSnapshot> = {}): ChatSnapshot {
  return {
    chat_id: CHAT_ID,
    running: true,
    turn_active: false,
    entries: [],
    streaming_text: null,
    pending_permission: null,
    ...overrides,
  };
}

/** Mount and let the `chat_open` snapshot land (loading renders nothing). */
async function renderChat() {
  render(<ChatView />);
  await waitFor(() => expect(invokedCommands()).toContain("chat_open"));
  await screen.findByRole("region", { name: "Chat" });
}

describe("ChatView", () => {
  beforeEach(() => {
    resetTauriMocks();
    onCommand("chat_open", () => snapshot());
    onCommand("chat_send", () => undefined);
    onCommand("chat_cancel", () => undefined);
    onCommand("chat_permission_respond", () => undefined);
    onCommand("chat_restart", () => snapshot());
  });

  it("opens the session on mount and shows the first-run empty state", async () => {
    await renderChat();
    expect(
      screen.getByText(/Ask about your notes\. Claude can search/),
    ).toBeInTheDocument();
  });

  it("hydrates the conversation from the snapshot", async () => {
    onCommand("chat_open", () =>
      snapshot({
        entries: [
          { kind: "user", text: "What projects do I have?" },
          { kind: "tool_use", summary: "Listed projects" },
          { kind: "assistant", text: "You have **Briarwood Golf**." },
        ],
        streaming_text: "And one more thing",
        turn_active: true,
      }),
    );
    await renderChat();

    expect(screen.getByText("What projects do I have?")).toBeInTheDocument();
    expect(screen.getByText("Listed projects")).toBeInTheDocument();
    // Markdown renders: the bold survives as an element, not literal asterisks.
    expect(screen.getByText("Briarwood Golf")).toBeInTheDocument();
    expect(screen.getByText("And one more thing")).toBeInTheDocument();
  });

  it("hydrates an in-flight turn as stoppable even with nothing streaming", async () => {
    // Mid-turn there may be no streamed text and no pending card (the model
    // thinking, a read tool running): the snapshot's explicit turn_active is
    // what must keep Stop up instead of re-enabling Send.
    onCommand("chat_open", () =>
      snapshot({
        entries: [{ kind: "user", text: "What projects do I have?" }],
        turn_active: true,
      }),
    );
    await renderChat();

    expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Send" })).not.toBeInTheDocument();
  });

  it("sends the draft and renders it as a user turn", async () => {
    const user = userEvent.setup();
    await renderChat();

    await user.type(screen.getByRole("textbox", { name: "Message Claude" }), "hi");
    await user.click(screen.getByRole("button", { name: "Send" }));

    expect(invoke).toHaveBeenCalledWith("chat_send", { text: "hi" });
    expect(screen.getByText("hi")).toBeInTheDocument();
    // The turn is now in flight: the action flips to Stop.
    expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
  });

  it("Enter submits and Shift+Enter does not", async () => {
    const user = userEvent.setup();
    await renderChat();
    const composer = screen.getByRole("textbox", { name: "Message Claude" });

    await user.type(composer, "line one{Shift>}{Enter}{/Shift}line two");
    expect(invokedCommands()).not.toContain("chat_send");

    await user.type(composer, "{Enter}");
    expect(invoke).toHaveBeenCalledWith("chat_send", {
      text: "line one\nline two",
    });
  });

  it("grows streamed deltas into prose and settles on the completed block", async () => {
    await renderChat();

    act(() => {
      emitFromBackend("chat:event", { type: "delta", chat_id: CHAT_ID, text: "The answer " });
      emitFromBackend("chat:event", { type: "delta", chat_id: CHAT_ID, text: "is 42." });
    });
    expect(screen.getByText("The answer is 42.")).toBeInTheDocument();

    act(() => {
      emitFromBackend("chat:event", {
        type: "assistant_done",
        chat_id: CHAT_ID,
        text: "The answer is 42, obviously.",
      });
      emitFromBackend("chat:event", {
        type: "turn_done",
        chat_id: CHAT_ID,
        stopped: false,
        error: null,
      });
    });
    expect(screen.getByText("The answer is 42, obviously.")).toBeInTheDocument();
    expect(screen.queryByText("The answer is 42.")).not.toBeInTheDocument();
  });

  it("drops events from another session's chat_id", async () => {
    await renderChat();
    act(() => {
      emitFromBackend("chat:event", {
        type: "delta",
        chat_id: "someone-else",
        text: "stray",
      });
    });
    expect(screen.queryByText("stray")).not.toBeInTheDocument();
  });

  it("renders tool use as a quiet status line", async () => {
    await renderChat();
    act(() => {
      emitFromBackend("chat:event", {
        type: "tool_use",
        chat_id: CHAT_ID,
        summary: 'Searched notes for "budget"',
      });
    });
    expect(screen.getByText('Searched notes for "budget"')).toBeInTheDocument();
  });

  it("answers a permission card with Allow", async () => {
    const user = userEvent.setup();
    await renderChat();

    act(() => {
      emitFromBackend("chat:event", {
        type: "permission_request",
        chat_id: CHAT_ID,
        request_id: "req-1",
        tool: "mcp__kodabi__add_glossary_term",
        question: 'Add glossary term "AEC" to Briarwood Golf?',
      });
    });
    expect(
      screen.getByText('Add glossary term "AEC" to Briarwood Golf?'),
    ).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Allow" }));
    expect(invoke).toHaveBeenCalledWith("chat_permission_respond", {
      requestId: "req-1",
      allow: true,
    });

    // The backend answers with the resolution; the card collapses to a line.
    act(() => {
      emitFromBackend("chat:event", {
        type: "permission_resolved",
        chat_id: CHAT_ID,
        request_id: "req-1",
        allowed: true,
        resolution: "user",
      });
    });
    expect(screen.queryByRole("button", { name: "Allow" })).not.toBeInTheDocument();
    expect(screen.getByText(/^Allowed ·/)).toBeInTheDocument();
  });

  it("answers a permission card with Deny", async () => {
    const user = userEvent.setup();
    await renderChat();

    act(() => {
      emitFromBackend("chat:event", {
        type: "permission_request",
        chat_id: CHAT_ID,
        request_id: "req-2",
        tool: "mcp__kodabi__file_note_to_project",
        question: "File note n_a1b2c3 to Briarwood Golf?",
      });
    });
    await user.click(screen.getByRole("button", { name: "Deny" }));
    expect(invoke).toHaveBeenCalledWith("chat_permission_respond", {
      requestId: "req-2",
      allow: false,
    });

    act(() => {
      emitFromBackend("chat:event", {
        type: "permission_resolved",
        chat_id: CHAT_ID,
        request_id: "req-2",
        allowed: false,
        resolution: "user",
      });
    });
    expect(screen.getByText(/^Not allowed ·/)).toBeInTheDocument();
  });

  it("stops an in-flight turn and discards the partial stream", async () => {
    const user = userEvent.setup();
    await renderChat();

    await user.type(screen.getByRole("textbox", { name: "Message Claude" }), "go");
    await user.type(
      screen.getByRole("textbox", { name: "Message Claude" }),
      "{Enter}",
    );
    act(() => {
      emitFromBackend("chat:event", { type: "delta", chat_id: CHAT_ID, text: "partial" });
    });

    await user.click(screen.getByRole("button", { name: "Stop" }));
    expect(invokedCommands()).toContain("chat_cancel");

    act(() => {
      emitFromBackend("chat:event", {
        type: "turn_done",
        chat_id: CHAT_ID,
        stopped: true,
        error: null,
      });
    });
    // A stop is not a failure, and the discarded partial is gone.
    expect(screen.queryByText("partial")).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Send" })).toBeInTheDocument();
  });

  it("surfaces a failed turn as a prefixed error line", async () => {
    await renderChat();
    act(() => {
      emitFromBackend("chat:event", {
        type: "turn_done",
        chat_id: CHAT_ID,
        stopped: false,
        error: "usage limit reached",
      });
    });
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Couldn't finish the answer: usage limit reached",
    );
  });

  it("offers a new chat after the process exits", async () => {
    const user = userEvent.setup();
    await renderChat();

    act(() => {
      emitFromBackend("chat:event", { type: "exited", chat_id: CHAT_ID, code: 1 });
    });
    expect(screen.getByText(/Claude Code exited/)).toBeInTheDocument();
    // The composer is gone; the restart affordance is the way forward.
    expect(
      screen.queryByRole("textbox", { name: "Message Claude" }),
    ).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Start a new chat" }));
    expect(invokedCommands()).toContain("chat_restart");
    await waitFor(() =>
      expect(screen.queryByText(/Claude Code exited/)).not.toBeInTheDocument(),
    );
    expect(
      screen.getByRole("textbox", { name: "Message Claude" }),
    ).toBeInTheDocument();
  });

  it("says what failed when the session cannot start", async () => {
    onCommand("chat_open", () => {
      throw new Error("claude not found");
    });
    render(<ChatView />);

    expect(
      await screen.findByText(/Couldn't start chat: .*claude not found/),
    ).toBeInTheDocument();
  });
});
