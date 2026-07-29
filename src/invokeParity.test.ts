import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

/*
 * The invoke-string guard.
 *
 * `.claude/rules/tauri-command-parity.md` requires the string a TS caller
 * passes to `invoke<T>()` to equal the Rust command's function name exactly.
 * Miss that and nothing complains: it compiles, it typechecks, and it fails
 * only at runtime when a user presses the button. That has always been a
 * review-enforced rule; this makes it a gate, in the same shape as the token
 * guard next door — it runs in `pnpm test`, which CI already runs, and adds no
 * dependency.
 *
 * This is the cheap half of the parity coverage. It cannot tell whether a
 * control is *wired* to the command it names — an onClick that was never
 * attached, an always-true `disabled`, an early return all pass this and reach
 * users. That gap is what the end-to-end tier in `e2e/` exists for; see
 * docs/UI_E2E_HARNESS.md for the split.
 */

const ROOT = process.cwd();
const LIB_RS = join(ROOT, "src-tauri", "src", "lib.rs");
const SRC = join(ROOT, "src");

/**
 * The command names registered in the single `tauri::generate_handler![…]`.
 *
 * Entries are `module::name` or a bare `name`; the invoke string is always the
 * last segment, because that is the Rust function name.
 */
function registeredCommands(): Set<string> {
  const source = readFileSync(LIB_RS, "utf8");
  const list = /generate_handler!\s*\[([\s\S]*?)\]/.exec(source);
  if (!list) {
    throw new Error(`no generate_handler![…] found in ${relative(ROOT, LIB_RS)}`);
  }
  const names = list[1]
    .split(",")
    .map((entry) => entry.replace(/\/\/.*$/gm, "").trim())
    .filter(Boolean)
    .map((entry) => entry.split("::").pop() as string);
  return new Set(names);
}

/** Every TS/TSX module that could hold a caller, excluding the test harness. */
function callerModules(dir: string, found: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) {
      if (entry !== "test") callerModules(full, found);
    } else if (/\.tsx?$/.test(entry) && !/\.test\.tsx?$/.test(entry)) {
      found.push(full);
    }
  }
  return found;
}

type Call = { file: string; line: number; command: string };

/**
 * Every `invoke("name")` / `invoke<T>("name")` in the frontend.
 *
 * Tauri's own plugin channels (`plugin:event|listen` and friends) are not
 * commands in the handler list, so they are skipped.
 */
function invokeCalls(): Call[] {
  const calls: Call[] = [];
  for (const file of callerModules(SRC)) {
    const lines = readFileSync(file, "utf8").split(/\r?\n/);
    lines.forEach((text, index) => {
      const matches = text.matchAll(/\binvoke\s*(?:<[^>]*>)?\s*\(\s*["'`]([^"'`]+)["'`]/g);
      for (const match of matches) {
        const command = match[1];
        if (command.startsWith("plugin:")) continue;
        calls.push({ file: relative(ROOT, file), line: index + 1, command });
      }
    });
  }
  return calls;
}

describe("tauri command parity", () => {
  it("registers every command the frontend invokes", () => {
    const registered = registeredCommands();
    const unknown = invokeCalls().filter((call) => !registered.has(call.command));

    expect(
      unknown.map((call) => `${call.file}:${call.line} invokes "${call.command}"`),
      `these invoke strings are not in generate_handler![…] in ${relative(ROOT, LIB_RS)} — ` +
        "a command that is not registered fails only at runtime",
    ).toEqual([]);
  });

  it("finds the handler list and the callers at all", () => {
    // A guard on the guard: if either regex silently stopped matching, the test
    // above would pass vacuously forever.
    expect(registeredCommands().size).toBeGreaterThan(20);
    expect(invokeCalls().length).toBeGreaterThan(20);
  });
});
