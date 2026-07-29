/**
 * Builds the debug app for the E2E harness with the CDP debug port baked into
 * the compiled config, not passed as a runtime environment variable.
 *
 * `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` (the mechanism this harness started
 * with, and the one that still works on an ordinary dev machine) is silently
 * ignored on some hardened/managed Windows images — confirmed on GitHub's
 * hosted `windows-latest` runner: `kodabi.exe` and its `msedgewebview2.exe`
 * children were alive and consuming real memory, so the app launched and
 * rendered fine, but the CDP port never opened. `tauri.e2e.conf.json`'s
 * `additionalBrowserArgs` on every window is Tauri's own config-level
 * mechanism for the same thing, and it survives that environment.
 *
 * `additionalBrowserArgs` is read once at compile time (via `TAURI_CONFIG`,
 * merged by JSON Merge Patch — RFC 7396), so the debug port is a fixed
 * constant (`CDP_PORT` in `lib/app.mjs`) rather than one the harness picks
 * per run. `tauri.e2e.conf.json` restates every field of all three windows,
 * not just the new one: RFC 7396 replaces an array wholesale rather than
 * merging it element-by-element, so a partial window object here would wipe
 * the rest of that window's config (its `url`, `visible`, `decorations`, …).
 *
 * `additionalBrowserArgs` also REPLACES wry's own default browser arguments
 * rather than appending to them, so this file's value re-includes
 * `--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection` — dropping
 * it would not break the harness, but would quietly change the shipping
 * WebView2 feature set for this build.
 */

import { spawn } from "node:child_process";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const overlayPath = fileURLToPath(new URL("../src-tauri/tauri.e2e.conf.json", import.meta.url));
const overlay = await readFile(overlayPath, "utf8");

const proc = spawn(
  "cargo",
  ["build", "-p", "kodabi", "--features", "tauri/custom-protocol", "--locked"],
  {
    env: { ...process.env, TAURI_CONFIG: overlay },
    stdio: "inherit",
  },
);

proc.on("exit", (code) => {
  process.exitCode = code ?? 1;
});
