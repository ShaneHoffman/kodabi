import { afterEach, describe, expect, it } from "vitest";
import { applyTheme } from "./theme";
import { setMediaMatches } from "./test/media";

/*
 * The theme's DOM contract: the `day` class on <html>, and nothing else.
 *
 * The interesting half is "system". Grove's tokens are plain CSS with no media
 * query of their own, so applyTheme has to READ `prefers-color-scheme` and
 * decide — it cannot defer to the stylesheet the way the pre-Grove layer did
 * through its `data-theme` attribute. That resolution is what these tests pin:
 * a build where the class never moved under "system" would leave every window
 * stuck at night whatever the desktop is set to.
 */

const LIGHT = "(prefers-color-scheme: light)";
const root = () => document.documentElement;

afterEach(() => {
  root().classList.remove("day");
});

describe("applyTheme", () => {
  it("puts an explicit light theme on the class", () => {
    applyTheme("light");
    expect(root()).toHaveClass("day");
  });

  it("leaves the class off for an explicit dark theme", () => {
    applyTheme("dark");
    expect(root()).not.toHaveClass("day");
  });

  it("resolves the class when the OS asks for light under system", () => {
    setMediaMatches(LIGHT, true);
    applyTheme("system");
    expect(root()).toHaveClass("day");
  });

  it("leaves the class off under system when the OS is dark", () => {
    setMediaMatches(LIGHT, false);
    applyTheme("system");
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
