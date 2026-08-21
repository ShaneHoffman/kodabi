/**
 * Launches the real Kodabi app against a throwaway vault and attaches the CDP
 * harness to it.
 *
 * The build this drives is NOT the one the cargo gates produce, and it is not
 * a plain `cargo build` either — it must go through `e2e/build.mjs`
 * (`pnpm e2e:build`), which does two things together:
 *
 *   1. `--features tauri/custom-protocol` flips tauri's `dev` cfg off, so the
 *      exe serves the embedded `dist/` from http://tauri.localhost instead of
 *      expecting a Vite dev server on 1420 — while staying on the debug
 *      profile, which keeps the MockEngine STT stub (no models, no native
 *      deps) and keeps the release-only `compile_error!` engine guard quiet.
 *   2. `TAURI_CONFIG` merges in `src-tauri/tauri.e2e.conf.json`, which bakes
 *      the CDP debug port (`CDP_PORT` below) into every window's
 *      `additionalBrowserArgs` at compile time.
 *
 * See docs/UI_E2E_HARNESS.md.
 */

import { execFile, spawn } from "node:child_process";
import { createServer } from "node:net";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";

import { waitForEndpoint } from "./cdp.mjs";
import { indexDbFor, seedVault } from "./vault.mjs";

const execFileAsync = promisify(execFile);

/**
 * The child's environment: one switch, and none of the seams it drives.
 *
 * `KODABI_SANDBOX` is the same mechanism `pnpm dev:sandbox` and the `/preview`
 * skill use — the harness has no isolation of its own any more. Rust derives
 * the vault root, the index, the config dir and the WebView2 profile from this
 * base and refuses to launch if any of them would land in the developer's real
 * app directories (`kodabi_core::sandbox`).
 *
 * That is strictly more isolation than this harness used to have: settings and
 * consent used to come from the real `app_config_dir()`. A sandboxed run now
 * boots on fresh defaults — consent unacknowledged (the nudge is push-only, so
 * it still opens straight to the Inbox) and `RetentionPolicy::KeepAll`, which
 * means a developer's own retention setting can no longer prune fixtures out
 * from under a run.
 *
 * The three state variables are deleted rather than passed through: they are
 * refused alongside the switch, so a developer's shell carrying them from an
 * older manual preview would otherwise fail every slice with a startup error.
 */
function sandboxEnv(base) {
  const env = { ...process.env, KODABI_SANDBOX: base };
  delete env.KODABI_KB_ROOT;
  delete env.KODABI_INDEX_DB;
  delete env.KODABI_LEDGER_DB;
  delete env.WEBVIEW2_USER_DATA_FOLDER;
  return env;
}

/**
 * A one-line snapshot of whether `kodabi.exe` and any `msedgewebview2.exe`
 * children are alive, for the failure path only.
 *
 * When the CDP endpoint never comes up, "the process crashed" and "the
 * process is alive but never created a webview" look identical from the
 * harness's side — both leave `logs()` empty, since neither writes anything
 * to stdout/stderr. This is the one cheap way to tell them apart without a
 * debugger attached to the machine: if `kodabi.exe` is gone, it exited
 * silently; if it's alive with zero webview children, the hang is in Rust
 * setup before any window exists; if webview children exist, Tauri got as
 * far as creating a webview and the debug-port argument or the CDP listener
 * itself is the problem.
 */
async function snapshotProcesses() {
  // Two calls, not one `/FI` per image name: tasklist ANDs multiple filters
  // together rather than ORing them, and a process can't match two image
  // names at once.
  const forImage = async (imageName) => {
    try {
      const { stdout } = await execFileAsync("tasklist", ["/FI", `IMAGENAME eq ${imageName}`]);
      return stdout.trim();
    } catch (error) {
      return `tasklist ${imageName} failed: ${error.message}`;
    }
  };
  const [kodabi, webview] = await Promise.all([
    forImage("kodabi.exe"),
    forImage("msedgewebview2.exe"),
  ]);
  return `${kodabi}\n${webview}`;
}

/** Keep the tail of the app's own stdio; it is the whole diagnosis in CI. */
const LOG_LINES = 200;

/**
 * The CDP debug port, fixed rather than harness-chosen.
 *
 * It has to match `additionalBrowserArgs` in `src-tauri/tauri.e2e.conf.json`
 * exactly: that value is compiled into the binary once (via `TAURI_CONFIG` at
 * build time — see `e2e/build.mjs`), so there is no runtime hook left for the
 * harness to pick a fresh one per run. `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`
 * *would* have let each run choose its own port, and worked exactly that way
 * on an ordinary dev machine — but it was silently ignored on GitHub's hosted
 * Windows runner: the app and its WebView2 children launched and ran fine,
 * they just never opened a CDP port. The config-level mechanism survives
 * that environment, at the cost of the port being fixed.
 */
const CDP_PORT = 9339;

/**
 * Fails fast, before spawning, if something is already listening on the
 * fixed CDP port — a leftover process from a crashed previous run, or an
 * unrelated program. Without this the harness would otherwise wait out the
 * full startup timeout attached to the wrong process, or worse, silently
 * pass against it.
 */
function assertPortFree(port) {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.on("error", () =>
      reject(
        new Error(
          `port ${port} is already in use — a leftover kodabi.exe from a prior run, ` +
            `or another program. Free it before running the E2E harness again.`,
        ),
      ),
    );
    server.listen(port, "127.0.0.1", () => server.close(resolve));
  });
}

/**
 * Launches the app against a fresh throwaway vault.
 *
 * `seed` names scenarios from `./vault.mjs` to write into that vault before the
 * app starts; omit it and the vault stays empty, which is what the quick-capture
 * slice needs. `handle.seeded` then carries the manifest of what landed.
 *
 * `startupTimeoutMs` is 120s and not 60: on a first-ever run on a fresh CI VM
 * the wait stacks a cold Windows Defender real-time scan of a freshly built,
 * unsigned exe on top of WebView2's own first-launch cost on that machine.
 * Locally this never gets close to either bound — success lands in ~1.3-1.7s —
 * so the higher ceiling costs nothing on the path that matters.
 */
export async function launchKodabi({ exe, seed = [], startupTimeoutMs = 120_000 } = {}) {
  if (!exe) {
    throw new Error("launchKodabi needs the path to a built kodabi.exe");
  }

  await assertPortFree(CDP_PORT);

  // One directory, not two: `KODABI_SANDBOX` derives the vault root, the index
  // (`.index/index.db`), the config dir and the WebView2 profile from a single
  // base, so the harness names a base and lets Rust lay it out.
  const vaultDir = await mkdtemp(join(tmpdir(), "kodabi-e2e-"));
  const indexDb = indexDbFor(vaultDir);
  const port = CDP_PORT;

  // Before the spawn, never after, and the ordering is load-bearing twice over.
  // `IndexState::initialize` fires a startup Reconcile over whatever is on disk
  // at launch; and `watch.rs` ignores everything under `sessions/` outright, so
  // a transcript written after launch is not merely late to the index — it is
  // never seen at all.
  let seeded = null;
  if (seed.length > 0) {
    try {
      seeded = await seedVault(vaultDir, seed);
    } catch (error) {
      await rm(vaultDir, { recursive: true, force: true }).catch(() => {});
      throw error;
    }
  }

  const proc = spawn(exe, [], {
    env: sandboxEnv(vaultDir),
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
    /**
     * The seed manifest, or null when nothing was seeded.
     *
     * A slice asserts against the titles and stems actually written rather than
     * restating catalogue strings, so a catalogue edit cannot leave a test
     * chasing a title that is gone.
     */
    seeded,
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
    const processes = await snapshotProcesses();
    const detail = [error.message, exited, handle.logs(), "--- tasklist ---", processes]
      .filter(Boolean)
      .join("\n");
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
