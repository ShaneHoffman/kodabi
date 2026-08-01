import { afterEach, describe, expect, it } from "vitest";
import { applyContrast, readContrast } from "./contrast";
import { setMediaMatches } from "./test/media";

/*
 * The high-contrast switch, and the one property that makes it safe: it can ADD
 * contrast but never take it away.
 *
 * There are two ways in — the OS `prefers-contrast: more` and the in-app toggle
 * — and only the second is persisted. The class is the OR of the two, which is
 * what pins "additive": turning the app toggle off while the OS is asking for
 * more contrast must not sharpen anything away from someone who needs it.
 */

const MORE = "(prefers-contrast: more)";
const root = () => document.documentElement;

afterEach(() => {
  root().removeAttribute("data-contrast");
  root().classList.remove("hc");
  window.localStorage.clear();
});

describe("applyContrast", () => {
  it("sets both systems when the user asks for more", () => {
    applyContrast(true);
    expect(root()).toHaveAttribute("data-contrast", "more");
    expect(root()).toHaveClass("hc");
  });

  it("clears both when the user turns it off", () => {
    applyContrast(true);
    applyContrast(false);
    expect(root()).not.toHaveAttribute("data-contrast");
    expect(root()).not.toHaveClass("hc");
  });

  it("keeps the class on for an OS request the app toggle never made", () => {
    setMediaMatches(MORE, true);
    applyContrast(false);
    expect(root()).toHaveClass("hc");
    // The attribute stays off: it mirrors the app preference alone, and the
    // pre-Grove tokens.css answers the OS query itself.
    expect(root()).not.toHaveAttribute("data-contrast");
  });

  it("cannot overrule the OS — turning the toggle off leaves the OS request standing", () => {
    setMediaMatches(MORE, true);
    applyContrast(true);
    expect(root()).toHaveClass("hc");
    applyContrast(false);
    expect(root()).toHaveClass("hc");
  });

  it("persists only the app preference, not the OS one", () => {
    setMediaMatches(MORE, true);
    applyContrast(false);
    expect(readContrast()).toBe(false);

    applyContrast(true);
    expect(readContrast()).toBe(true);
  });
});
