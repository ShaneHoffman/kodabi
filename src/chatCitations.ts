import type { ChatEntry, ChatNoteRef } from "./chat";

/**
 * The notes the answer at `index` drew on: every note read since the previous
 * turn boundary, in the order the model opened them, each cited once.
 *
 * Derived at render rather than stored on the entry, so a conversation replayed
 * from `chat_open`'s snapshot attributes exactly as the live event stream did —
 * the log is a flat record of what happened, and which answer a read belongs to
 * is a reading of that record. A permission card or an error mid-run does not
 * break the run: the tools still ran for this answer.
 *
 * Only a note the answer actually opened is a citation; the backend decides
 * that (`kodabi_core::chat::cited_note_id`) and leaves `note` null on every
 * other call, so a search's hits never become chips.
 */
export function citationsFor(
  entries: ChatEntry[],
  index: number,
): ChatNoteRef[] {
  const cited: ChatNoteRef[] = [];
  const seen = new Set<string>();
  for (let scan = index - 1; scan >= 0; scan -= 1) {
    const entry = entries[scan];
    if (entry.kind === "user" || entry.kind === "assistant") break;
    if (entry.kind === "tool_use" && entry.note && !seen.has(entry.note.id)) {
      seen.add(entry.note.id);
      cited.unshift(entry.note);
    }
  }
  return cited;
}
