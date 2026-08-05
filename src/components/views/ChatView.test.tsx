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
import type { ChatEntry, ChatNoteRef, ChatSnapshot } from "../../chat";
import { citationsFor } from "../../chatCitations";
import { NavigationContext, type View } from "../../useNavigation";
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

/** A tool-use entry. Most calls cite nothing; a `get_note` carries the note it
 * read, which is what the citation chips render. */
function toolUse(summary: string, note: ChatNoteRef | null = null): ChatEntry {
  return { kind: "tool_use", summary, note };
}

function noteRef(overrides: Partial<ChatNoteRef> = {}): ChatNoteRef {
  return {
    id: "n_a1b2c3",
    title: "Deck railing material decision",
    project: "Briarwood Golf",
    ...overrides,
  };
}

/** Where a citation chip sent the reader. ChatView navigates, so every render
 * needs the context; the spy is what the chip tests assert on. */
let navigate: ReturnType<typeof vi.fn<(view: View) => void>>;

/** Mount and let the `chat_open` snapshot land (loading renders nothing).
 * Returns the render result so the motion tests can reach for the entrance
 * marker by selector — there is no accessible handle on a wrapper div. */
async function renderChat() {
  const view = render(
    <NavigationContext.Provider value={{ view: { kind: "chat" }, navigate }}>
      <ChatView />
    </NavigationContext.Provider>,
  );
  await waitFor(() => expect(invokedCommands()).toContain("chat_open"));
  await screen.findByRole("region", { name: "Chat" });
  return view;
}

describe("ChatView", () => {
  beforeEach(() => {
    navigate = vi.fn<(view: View) => void>();
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
          toolUse("Listed projects"),
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

  it("lets a user message break a token too long for its bubble", async () => {
    // jsdom has no layout, so what this locks in is the class contract, the
    // same way the entrance tests below do. It matters because the two classes
    // are only correct together: `pre-wrap` keeps the typed line breaks and
    // leaves overflow-wrap at `normal`, so without `break-words` a pasted path
    // paints outside the bubble's 55% cap and gives the log a horizontal
    // scrollbar. Measured before the fix: a 63-character path ran 465px wide
    // in a 383px content box.
    const PASTED_PATH = "C:\\Users\\example\\repos\\kodabi\\docs\\FOUNDING_DOC.md";
    onCommand("chat_open", () =>
      snapshot({ entries: [{ kind: "user", text: PASTED_PATH }] }),
    );
    await renderChat();

    const bubble = screen.getByText(PASTED_PATH);
    expect(bubble).toHaveClass("whitespace-pre-wrap");
    expect(bubble).toHaveClass("break-words");
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

  // ---------- the answer's entrance (docs/DESIGN_SYSTEM.md §4) ----------
  // jsdom runs neither @starting-style nor transitions, so what these lock in
  // is the class contract: which element carries `[data-live-answer]`, and
  // just as importantly which is denied it. That IS the design — see the
  // comment on ChatEntryRow's assistant case.

  it("marks the arriving answer and keeps the same node across deltas", async () => {
    const { container } = await renderChat();
    expect(container.querySelector("[data-live-answer]")).toBeNull();

    act(() => {
      emitFromBackend("chat:event", {
        type: "delta",
        chat_id: CHAT_ID,
        text: "Streaming ",
      });
    });
    const arriving = container.querySelector("[data-live-answer]");
    expect(arriving).toHaveTextContent("Streaming");

    act(() => {
      emitFromBackend("chat:event", {
        type: "delta",
        chat_id: CHAT_ID,
        text: "onward.",
      });
    });
    // The same DOM node, grown — not a replacement. @starting-style resolves
    // only on insertion, so a fresh node here would re-run the entrance on
    // every delta, which at the backend's 16ms coalesce window is ~60Hz.
    expect(container.querySelector("[data-live-answer]")).toBe(arriving);
    expect(arriving).toHaveTextContent("Streaming onward.");
  });

  it("does not animate the completed answer the stream hands off to", async () => {
    const { container } = await renderChat();

    act(() => {
      emitFromBackend("chat:event", {
        type: "delta",
        chat_id: CHAT_ID,
        text: "The answer is 42.",
      });
    });
    expect(container.querySelector("[data-live-answer]")).not.toBeNull();

    act(() => {
      emitFromBackend("chat:event", {
        type: "assistant_done",
        chat_id: CHAT_ID,
        text: "The answer is 42.",
      });
    });
    // `assistant_done` unmounts the streaming block and mounts a different div
    // holding the same prose. If that entry carried the entrance class, every
    // turn would end by fading in an answer the reader has already read.
    expect(screen.getByText("The answer is 42.")).toBeInTheDocument();
    expect(container.querySelector("[data-live-answer]")).toBeNull();
  });

  it("marks each answer block a tool-using turn hands back", async () => {
    // The other half of the contract, and the reason §4 says "once per
    // assistant block" rather than "once per turn": a turn that stops to call
    // a tool comes back as prose, a tool line, then more prose. The second
    // block is a fresh node, so it gets its own entrance — which is one arrival
    // apiece, not a stagger.
    const { container } = await renderChat();

    act(() => {
      emitFromBackend("chat:event", {
        type: "delta",
        chat_id: CHAT_ID,
        text: "Let me look.",
      });
    });
    const first = container.querySelector("[data-live-answer]");
    expect(first).not.toBeNull();

    // One `act` per delivery, because that is what the backend does: the pump
    // flushes pending deltas before it emits `assistant_done`, and the next
    // block's first delta is at least a COALESCE_MS window later
    // (`chat_cmds.rs`). So React commits an empty `streamingText` in between
    // and the block genuinely unmounts. Batched into one `act`, it would never
    // leave the DOM and this would silently assert nothing.
    act(() => {
      emitFromBackend("chat:event", {
        type: "assistant_done",
        chat_id: CHAT_ID,
        text: "Let me look.",
      });
      emitFromBackend("chat:event", {
        type: "tool_use",
        chat_id: CHAT_ID,
        summary: "Searched notes",
        note: null,
      });
    });
    expect(container.querySelector("[data-live-answer]")).toBeNull();

    act(() => {
      emitFromBackend("chat:event", {
        type: "delta",
        chat_id: CHAT_ID,
        text: "You have two.",
      });
    });
    const second = container.querySelector("[data-live-answer]");
    expect(second).toHaveTextContent("You have two.");
    expect(second).not.toBe(first);
    // Still exactly one live block: the first one settled into a still entry.
    expect(container.querySelectorAll("[data-live-answer]")).toHaveLength(1);
  });

  it("leaves a backlog hydrated from the snapshot entirely still", async () => {
    onCommand("chat_open", () =>
      snapshot({
        entries: [
          { kind: "user", text: "What projects do I have?" },
          toolUse("Listed projects"),
          { kind: "assistant", text: "You have **Briarwood Golf**." },
        ],
      }),
    );
    const { container } = await renderChat();

    // Why "scrollback never animates" is provable rather than asserted: no
    // entry carries the class, so returning to the Chat tab has nothing to
    // replay.
    expect(container.querySelectorAll("[data-live-answer]")).toHaveLength(0);
  });

  it("still marks a mid-turn answer hydrated from the snapshot", async () => {
    // Accepted by design: remounting mid-turn (leaving Chat and coming back)
    // replays the entrance on the live answer alone, while the backlog around
    // it paints at rest. That answer genuinely is still arriving, and the
    // motion marks which part of the view is live.
    onCommand("chat_open", () =>
      snapshot({
        entries: [{ kind: "user", text: "What projects do I have?" }],
        streaming_text: "You have",
        turn_active: true,
      }),
    );
    const { container } = await renderChat();

    const arriving = container.querySelectorAll("[data-live-answer]");
    expect(arriving).toHaveLength(1);
    expect(arriving[0]).toHaveTextContent("You have");
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
        note: null,
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
    render(
      <NavigationContext.Provider value={{ view: { kind: "chat" }, navigate }}>
        <ChatView />
      </NavigationContext.Provider>,
    );

    expect(
      await screen.findByText(/Couldn't start chat: .*claude not found/),
    ).toBeInTheDocument();
  });

  // ---------- citations ----------

  it("chips the notes an answer read and opens one on click", async () => {
    const user = userEvent.setup();
    onCommand("chat_open", () =>
      snapshot({
        entries: [
          { kind: "user", text: "What did we decide about the deck railing?" },
          toolUse("Opened note n_a1b2c3", noteRef()),
          { kind: "assistant", text: "You are leaning toward aluminum." },
        ],
      }),
    );
    await renderChat();

    await user.click(
      screen.getByRole("button", { name: "Deck railing material decision" }),
    );

    // `origin` is what makes the editor's back link read "← Chat" and return
    // to this conversation (NoteEditorView's backTarget).
    expect(navigate).toHaveBeenCalledWith({
      kind: "noteEditor",
      noteId: "n_a1b2c3",
      project: "Briarwood Golf",
      origin: { kind: "chat" },
    });
  });

  it("files a citation with no project under the inbox", async () => {
    const user = userEvent.setup();
    onCommand("chat_open", () =>
      snapshot({
        entries: [
          { kind: "user", text: "What did I capture yesterday?" },
          toolUse("Opened note n_x", noteRef({ id: "n_x", project: null })),
          { kind: "assistant", text: "A note about the railing." },
        ],
      }),
    );
    await renderChat();

    await user.click(
      screen.getByRole("button", { name: "Deck railing material decision" }),
    );

    expect(navigate).toHaveBeenCalledWith(
      expect.objectContaining({ noteId: "n_x", project: "Inbox" }),
    );
  });

  it("chips only the notes actually read", async () => {
    onCommand("chat_open", () =>
      snapshot({
        entries: [
          { kind: "user", text: "What did we decide?" },
          // A search sees notes it may never draw on, so it carries no note
          // and earns no chip.
          toolUse('Searched notes for "railing"'),
          { kind: "assistant", text: "Aluminum." },
        ],
      }),
    );
    await renderChat();

    expect(screen.getByText('Searched notes for "railing"')).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /Deck railing/ }),
    ).not.toBeInTheDocument();
  });

  it("attributes each answer only the notes read for it", () => {
    const first = noteRef();
    const second = noteRef({ id: "n_d4e5f6", title: "Call with Dana" });
    const entries: ChatEntry[] = [
      { kind: "user", text: "One?" },
      toolUse("Opened note n_a1b2c3", first),
      { kind: "assistant", text: "Answer one." },
      { kind: "user", text: "Two?" },
      toolUse("Opened note n_d4e5f6", second),
      { kind: "assistant", text: "Answer two." },
    ];

    expect(citationsFor(entries, 2)).toEqual([first]);
    // The run stops at the turn boundary: the first answer's reads do not
    // leak onto the second.
    expect(citationsFor(entries, 5)).toEqual([second]);
  });

  it("cites a note once however often it was read", () => {
    const note = noteRef();
    const entries: ChatEntry[] = [
      { kind: "user", text: "What did we decide?" },
      toolUse("Opened note n_a1b2c3", note),
      // A permission card mid-run does not end the run: the tools still ran
      // for this answer.
      {
        kind: "permission",
        question: "File note n_a1b2c3 to Briarwood Golf?",
        tool: "file_note_to_project",
        allowed: true,
        resolution: "user",
      },
      toolUse("Opened note n_a1b2c3", note),
      { kind: "assistant", text: "Aluminum." },
    ];

    expect(citationsFor(entries, 4)).toEqual([note]);
  });

  // ---------- the in-flight pulse ----------

  it("pulses while a turn runs with nothing yet to show", async () => {
    onCommand("chat_open", () =>
      snapshot({
        entries: [{ kind: "user", text: "What projects do I have?" }],
        turn_active: true,
      }),
    );
    await renderChat();
    expect(screen.getByTestId("chat-thinking")).toBeInTheDocument();

    // Prose replaces it the moment there is any to render.
    act(() => {
      emitFromBackend("chat:event", {
        type: "delta",
        chat_id: CHAT_ID,
        text: "You have two.",
      });
    });
    expect(screen.queryByTestId("chat-thinking")).not.toBeInTheDocument();
  });

  it("does not pulse when no turn is running", async () => {
    await renderChat();
    expect(screen.queryByTestId("chat-thinking")).not.toBeInTheDocument();
  });
});
