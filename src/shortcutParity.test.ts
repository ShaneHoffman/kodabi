import { readFileSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";
import { CAPTURE_TOGGLE_SHORTCUT } from "./captureControl";
import { QUICK_CAPTURE_SHORTCUT } from "./quickCapture";

/*
 * The global-chord guard.
 *
 * Two OS-global accelerators are registered in Rust at startup and mirrored as
 * string constants here, because the frontend has to print them: the palette
 * hints and Settings' Capture card, which is the one place the app writes its
 * shortcuts down. Nothing checked that the mirror was still true. Change
 * `DEFAULT_QUICK_CAPTURE_SHORTCUT` in Rust and every gate stays green while
 * Settings confidently documents a chord that does nothing — the worst shape
 * of failure for a card whose entire job is to be believed.
 *
 * Same shape as the invoke-string guard next door: read the Rust source, no
 * dependency, runs in `pnpm test`. The end-to-end tier cannot close this one —
 * `Runtime.evaluate` cannot press an OS-global hotkey at all, which
 * docs/UI_E2E_HARNESS.md names as the gap `tauri-driver` would have to fill.
 */

const ROOT = process.cwd();

/** The Rust-side literal for a `pub const NAME: &str = "…";`, by file and name. */
function rustShortcut(file: string, name: string): string {
  const path = join(ROOT, "src-tauri", "src", file);
  const source = readFileSync(path, "utf8");
  const match = new RegExp(`${name}\\s*:\\s*&str\\s*=\\s*"([^"]+)"`).exec(source);
  if (!match) {
    throw new Error(`no ${name} found in ${relative(ROOT, path)}`);
  }
  return match[1];
}

describe("global shortcut parity", () => {
  it("prints the capture toggle the backend actually registers", () => {
    expect(
      CAPTURE_TOGGLE_SHORTCUT,
      "src/captureControl.ts must mirror DEFAULT_TOGGLE_SHORTCUT in " +
        "src-tauri/src/capture_control.rs — the frontend only prints this chord, " +
        "so drift teaches users a keypress that does nothing",
    ).toBe(rustShortcut("capture_control.rs", "DEFAULT_TOGGLE_SHORTCUT"));
  });

  it("prints the quick-capture chord the backend actually registers", () => {
    expect(
      QUICK_CAPTURE_SHORTCUT,
      "src/quickCapture.ts must mirror DEFAULT_QUICK_CAPTURE_SHORTCUT in " +
        "src-tauri/src/quick_capture.rs — Settings' Capture card and the palette " +
        "hint both read from it",
    ).toBe(rustShortcut("quick_capture.rs", "DEFAULT_QUICK_CAPTURE_SHORTCUT"));
  });

  it("reads the Rust constants at all", () => {
    // A guard on the guard: if the regex silently stopped matching, the two
    // tests above would still have to throw rather than pass vacuously — this
    // pins that they are reading real accelerator text, not an empty string.
    expect(rustShortcut("capture_control.rs", "DEFAULT_TOGGLE_SHORTCUT")).toMatch(/\+/);
    expect(rustShortcut("quick_capture.rs", "DEFAULT_QUICK_CAPTURE_SHORTCUT")).toMatch(/\+/);
  });
});
