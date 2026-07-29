/**
 * The end-to-end slice: type a thought into the real quick-capture window, file
 * it with the real button, and watch it arrive in the real Inbox — across two
 * webviews, through the real IPC bridge, against the real Rust backend and real
 * files on disk.
 *
 * This is the one thing `pnpm test` structurally cannot do. That suite mocks
 * `invoke`/`listen` at the IPC boundary (`src/test/tauri.ts`), so an unwired
 * onClick, a renamed invoke string, and a DTO field-casing mismatch all pass it.
 * See docs/UI_E2E_HARNESS.md.
 */

import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";

import { attach, findTarget } from "./lib/cdp.mjs";
import { launchKodabi, WINDOW } from "./lib/app.mjs";
import { allTextOf, clickWhenEnabled, textOf, typeInto, waitForOneOf } from "./lib/page.mjs";

const EXE = process.env.KODABI_E2E_EXE ?? "target/debug/kodabi.exe";

/**
 * A single line, and unique per run.
 *
 * Single-line matters: `filename_seed` takes the first non-empty line for the
 * slug, and `capture.rs` stores that same line as the frontmatter title, which
 * `vault::effective_title` then prefers over the de-slugged filename. So the
 * Inbox row's title comes back byte-identical to what is typed here, which is
 * what makes an exact-equality assertion meaningful.
 */
const MARKER = `e2e quick capture ${process.pid}-${Date.now()}`;

let app;
let capture;
let main;

/**
 * Whether anything in this file has failed.
 *
 * Tracked by hand because `process.exitCode` cannot answer it here: node:test
 * sets the exit code only *after* the file's `after` hook has run, so reading it
 * in the hook always sees `undefined` — and every diagnostic below would be
 * skipped on exactly the runs that need it.
 */
let failed = false;

/** A `test()` that records its own failure for the `after` hook. */
function scenario(name, body) {
  test(name, async (t) => {
    try {
      await body(t);
    } catch (error) {
      failed = true;
      throw error;
    }
  });
}

before(async () => {
  try {
    app = await launchKodabi({ exe: EXE });
    capture = await attach(await findTarget(app.port, { urlEndsWith: WINDOW.quickCapture }));
    main = await attach(await findTarget(app.port, { urlEndsWith: WINDOW.main }));
  } catch (error) {
    failed = true;
    throw error;
  }
});

after(async () => {
  // Dump both consoles and the app's own stdio; a CI-only failure is close to
  // undiagnosable without them.
  if (failed) {
    console.error("--- quick-capture console ---\n" + (capture?.consoleLog().join("\n") ?? ""));
    console.error("--- main console ---\n" + (main?.consoleLog().join("\n") ?? ""));
    console.error("--- app stdio ---\n" + (app?.logs() ?? ""));
    console.error(`--- vault kept at ${app?.vaultDir} ---`);
  }
  capture?.close();
  main?.close();
  await app?.stop({ keepArtifacts: failed });
});

scenario("both webviews mount against a fresh vault", async () => {
  // The quick-capture window is `visible: false` but pre-created, so its
  // webview is live from startup. Driving it hidden is deliberate: it sidesteps
  // the DismissArmed blur guard entirely, because nothing can steal focus from
  // a window that never had it.
  await capture.waitFor(
    `document.readyState === "complete" && !!document.querySelector('[data-testid="quick-capture-input"]')`,
    { label: "quick-capture mounted" },
  );
  // Waiting on a rendered element, not just `readyState`: the root render is
  // scheduled rather than run inline, so "complete" can fire with an empty body
  // and every assertion below would then read null off a DOM React has not
  // reached yet.
  await main.waitFor(
    `document.readyState === "complete" && !!document.querySelector('[data-testid="sidebar-inbox-count"]')`,
    { label: "main mounted" },
  );

  // A cheap tripwire, not a proof: the row renders `note_count ?? 0` while
  // `list_projects` is still in flight, so a "0" read early says nothing. The
  // real freshness guarantee is structural — `launchKodabi` points KODABI_KB_ROOT
  // at a fresh `mkdtemp` dir — and the assertions that would actually catch the
  // app ignoring it are the exact-list and count-of-1 ones below.
  assert.equal(await textOf(main, "sidebar-inbox-count"), "0", "the temp vault should start empty");
});

scenario("filing a thought puts it in the Inbox", async () => {
  await typeInto(capture, "quick-capture-input", MARKER);

  // The click, not an Enter keydown: "the button doesn't actually call the
  // command" is precisely the failure this tier exists to catch, and only the
  // click path proves that wiring.
  await clickWhenEnabled(capture, "quick-capture-submit");

  // Proves invoke("quick_capture_submit") reached the Rust command and
  // resolved. A renamed invoke string rejects the promise and this times out.
  await capture.waitFor(
    `document.querySelector('[data-testid="quick-capture-destination"]')?.textContent.includes("Inbox") ?? false`,
    { label: "the filed flash names the Inbox" },
  );

  // Crossing to the other webview. Nothing is pushed from the harness: the Rust
  // side emits `vault:changed`, the bridge relays it to the DOM bus, and every
  // vault query refetches. The list reads straight off disk rather than the
  // SQLite index, so there is no indexing latency to wait out.
  await waitForOneOf(main, "inbox-row-title", MARKER);

  const titles = await allTextOf(main, "inbox-row-title");
  assert.deepEqual(titles, [MARKER], "the Inbox should hold exactly the note just filed");
});

scenario("the sidebar count reflects the new note", async () => {
  // A second, independent assertion over a *different* command and DTO:
  // list_projects -> inbox_note_count (snake_case). A field-casing regression
  // there breaks this and nothing else in the slice, which is what makes it
  // worth asserting separately.
  await main.waitFor(
    `document.querySelector('[data-testid="sidebar-inbox-count"]')?.textContent.trim() === "1"`,
    { label: "sidebar inbox count reaches 1" },
  );
});

scenario("the note is on disk with the captured title", async () => {
  // Separates "the UI did not render it" from "the backend did not write it".
  const inbox = join(app.vaultDir, "Inbox");
  const files = (await readdir(inbox)).filter((name) => name.endsWith(".md"));
  assert.equal(files.length, 1, `expected one note in ${inbox}, saw ${files.join(", ")}`);

  const body = await readFile(join(inbox, files[0]), "utf8");
  assert.match(body, new RegExp(`^title: .*${escapeRegExp(MARKER)}`, "m"));
});

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
