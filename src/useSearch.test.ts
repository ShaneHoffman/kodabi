import { describe, expect, it } from "vitest";
import { highlightTerms, parseSnippet } from "./useSearch";

/** The sentinels `search_notes` marks matches with (index_cmds.rs), as escapes
 * for the same reason the source spells them that way. */
const OPEN = "\uE000";
const CLOSE = "\uE001";

describe("parseSnippet", () => {
  it("splits a marked snippet into plain and matched runs", () => {
    expect(parseSnippet(`the ${OPEN}tournament${CLOSE} is set`)).toEqual([
      { text: "the ", marked: false },
      { text: "tournament", marked: true },
      { text: " is set", marked: false },
    ]);
  });

  it("keeps every match when several are marked", () => {
    const segments = parseSnippet(`${OPEN}deck${CLOSE} and ${OPEN}railing${CLOSE}`);
    expect(segments.filter((segment) => segment.marked).map((segment) => segment.text)).toEqual([
      "deck",
      "railing",
    ]);
  });

  it("leaves a note's own Markdown bold alone", () => {
    // The whole reason for the private-use sentinels: `**` in a body is text.
    expect(parseSnippet("the **quote** was firm")).toEqual([
      { text: "the **quote** was firm", marked: false },
    ]);
  });

  it("treats an unpaired sentinel as plain text rather than dropping it", () => {
    const segments = parseSnippet(`a truncated ${OPEN}match`);
    expect(segments.map((segment) => segment.text).join("")).toContain("a truncated ");
    expect(segments.every((segment) => !segment.marked)).toBe(true);
  });

  it("returns an unmarked snippet as one run (the vector-only hit)", () => {
    expect(parseSnippet("nearest chunk text")).toEqual([
      { text: "nearest chunk text", marked: false },
    ]);
  });
});

describe("highlightTerms", () => {
  it("marks a term wherever it appears, case-insensitively", () => {
    expect(highlightTerms("Tournament bracket", "tournament")).toEqual([
      { text: "Tournament", marked: true },
      { text: " bracket", marked: false },
    ]);
  });

  it("marks each term of a multi-word query", () => {
    const segments = highlightTerms("deck railing decision", "deck railing");
    expect(segments.filter((segment) => segment.marked).map((segment) => segment.text)).toEqual([
      "deck",
      "railing",
    ]);
  });

  it("marks a partial word, which is what the backend's prefix match found", () => {
    expect(highlightTerms("Fall tournament", "tourna")).toEqual([
      { text: "Fall ", marked: false },
      { text: "tourna", marked: true },
      { text: "ment", marked: false },
    ]);
  });

  it("returns the text unmarked when nothing matches", () => {
    expect(highlightTerms("Deck railing", "zzzz")).toEqual([
      { text: "Deck railing", marked: false },
    ]);
  });

  it("returns the text unmarked for an empty query", () => {
    expect(highlightTerms("Deck railing", "")).toEqual([{ text: "Deck railing", marked: false }]);
  });
});
