/**
 * Launches the real Kodabi app against a throwaway vault and attaches the CDP
 * harness to it.
 *
 * The build this drives is NOT the one the cargo gates produce:
 *
 *   cargo build -p kodabi --features tauri/custom-protocol
 *
 * `tauri/custom-protocol` flips tauri's `dev` cfg off, so the exe serves the
 * embedded `dist/` from http://tauri.localhost instead of expecting a Vite dev
 * server on 1420 — while staying on the debug profile, which keeps the
 * MockEngine STT stub (no models, no native deps) and keeps the release-only
 * `compile_error!` engine guard quiet. See docs/UI_E2E_HARNESS.md.
 */

import { spawn } from "node:child_process";
import { createServer } from "node:net";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { waitForEndpoint } from "./cdp.mjs";

/** Keep the tail of the app's own stdio; it is the whole diagnosis in CI. */
const LOG_LINES = 200;

/**
 * Asks the OS for an unused port.
 *
 * Deliberately not a fixed 9222: a developer with Chrome already listening
 * there would have the harness silently attach to their browser and assert
 * against the wrong process.
 */
function freePort() {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
}

export async function launchKodabi({ exe, startupTimeoutMs = 60_000 } = {}) {
  if (!exe) {
    throw new Error("launchKodabi needs the path to a built kodabi.exe");
  }

  const vaultDir = await mkdtemp(join(tmpdir(), "kodabi-e2e-vault-"));
  const stateDir = await mkdtemp(join(tmpdir(), "kodabi-e2e-state-"));
  const indexDb = join(stateDir, "index.db");
  const userDataDir = join(stateDir, "webview2");
  const port = await freePort();

  const proc = spawn(exe, [], {
    env: {
      ...process.env,
      // The vault seam. Both are required together: index_state.rs feeds the KB
      // root to the startup reconcile job, so redirecting the vault while the
      // index stays in the real app-data dir would make a harness run converge
      // the developer's real index against an empty temp vault and delete every
      // row.
      KODABI_KB_ROOT: vaultDir,
      KODABI_INDEX_DB: indexDb,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}`,
      // Not optional. Tauri leaves WebView2's data directory unset, so it is
      // derived from the exe name — meaning a harness-launched kodabi.exe would
      // otherwise share a browser process with a developer's already-running
      // instance, and the debug-port argument would not take effect.
      WEBVIEW2_USER_DATA_FOLDER: userDataDir,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });

  const logLines = [];
  const record = (stream) => (chunk) => {
    for (const line of String(chunk).split(/\r?\n/)) {
      if (line.trim()) {
        logLines.push(`[${stream}] ${line}`);
      }
    }
    if (logLines.length > LOG_LINES) {
      logLines.splice(0, logLines.length - LOG_LINES);
    }
  };
  proc.stdout.on("data", record("out"));
  proc.stderr.on("data", record("err"));

  let exited = null;
  proc.on("exit", (code, signal) => {
    exited = `the app exited early (code ${code}, signal ${signal})`;
  });

  const reap = () => {
    if (proc.exitCode !== null || proc.signalCode !== null) {
      return;
    }
    // /T because WebView2 spawns msedgewebview2.exe children that do not
    // reliably die with the host, and /F because lib.rs intercepts
    // CloseRequested with prevent_close() and hides to tray — there is no
    // graceful close path outside the tray's Quit item, so a polite kill hangs.
    spawn("taskkill", ["/PID", String(proc.pid), "/T", "/F"], { stdio: "ignore" });
  };
  process.on("exit", reap);

  const handle = {
    proc,
    port,
    vaultDir,
    indexDb,
    logs: () => logLines.join("\n"),
    async stop({ keepArtifacts = false } = {}) {
      process.off("exit", reap);
      reap();
      // Give the OS a beat to release the temp dirs before removing them.
      await new Promise((resolve) => setTimeout(resolve, 500));
      if (keepArtifacts) {
        return;
      }
      await rm(vaultDir, { recursive: true, force: true }).catch(() => {});
      await rm(stateDir, { recursive: true, force: true }).catch(() => {});
    },
  };

  try {
    if (exited) {
      throw new Error(exited);
    }
    const version = await waitForEndpoint(port, { timeoutMs: startupTimeoutMs });
    handle.webviewVersion = version.Browser;
    return handle;
  } catch (error) {
    const detail = [error.message, exited, handle.logs()].filter(Boolean).join("\n");
    await handle.stop();
    throw new Error(`failed to launch ${exe}:\n${detail}`);
  }
}

/** The URL suffixes identifying each of the app's three webviews. */
export const WINDOW = {
  // `main` declares no url in tauri.conf.json, so it serves a bare path.
  main: "tauri.localhost/",
  quickCapture: "/capture.html",
  overlay: "/overlay.html",
};
