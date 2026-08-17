/**
 * `node scripts/screenshots.mjs [shot...]` — retakes the screenshots README.md
 * embeds, from a running sandboxed dev window.
 *
 * Why a script rather than five hand-shot PNGs: the images make a claim about
 * what the app looks like *now*, and the first one went stale four days after it
 * was committed — the custom title bar landed and the hero still showed a window
 * with no caption controls, which is what task #225 was filed for. A retake has
 * to be cheap or it does not happen.
 *
 * It drives the REAL app over CDP rather than the `preview-mock.js` design
 * harness, so every count, match score and routing destination on screen came
 * from the seeded fixture vault through the real backend. Two consequences:
 *
 *   1. It needs a window to attach to, launched with a debugging port:
 *
 *        $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9222'
 *        pnpm dev:sandbox
 *
 *      `pnpm dev:sandbox`, never bare `pnpm tauri dev`: a screenshot run is one
 *      of the launches `.claude/rules/dev-sandbox.md` names, and shooting the
 *      developer's own vault would put real meeting notes in a public README.
 *      The variable is read by the WebView2 runtime itself, and the launcher
 *      passes the environment through untouched apart from `KODABI_SANDBOX`.
 *
 *   2. The chat shot spends one real Claude turn. `ChatView` is a live session
 *      and the fixture catalogue deliberately writes no `chats/` transcript, so
 *      there is nothing to load instead. Name shots on the command line to
 *      re-take a subset and skip it: `node scripts/screenshots.mjs inbox note`.
 *
 * Geometry: every main-window shot is a 1280x720 CSS viewport rasterized at 2x,
 * which is 2560x1440 — the size and the composition of the first hero, measured
 * back off it. The viewport is imposed with `Emulation.setDeviceMetricsOverride`
 * rather than by resizing the window, so the output does not depend on the
 * display the script runs on; `src-tauri/capabilities/main-window.json` grants no
 * `set-size` permission, so the page could not resize itself anyway.
 */

import { Buffer } from "node:buffer";
import { writeFile } from "node:fs/promises";

import { WINDOW } from "../e2e/lib/app.mjs";
import { attach, findTarget, listTargets, waitForEndpoint } from "../e2e/lib/cdp.mjs";
import { settle } from "../e2e/lib/page.mjs";

/**
 * The dev window's debugging port.
 *
 * Not the e2e tier's fixed 9339: that one is compiled into the e2e binary via
 * `tauri.e2e.conf.json`, while a dev launch picks its port at runtime through
 * `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`. 9222 is the conventional default the
 * command in the header prints; override both together if it is taken.
 */
const PORT = Number(process.env.KODABI_SCREENSHOT_PORT ?? 9222);

const OUT_DIR = new URL("../docs/screenshots/", import.meta.url);

/** The hero's measured CSS viewport, and the factor that makes it 2560x1440. */
const MAIN_VIEWPORT = { width: 1280, height: 720 };
const SCALE = 2;

/**
 * The quick-capture window's real size, from `app.windows` in
 * `src-tauri/tauri.conf.json`. Shot at its own size rather than the main
 * window's: it is a small floating sheet, and stretching its viewport to 720px
 * tall would photograph a 280px panel centred in 440px of nothing.
 */
const CAPTURE_VIEWPORT = { width: 640, height: 280 };

/** The fixture note the hero has always shown (`composition/at-ceiling`). */
const HERO_PROJECT = "Briarwood Golf";
const HERO_NOTE = "Q3 budget and irrigation contractor shortlist";

/** A term the seeded vault answers in more than one note, so the results list
 * has something to show and the marks land in both titles and snippets. */
const SEARCH_QUERY = "irrigation";

/** One real question over the fixture notes. Short on purpose: it costs a live
 * Claude turn, and a long answer would scroll its own opening off the shot. */
const CHAT_PROMPT = "What did we decide about the irrigation contractor?";

/** The text that gives the quick-capture sheet a routing destination to show.
 * It is only ever typed, never submitted, so the sandbox vault is unchanged. */
const CAPTURE_DRAFT = "Ask GreenFlow whether the irrigation bid covers the clubhouse drainage run";

/** Embeds a JS value into an expression safely (page.mjs's `lit`). */
const lit = (value) => JSON.stringify(value);

/**
 * Finds one button by its visible label, scoped to a container.
 *
 * The dock rows, the project index rows and the note rows are all "the whole row
 * is the button", and none of them carries a `data-testid` — they are lists of
 * vault content, so there is nothing stable to hang one on. `e2e/lib/page.mjs`
 * selects by testid throughout, which is right for assertions; a screenshot
 * script is staging, and matching the visible label is what keeps this readable.
 *
 * Two ways of matching, because the two kinds of button in reach spell their
 * label differently: a `Button` primitive renders its children straight into the
 * element, so its whole text IS the label, while a row wraps each part in its own
 * block `<span>` and the element's text is the label with the row's count and
 * metadata run together after it.
 */
const buttonLabelled = (scope, label) => `
  [...document.querySelectorAll(${lit(`${scope} button`)})].find(
    (button) =>
      button.textContent.trim() === ${lit(label)} ||
      [...button.querySelectorAll("span")].some(
        (span) => span.textContent.trim() === ${lit(label)},
      ),
  )`;

async function clickLabelled(session, scope, label) {
  const expression = buttonLabelled(scope, label);
  await session.waitFor(`!!(${expression})`, {
    label: `a ${scope} button labelled ${lit(label)}`,
    timeoutMs: 15_000,
  });
  return session.evaluate(`((${expression}).click(), true)`);
}

/**
 * Types into a React-controlled field addressed by CSS selector.
 *
 * The same native-setter trick as `page.mjs`'s `typeInto`, and for the same
 * reason (React installs its own value setter on the instance, so assigning
 * `.value` leaves its state untouched and the next render wipes the text). This
 * one takes a selector because the two fields it drives — SearchView's input and
 * ChatView's composer — are identified by their `aria-label` rather than a
 * testid.
 */
async function typeIntoSelector(session, selector, text) {
  return session.evaluate(`
    (() => {
      const element = document.querySelector(${lit(selector)});
      if (!element) {
        throw new Error("no element matching " + ${lit(selector)});
      }
      const descriptor = Object.getOwnPropertyDescriptor(
        Object.getPrototypeOf(element),
        "value",
      );
      descriptor.set.call(element, ${lit(text)});
      element.dispatchEvent(new Event("input", { bubbles: true }));
      return true;
    })()
  `);
}

/** A real Escape keypress, which is how the first-run dialogs dismiss. */
async function pressEscape(session) {
  for (const type of ["keyDown", "keyUp"]) {
    await session.send("Input.dispatchKeyEvent", {
      type,
      key: "Escape",
      code: "Escape",
      windowsVirtualKeyCode: 27,
      nativeVirtualKeyCode: 27,
    });
  }
}

/**
 * Clears the first-run chrome that would otherwise sit over every shot.
 *
 * A sandbox boots on fresh settings with an empty `.models/`, so
 * `ModelDownloadNudge` opens as a modal dialog on launch. It is real and correct
 * — it is just first-run onboarding rather than any of the features these images
 * illustrate, and a user sees it once. Escape is one of the three ways it
 * documents for dismissing itself, so this is the same gesture a person makes.
 *
 * The update notice needs no handling: `UpdaterProvider` never checks in a dev
 * session, which is also what README's privacy section promises.
 *
 * A bounded wait rather than one snapshot, because the nudge does not exist yet
 * when the shell does: `useModelDownload` seeds `{ status: "unknown" }` and the
 * dialog renders `null` for that beat, so it only mounts once the `model_status`
 * invoke lands — and `waitForShell` pins the *vault* query, which is a different
 * round trip with no ordering between them. A single check can therefore pass a
 * moment before the modal opens, and then every shot is taken through its scrim.
 * There is no positive signal for "the status landed and there was nothing to
 * show" — an already-provisioned sandbox renders no dialog at all — so this
 * waits out the window instead of waiting for an element.
 */
async function dismissFirstRunChrome(session, { graceMs = 5_000 } = {}) {
  const dialog = 'document.querySelector(\'[role="dialog"]\')';
  const deadline = Date.now() + graceMs;
  while (Date.now() < deadline) {
    if (await session.evaluate(`!!${dialog}`)) {
      await pressEscape(session);
      await session.waitFor(`!${dialog}`, { label: "the first-run dialog closed" });
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
}

/**
 * Imposes a CSS viewport on a window, independent of its real size.
 *
 * The scale factor is where the 2x comes from. It only changes the raster, not
 * the layout — `window.innerWidth` stays at the CSS width above, so the
 * composition is the one the constants describe and the extra pixels go into
 * sharper text. Handing the 2x to the capture's `clip.scale` instead reaches the
 * same file size by upscaling a 1x raster, which is visibly softer.
 */
async function setViewport(session, { width, height }) {
  await session.send("Emulation.setDeviceMetricsOverride", {
    width,
    height,
    deviceScaleFactor: SCALE,
    mobile: false,
  });
}

/**
 * Captures the viewport and writes it into `docs/screenshots/`.
 *
 * `fromSurface` is left at its default of true, and that is not a free choice:
 * this WebView2 build (Edg/151) caps a `fromSurface: false` capture at CSS
 * pixels, ignoring both `deviceScaleFactor` and `clip.scale`, so the renderer
 * path cannot produce a 2x image at all. The surface path honours both. The cost
 * is that a capture reads a real window surface, which is what makes the
 * quick-capture shot the awkward one — see its entry below.
 *
 * The written file is measured back out of its own IHDR rather than trusted. That
 * check is what caught the flag above: the shot came back correctly composed and
 * half-size, which is exactly the kind of wrong that survives review and only
 * shows up as a blurry README.
 */
async function shoot(session, file, viewport) {
  const { data } = await session.send(
    "Page.captureScreenshot",
    { format: "png" },
    { timeoutMs: 60_000 },
  );
  const png = Buffer.from(data, "base64");
  const size = pngSize(png);
  const expected = { width: viewport.width * SCALE, height: viewport.height * SCALE };
  if (size.width !== expected.width || size.height !== expected.height) {
    throw new Error(
      `${file} came out ${size.width}x${size.height}, expected ` +
        `${expected.width}x${expected.height} — the device-metrics override did not take`,
    );
  }
  await writeFile(new URL(file, OUT_DIR), png);
  console.log(`  wrote ${file}  ${size.width}x${size.height}`);
}

/** Width and height out of a PNG's IHDR, so `shoot` can check its own output. */
function pngSize(png) {
  return { width: png.readUInt32BE(16), height: png.readUInt32BE(20) };
}

/**
 * The main window's page target.
 *
 * `WINDOW.main` is `tauri.localhost/`, which is the PACKAGED url — under
 * `pnpm tauri dev` the same window is served by Vite and reads
 * `http://localhost:1420/`, so the e2e harness's constant finds nothing here.
 * Rather than hard-coding the dev port instead, this takes the one page whose
 * path is `/`: `main` is the only window that declares no `url` in
 * tauri.conf.json, so it is the only one without a page file in either build.
 */
async function findMainTarget(port, { timeoutMs = 20_000 } = {}) {
  const deadline = Date.now() + timeoutMs;
  let seen = [];
  while (Date.now() < deadline) {
    seen = await listTargets(port);
    const matches = seen.filter((target) => target.type === "page" && pathOf(target.url) === "/");
    if (matches.length === 1) {
      return matches[0];
    }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error(
    `no main-window target on 127.0.0.1:${port} after ${timeoutMs}ms; saw:\n` +
      (seen.map((target) => `  [${target.type}] ${target.url}`).join("\n") || "  (no targets)"),
  );
}

function pathOf(url) {
  try {
    return new URL(url).pathname;
  } catch {
    return "";
  }
}

/** Waits until the shell has actually rendered, not merely finished loading. */
async function waitForShell(session) {
  await session.waitFor(
    `document.readyState === "complete" && !!document.querySelector('[data-testid="sidebar-inbox-count"]')`,
    { label: "the shell mounted", timeoutMs: 30_000 },
  );
}

/* ---------- the shots ----------
   Ordered cheapest-first, and the two that cost something come last: chat
   spends a live Claude turn, and quick capture is the one shot that leaves the
   main window. Each `run` is handed the sessions it needs and leaves the app on
   whatever screen it photographed; nothing here depends on the previous shot's
   final state, so any subset can be named on the command line. */

const SHOTS = {
  inbox: {
    file: "inbox.png",
    async run({ shell, file }) {
      await clickLabelled(shell, "aside", "Inbox");
      // The rows arrive on a staggered cascade, so the settle is not politeness:
      // shoot too early and half the list is mid-rise at partial opacity.
      await shell.waitFor(`!!document.querySelector('[data-testid="inbox-list"]')`, {
        label: "the inbox list",
      });
      await settle(shell, 900);
      await shoot(shell, file, MAIN_VIEWPORT);
    },
  },

  note: {
    file: "note.png",
    async run({ shell, file }) {
      await clickLabelled(shell, "aside", HERO_PROJECT);
      await clickLabelled(shell, "main", HERO_NOTE);
      // The details rail resolves its own artifacts after the note renders, and
      // the audio and transcript chips are half of what this shot is for.
      await shell.waitFor(`!!document.querySelector('[data-testid="session-artifacts"]')`, {
        label: "the session rail",
        timeoutMs: 15_000,
      });
      await settle(shell, 900);
      await shoot(shell, file, MAIN_VIEWPORT);
    },
  },

  search: {
    file: "search.png",
    async run({ shell, file }) {
      await clickLabelled(shell, "aside", "Search");
      const field = 'input[aria-label="Search notes"]';
      await shell.waitFor(`!!document.querySelector(${lit(field)})`, { label: "the search field" });
      await typeIntoSelector(shell, field, SEARCH_QUERY);
      // Debounced, then a round trip to the index, so this waits on the results
      // rather than on a delay that would be a guess.
      await shell.waitFor(
        `(document.querySelectorAll('[role="listbox"][aria-label="Search results"] [role="option"]').length > 0)`,
        { label: `results for ${lit(SEARCH_QUERY)}`, timeoutMs: 20_000 },
      );
      await settle(shell, 700);
      await shoot(shell, file, MAIN_VIEWPORT);
    },
  },

  chat: {
    file: "chat.png",
    async run({ shell, file }) {
      // Via the Inbox, even when the chat view is already open: `useChatSession`
      // opens its backend session on mount, so leaving and coming back is what
      // guarantees a fresh, single-exchange conversation. Without it a second run
      // photographs the tail of a two-question thread.
      await clickLabelled(shell, "aside", "Inbox");
      await clickLabelled(shell, "aside", "Chat");
      const composer = 'textarea[aria-label="Message Claude"]';
      // A chat session that could not start renders the missing-Claude state
      // instead of a composer, and photographing that as "chat over your
      // history" would be worse than failing here.
      await shell.waitFor(`!!document.querySelector(${lit(composer)})`, {
        label: "the chat composer (is the `claude` CLI installed?)",
        timeoutMs: 30_000,
      });
      await typeIntoSelector(shell, composer, CHAT_PROMPT);
      await clickLabelled(shell, "form", "Send");
      // Mid-turn the composer's button is Stop; Send returning is the turn
      // ending. Waiting on the entries as well, because Send is also what is
      // there when the turn never started.
      await shell.waitFor(
        `!!document.querySelector('form button[type="submit"]') &&
         document.querySelectorAll('[aria-label="Conversation"] > *').length >= 2`,
        { label: "a finished answer", timeoutMs: 240_000, intervalMs: 500 },
      );
      // Back to the top of the log. The view follows a streaming answer down, so
      // by the time the turn lands the question that prompted it has scrolled off
      // — and a chat screenshot showing only an answer is a screenshot of nothing
      // in particular. The answer is longer than the column either way, so this
      // trades the citation chip at the foot for the question at the head.
      await shell.evaluate(
        `(document.querySelector('[aria-label="Conversation"]').scrollTop = 0, true)`,
      );
      await settle(shell, 900);
      await shoot(shell, file, MAIN_VIEWPORT);
    },
  },

  "quick-capture": {
    file: "quick-capture.png",
    async run({ sheet, file }) {
      await sheet.waitFor(`!!document.querySelector('[data-testid="quick-capture-input"]')`, {
        label: "the capture sheet mounted",
        timeoutMs: 30_000,
      });
      await setViewport(sheet, CAPTURE_VIEWPORT);
      // A backdrop, because this window is `transparent: true` and paints nothing
      // — capture.html forces `html, body, #root { background: transparent }` so
      // the sheet's rounded corners and shadow fall on the desktop rather than a
      // white rectangle. Photograph that as-is and the shadow falls on white
      // instead, which is the one thing the window is built to avoid.
      //
      // Set as an element style rather than by adding the `grove-ground` class:
      // that inline rule is unlayered, so it beats a Tailwind utility no matter
      // which element carries it. The value is still the theme's token, so this
      // stands the sheet on the same ground the other four shots are taken on
      // instead of inventing a colour — the same trick `preview-capture.html`
      // plays with its flat stand-in, for the same reason.
      await sheet.evaluate(
        `(document.documentElement.style.setProperty("background", "var(--color-ground)"), true)`,
      );
      await typeIntoSelector(sheet, '[data-testid="quick-capture-input"]', CAPTURE_DRAFT);
      // `quick-capture-route-preview` and not `quick-capture-destination`: the
      // latter is the filed flash, which only exists after a submit, and this
      // shot deliberately never submits — photographing the sheet must not write
      // a note into the vault it was seeded from. The preview is debounced and
      // then a real backend call, so waiting on it is what proves the routing
      // line in the shot is a live answer rather than an empty slot.
      await sheet.waitFor(
        `document.querySelector('[data-testid="quick-capture-route-preview"]')?.textContent.trim().length > 0`,
        { label: "the route preview resolved", timeoutMs: 20_000 },
      );
      await settle(sheet, 700);
      await shoot(sheet, file, CAPTURE_VIEWPORT);
    },
  },
};

async function main() {
  const requested = process.argv.slice(2);
  const unknown = requested.filter((name) => !(name in SHOTS));
  if (unknown.length > 0) {
    throw new Error(
      `unknown shot(s): ${unknown.join(", ")}\n` + `  known: ${Object.keys(SHOTS).join(", ")}`,
    );
  }
  const names = requested.length > 0 ? requested : Object.keys(SHOTS);

  const version = await waitForEndpoint(PORT, { timeoutMs: 10_000 }).catch(() => {
    throw new Error(
      `nothing is listening for CDP on 127.0.0.1:${PORT}.\n` +
        `  Launch the app with a debugging port first:\n` +
        `    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=${PORT}'\n` +
        `    pnpm dev:sandbox`,
    );
  });
  console.log(`${version.Browser} on 127.0.0.1:${PORT}`);

  const shell = await attach(await findMainTarget(PORT));
  // Attached whether or not the quick-capture shot was asked for: the window is
  // pre-created and hidden from startup, so this is cheap, and it keeps the
  // teardown below a single shape.
  const sheet = await attach(await findTarget(PORT, { urlEndsWith: WINDOW.quickCapture }));

  try {
    await waitForShell(shell);
    await dismissFirstRunChrome(shell);
    await setViewport(shell, MAIN_VIEWPORT);
    // The override relayouts the shell, and the views measure themselves.
    await settle(shell, 600);

    for (const name of names) {
      console.log(`${name}:`);
      await SHOTS[name].run({ shell, sheet, file: SHOTS[name].file });
    }
  } finally {
    // Hand both windows back at their real size, so the dev session the shots
    // were taken from is still usable afterwards.
    await shell.send("Emulation.clearDeviceMetricsOverride").catch(() => {});
    await sheet.send("Emulation.clearDeviceMetricsOverride").catch(() => {});
    shell.close();
    sheet.close();
  }
}

await main();
