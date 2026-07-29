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
 *
 * It also gates the shipping Content Security Policy, for the same structural
 * reason: this is the only build in the repo that enforces one. `pnpm tauri dev`
 * serves the frontend from Vite, which sends no CSP header at all, so the whole
 * policy is inert in the one build mode anyone runs day to day and first bites in
 * exactly the build that ships. The last two scenarios are that gate; the policy
 * itself is annotated below, because `tauri.conf.json` is strict JSON and cannot
 * hold a comment.
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

/**
 * The shipping CSP (`src-tauri/tauri.conf.json`), source by source, and the
 * consumer that earns each one. Annotated here because that file is strict JSON
 * and takes no comment, and because this is where the only assertion that
 * enforces the policy lives.
 *
 *   default-src 'self'                the floor, and still load-bearing once
 *                                     connect-src is declared: it is what covers
 *                                     object-, frame-, child-, worker- and
 *                                     manifest-src.
 *   connect-src 'self'                same-origin fetch. No consumer today, but a
 *                                     *declared* connect-src stops inheriting
 *                                     default-src, so dropping it would break the
 *                                     first fetch anyone adds, with an error
 *                                     nobody would trace back to this line.
 *   connect-src http://ipc.localhost  Tauri's fast IPC path. Its injected
 *                                     ipc-protocol.js POSTs every invoke to
 *                                     convertFileSrc(cmd, "ipc"), which is this
 *                                     URL on Windows. Refuse it and Tauri catches
 *                                     the rejection, warns *once*, and silently
 *                                     degrades every later invoke in that webview
 *                                     to the slower postMessage bridge.
 *   connect-src ipc:                  the same target on macOS/Linux; inert on
 *                                     Windows. Kept so the string is the canonical
 *                                     @tauri-apps/api shape rather than a Windows
 *                                     dialect — the same reason `img-src asset:`
 *                                     is already there. Not dead weight.
 *   font-src data:                    six @fontsource faces fall under Vite's
 *                                     4096-byte assetsInlineLimit default and ship
 *                                     as data: URIs inside the built stylesheet.
 *   style-src 'unsafe-inline'         Tailwind's runtime, and xterm.js's injected
 *                                     stylesheet.
 *   media-src http://asset.localhost  the <audio src={convertFileSrc(…)}> in
 *                                     SessionArtifactsSection.tsx, which is the
 *                                     only asset-protocol consumer in the app.
 *
 * Deliberately absent: `https:` on img-src. A note body's `![](https://…)` reaches
 * a real <img> through ReactMarkdown (NoteEditorView.tsx), and blocking that is
 * the point — it is the remote-tracking-pixel block, not a bug to fix.
 */

/**
 * Console lines meaning the policy refused something, or that Tauri gave up on it.
 *
 * Targeted rather than a blanket "no console errors": WebView2 emits noise that
 * varies by runtime build, and docs/UI_E2E_HARNESS.md commits to *retiring* this
 * tier if a flake that is not a real bug goes unfixed for one attempt. A gate a
 * WebView2 auto-update can turn red is a gate that gets deleted.
 *
 * Two patterns rather than one. The first is Chromium's wording for every refusal
 * whatever the directive, so it also catches a future img-src or media-src
 * regression. The second pins this specific symptom, and stays valid if Chromium
 * ever rewords the first.
 */
const CSP_COMPLAINTS = [/Content Security Policy/i, /IPC custom protocol failed/i];

/** Every buffered console line from `session` that matches a complaint pattern. */
function cspComplaints(session) {
  return session.consoleLog().filter((line) => CSP_COMPLAINTS.some((re) => re.test(line)));
}

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

scenario("the inlined font faces load under the CSP", async () => {
  // Six @fontsource files fall under Vite's 4096-byte inline threshold and ship as
  // `data:` URIs inside dist/assets/*.css: the woff2 and the woff of all three
  // weights of Source Code Pro's Greek Extended subset (`unicode-range`
  // U+1F00-1FFF). Chromium evaluates a `data:` source eagerly rather than
  // deferring it on the range the way it defers a network fetch, so under a
  // `font-src` that omits `data:` all six are refused on load — observed, not
  // inferred: they appear in a webview's buffer with no Greek ever rendered.
  //
  // The console scenario below would therefore already catch this. This one
  // asserts it positively instead, which earns its keep twice over: it names the
  // actual failure ("the face did not load") rather than a log line, and it does
  // not depend on Chromium's wording for a refusal.
  //
  // U+1FB0 is GREEK SMALL LETTER ALPHA WITH VRACHY. `document.fonts.load` is a
  // plain expression returning a promise, which `evaluate` awaits: no eval, no
  // injected <script>, so this stays inside `script-src 'self'` (see lib/cdp.mjs).
  // A refused face makes it reject outright with a NetworkError.
  const probe = await main.evaluate(`
    document.fonts.load('400 12px "Source Code Pro"', "\\u1FB0").then(
      (faces) => ({
        ok: faces.length > 0 && faces.every((face) => face.status === "loaded"),
        detail: faces.map((face) => face.status).join(",") || "no @font-face matched",
      }),
      (error) => ({ ok: false, detail: "rejected: " + String(error) }),
    )
  `);

  // "no @font-face matched" fails too, and deliberately so: it means the probe
  // went stale — a @fontsource release restructured its subsets — not that the
  // policy is fine. Otherwise this gate could quietly decay into a no-op.
  assert.ok(probe.ok, `the Greek Extended mono face did not load: ${probe.detail}`);
});

scenario("no webview logged a CSP refusal or a degraded IPC path", async () => {
  // Last on purpose. By now both webviews have completed real invokes
  // (list_projects, list_notes, quick_capture_submit) and both have loaded every
  // font face the stylesheet declares, so every request the policy governs has
  // already been made.
  //
  // No sleep before reading the buffer: CDP delivers events and command replies
  // over one ordered socket, and a refusal is logged the moment the request is
  // blocked — strictly before the promise that request feeds can settle. So
  // anything the scenarios above awaited has already flushed.
  //
  // Two webviews, not three: `capture-overlay` is never attached. All three
  // enforce the identical policy and all three import src/fonts.ts, so these two
  // are sufficient evidence.
  //
  // deepEqual against [] rather than a length check, so a failure prints the
  // offending console lines verbatim. That output is usually the whole diagnosis.
  assert.deepEqual(cspComplaints(capture), [], "the quick-capture webview hit the CSP");
  assert.deepEqual(cspComplaints(main), [], "the main webview hit the CSP");
});

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
