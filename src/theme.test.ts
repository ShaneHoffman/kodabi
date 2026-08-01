import { afterEach, describe, expect, it } from "vitest";
import { applyTheme } from "./theme";
import { setMediaMatches } from "./test/media";

/*
 * The theme's DOM contract, both halves of it.
 *
 * Two systems run side by side while the Grove migration is in flight, and they
 * answer "system" differently — which is the whole reason this file exists:
 *
 *   - `data-theme` is pre-Grove. It DEFERS: "system" removes the attribute and
 *     design/tokens.css answers `prefers-color-scheme` itself.
 *   - the `day` class is Grove's. It RESOLVES: the Grove tokens are plain CSS
 *     with no media query of their own, so applyTheme has to read the query and
 *     decide.
 *
 * A test that only checked the attribute would pass on a build where the class
 * never moved, which is precisely the regression worth pinning.
 */

const LIGHT = "(prefers-color-scheme: light)";
const root = () => document.documentElement;

afterEach(() => {
  root().removeAttribute("data-theme");
  root().classList.remove("day");
});

describe("applyTheme", () => {
  it("puts an explicit light theme in both systems", () => {
    applyTheme("light");
    expect(root()).toHaveAttribute("data-theme", "light");
    expect(root()).toHaveClass("day");
  });

  it("puts an explicit dark theme in both systems", () => {
    applyTheme("dark");
    expect(root()).toHaveAttribute("data-theme", "dark");
    expect(root()).not.toHaveClass("day");
  });

  it("defers data-theme but resolves the class when the OS asks for light", () => {
    setMediaMatches(LIGHT, true);
    applyTheme("system");
    // Deferred: tokens.css answers the query on its own.
    expect(root()).not.toHaveAttribute("data-theme");
    // Resolved: Grove's tokens cannot.
    expect(root()).toHaveClass("day");
  });

  it("leaves the class off under system when the OS is dark", () => {
    setMediaMatches(LIGHT, false);
    applyTheme("system");
    expect(root()).not.toHaveAttribute("data-theme");
    expect(root()).not.toHaveClass("day");
  });

  it("an explicit choice overrules the OS in both directions", () => {
    setMediaMatches(LIGHT, true);
    applyTheme("dark");
    expect(root()).not.toHaveClass("day");

    setMediaMatches(LIGHT, false);
    applyTheme("light");
    expect(root()).toHaveClass("day");
  });

  it("clears the class when switching back from light to dark", () => {
    applyTheme("light");
    expect(root()).toHaveClass("day");
    applyTheme("dark");
    expect(root()).not.toHaveClass("day");
  });
});
