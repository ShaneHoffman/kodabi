/**
 * Recognising the one backend failure that has a fix the user can perform.
 *
 * The `claude` CLI is a user-installed prerequisite (README.md), so on a fresh
 * machine every AI feature fails at the same place, and the failure most people
 * meet first is a distill that produced nothing. Left as the OS error it starts
 * as ("program not found"), it reads as a bug in Kodabi rather than a missing
 * install.
 *
 * Every channel this failure travels is a plain string — the `distill:state`
 * event's `message`, and the rejection of `chat_open` / `terminal_open`, which
 * like every Tauri command here returns `Result<T, String>`. So rather than
 * three new wire shapes, Rust puts one canonical message on the wire and this
 * module matches the marker phrase inside it. The Rust half is
 * `kodabi_core::llm::CLAUDE_MISSING_MESSAGE`
 * (crates/kodabi-core/src/llm.rs); a test there and in `claudeMissing.test.ts`
 * hold the two halves together.
 */

/** The phrase Rust's canonical message is guaranteed to contain. A substring
 * rather than the whole message because the chat path wraps it in a Display
 * prefix on the way out. */
export const CLAUDE_MISSING_MARKER = "Claude Code isn't installed";

/** Whether a backend error message is the missing-CLI case. */
export function isClaudeMissingMessage(message: string): boolean {
  return message.includes(CLAUDE_MISSING_MARKER);
}

/** Where to get it, shared by every surface so the URL is written once. */
export const CLAUDE_INSTALL_URL = "docs.claude.com/en/docs/claude-code/overview";
