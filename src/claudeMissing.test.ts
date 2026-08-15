import { describe, expect, it } from "vitest";
import { isClaudeMissingMessage } from "./claudeMissing";
import {
  CHAT_CLAUDE_MISSING_ERROR,
  CLAUDE_MISSING_MESSAGE,
} from "./test/claudeMissing";

/**
 * The TS half of a cross-language pin. The fixture is
 * `kodabi_core::llm::CLAUDE_MISSING_MESSAGE` (crates/kodabi-core/src/llm.rs)
 * copied verbatim; the test beside that constant asserts the same marker phrase
 * from the Rust side. The copy may change, but the marker has to survive in
 * both, or one of these two tests fails.
 */
describe("isClaudeMissingMessage", () => {
  it("recognises the canonical message from Rust", () => {
    expect(isClaudeMissingMessage(CLAUDE_MISSING_MESSAGE)).toBe(true);
  });

  it("recognises it through the chat path's Display prefix", () => {
    // `ChatSpawnError` renders as "failed to spawn ...: <message>", and the
    // rejection reaches the view through String(error) on top of that.
    expect(isClaudeMissingMessage(`Error: ${CHAT_CLAUDE_MISSING_ERROR}`)).toBe(
      true,
    );
  });

  it("leaves every other backend failure to the generic copy", () => {
    const others = [
      "claude not found",
      "program not found (os error 2)",
      "claude exited 1",
      "headless Claude Code did not exit within 180s",
      "",
    ];

    for (const message of others) {
      expect(isClaudeMissingMessage(message)).toBe(false);
    }
  });
});
