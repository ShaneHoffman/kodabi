/**
 * The missing-CLI message exactly as Rust puts it on the wire.
 *
 * A verbatim copy of `kodabi_core::llm::CLAUDE_MISSING_MESSAGE`
 * (crates/kodabi-core/src/llm.rs), kept in one place so every surface's test
 * asserts against the real string rather than a paraphrase of it. The pin that
 * this copy is still faithful lives in `src/claudeMissing.test.ts`, with its
 * Rust counterpart beside the constant.
 */
export const CLAUDE_MISSING_MESSAGE =
  "Kodabi's AI features run through Claude Code, and Claude Code isn't installed on this computer. Install the claude CLI from docs.claude.com/en/docs/claude-code/overview, then try again.";

/** The same failure as it reaches the chat view: `ChatSpawnError`'s Display
 * prefix, then `String(error)` on the rejection. */
export const CHAT_CLAUDE_MISSING_ERROR = `failed to spawn headless Claude Code for chat: ${CLAUDE_MISSING_MESSAGE}`;
