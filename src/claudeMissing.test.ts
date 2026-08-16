import { readFileSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";
import { isClaudeMissingMessage } from "./claudeMissing";
import { CLAUDE_MISSING_MESSAGE } from "./test/claudeMissing";

/**
 * The cross-language pin, and it has to actually cross: the detection is a
 * substring match against a string minted in Rust, so a test asserting the TS
 * marker against a TS fixture proves only that the fixture and the marker
 * agree with each other. Reword `CLAUDE_MISSING_MESSAGE` in Rust and every
 * test on both sides stays green while `isClaudeMissingMessage` quietly
 * matches nothing and all four surfaces fall back to echoing the raw error.
 *
 * So the Rust constant is read from source here, the way
 * `src/invokeParity.test.ts` reads the `generate_handler![…]` list. That makes
 * a reworded message a failing test rather than a silent regression, and the
 * fixture in `src/test/claudeMissing.ts` a checked copy rather than a hopeful
 * one.
 */
const LLM_RS = join(process.cwd(), "crates", "kodabi-core", "src", "llm.rs");

/** `kodabi_core::llm::CLAUDE_MISSING_MESSAGE`, read out of the Rust source. */
function rustMissingMessage(): string {
  const source = readFileSync(LLM_RS, "utf8");
  const declaration = /pub const CLAUDE_MISSING_MESSAGE: &str = "([^"]*)";/.exec(
    source,
  );
  if (!declaration) {
    throw new Error(
      `no CLAUDE_MISSING_MESSAGE declaration found in ${relative(process.cwd(), LLM_RS)}`,
    );
  }
  return declaration[1];
}

describe("isClaudeMissingMessage", () => {
  it("recognises the message Rust actually ships", () => {
    expect(isClaudeMissingMessage(rustMissingMessage())).toBe(true);
  });

  it("keeps the test fixture a verbatim copy of the Rust constant", () => {
    expect(CLAUDE_MISSING_MESSAGE).toBe(rustMissingMessage());
  });

  it("recognises the canonical message from Rust", () => {
    expect(isClaudeMissingMessage(CLAUDE_MISSING_MESSAGE)).toBe(true);
  });

  it("recognises it inside a wrapping Display prefix", () => {
    // The chat path now strips its own prefix in Rust (`user_errors` passes
    // the sentence through bare), but the match deliberately stays a
    // substring test: a channel that wraps the message stays recognisable.
    expect(
      isClaudeMissingMessage(`Error: some wrapper: ${CLAUDE_MISSING_MESSAGE}`),
    ).toBe(true);
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
