import { describe, expect, it } from "vitest";
import { applyMarkup } from "./textareaCaret";

/** The note editor's format toolbar, as a pure function. `selectionAnchor` is
 * left to the app: it measures a real control's layout, and a jsdom mirror
 * reports every offset as zero, so a test of it would assert nothing. */
describe("applyMarkup", () => {
  describe("wrapping", () => {
    it("wraps the selection", () => {
      const result = applyMarkup("say hello there", 4, 9, { wrap: "**" });

      expect(result.value).toBe("say **hello** there");
      expect(result.value.slice(result.start, result.end)).toBe("hello");
    });

    it("unwraps when the same markup is already there, so pressing B twice is a no-op", () => {
      const bolded = applyMarkup("say hello there", 4, 9, { wrap: "**" });

      const result = applyMarkup(bolded.value, bolded.start, bolded.end, { wrap: "**" });

      expect(result.value).toBe("say hello there");
      expect(result.value.slice(result.start, result.end)).toBe("hello");
    });

    it("nests italic inside bold instead of eating the bold", () => {
      // `*` appears on both sides of the selection because `**` does. Matching
      // on that alone made Italic-on-bold silently downgrade the text to
      // italic — the one keystroke that must not lose formatting.
      const result = applyMarkup("**bold**", 2, 6, { wrap: "*" });

      expect(result.value).toBe("***bold***");
      expect(result.value.slice(result.start, result.end)).toBe("bold");
    });

    it("still unwraps italic that is genuinely on its own", () => {
      const result = applyMarkup("*just italic*", 1, 12, { wrap: "*" });

      expect(result.value).toBe("just italic");
    });
  });

  describe("prefixing", () => {
    it("prefixes every line of a multi-line selection", () => {
      const result = applyMarkup("one\ntwo\nthree", 0, 13, { prefix: "- " });

      expect(result.value).toBe("- one\n- two\n- three");
    });

    it("leaves a line that already carries the prefix alone", () => {
      const result = applyMarkup("- one\ntwo", 0, 9, { prefix: "- " });

      expect(result.value).toBe("- one\n- two");
    });

    it("reaches back to the start of the line the selection begins in", () => {
      const result = applyMarkup("hello there", 6, 11, { prefix: "## " });

      expect(result.value).toBe("## hello there");
    });
  });

  describe("linking", () => {
    it("puts the selection in the label and leaves the caret in the URL slot", () => {
      // The two halves of a link differ, so it cannot go through `wrap`:
      // that put the whole `[]()` string on both sides and produced
      // `[]()docs[]()` for every input.
      const result = applyMarkup("see docs now", 4, 8, { link: true });

      expect(result.value).toBe("see [docs]() now");
      expect(result.start).toBe(result.end);
      expect(result.value.slice(0, result.start)).toBe("see [docs](");
    });

    it("still opens an empty link when nothing is selected", () => {
      const result = applyMarkup("see ", 4, 4, { link: true });

      expect(result.value).toBe("see []()");
      expect(result.value.slice(0, result.start)).toBe("see [](");
    });
  });
});
