/**
 * The second end-to-end slice: open a distilled note and check what survived
 * retention, across every state the source-pairing disclosure can reach.
 *
 * This is the slice the fixture catalogue was built for. The states differ only
 * in which files exist on disk beside a session's `.jsonl`, so reaching them
 * needs a real vault in a known shape — which is exactly what `pnpm test`
 * cannot do: it mocks `invoke` at the IPC boundary (`src/test/tauri.ts`), so it
 * never runs `read_session_artifacts` against a real file at all. Reaching them
 * by *driving* the app is not an option either; retention pruning a recording is
 * not a thing a user can do on demand.
 *
 * It also puts the CSP's `media-src` under test for the first time. That
 * directive has been annotated in `quick-capture.test.mjs` since the policy was
 * written, but nothing exercised it: the `<audio src={convertFileSrc(…)}>` in
 * `SessionPanel` is the app's only asset-protocol consumer, and that slice never
 * opens a note with a recording. Two scenarios here mount one.
 *
 * See docs/UI_E2E_HARNESS.md.
 */

import { after, before, test } from "node:test";
import assert from "node:assert/strict";

import { attach, findTarget } from "./lib/cdp.mjs";
import { launchKodabi, WINDOW } from "./lib/app.mjs";
import { cspComplaints } from "./lib/console.mjs";
import {
  allTextOf,
  byTestId,
  clickRowLabelled,
  clickWhenEnabled,
  settle,
  textOf,
  waitForText,
} from "./lib/page.mjs";

const EXE = process.env.KODABI_E2E_EXE ?? "target/debug/kodabi.exe";

/**
 * Seven scenarios, and deliberately not all ten.
 *
 * `sessions/needs-attention` is left out because it writes captures no note
 * claims, which would break the "every seeded session is claimed" tripwire
 * below — that scenario deserves its own slice, against the Needs-attention
 * view. `composition/at-ceiling` is left out because its claim is a design
 * judgement (three clusters, four controls) that a machine would assert badly.
 * `confidence/low-score` is a display string, which the jsdom tier already
 * covers.
 */
const SEED = [
  "retention/both",
  "retention/transcript-only",
  "retention/recording-only",
  "retention/nothing",
  "retention/empty-transcript",
  "transcript/fifty-turns",
  "source/keyword-only",
];

const PRUNED = "The raw transcript for this note is no longer stored.";

let app;
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
    app = await launchKodabi({ exe: EXE, seed: SEED });
    main = await attach(await findTarget(app.port, { urlEndsWith: WINDOW.main }));
    await main.waitFor(
      `document.readyState === "complete" && !!${byTestId("sidebar-inbox-count")}`,
      { label: "main mounted" },
    );
  } catch (error) {
    failed = true;
    throw error;
  }
});

after(async () => {
  if (failed) {
    console.error("--- main console ---\n" + (main?.consoleLog().join("\n") ?? ""));
    console.error("--- app stdio ---\n" + (app?.logs() ?? ""));
    console.error(`--- vault kept at ${app?.vaultDir} ---`);
  }
  main?.close();
  await app?.stop({ keepArtifacts: failed });
});

/** A seeded note, read off the manifest rather than restated here — a catalogue
 *  edit cannot leave this file chasing a title or a count that is gone. */
function noteOf(id) {
  const note = app.seeded.notes.find((candidate) => candidate.id === id);
  if (!note) {
    throw new Error(`no seeded note ${id}; seeded: ${app.seeded.notes.map((n) => n.id).join(", ")}`);
  }
  return note;
}

function titleOf(id) {
  return noteOf(id).title;
}

/** What the Transcript chip should read for a seeded note: the manifest counts
 * the words its own fixture turns hold, so this stays honest through a
 * catalogue edit. Joined by the session stem in the note's `source:`, which is
 * the same link the app itself resolves.
 *
 * `toLocaleString` on both sides, matching the component. Every fixture is
 * under a thousand words, where no locale inserts a separator — so this is an
 * exact match today, and the day a fixture crosses 999 it will say so rather
 * than drift. */
function transcriptChipOf(id) {
  const stem = noteOf(id).source.replace(/^sessions\//, "").replace(/\.jsonl$/, "");
  const session = app.seeded.sessions.find((candidate) => candidate.stem === stem);
  if (!session) {
    throw new Error(`note ${id} points at no seeded session (${stem})`);
  }
  return session.words === 1 ? "Transcript 1 word" : `Transcript ${session.words.toLocaleString()} words`;
}

/**
 * Opens a seeded Inbox note by title.
 *
 * Always via the dock, so each scenario starts from the same place whatever
 * the one before it left on screen. The wait is on `note-read` rather than on
 * anything this file asserts about — waiting for the source section would beg
 * the question every scenario below is about to ask.
 */
async function openInboxNote(title) {
  await clickWhenEnabled(main, "sidebar-inbox");
  await main.waitFor(`!!${byTestId("inbox-list")}`, { label: "the Inbox list" });
  await clickRowLabelled(main, "inbox-row", "inbox-row-title", title);
  await main.waitFor(`!!${byTestId("note-read")}`, { label: `the note screen for ${title}` });
}

/** Opens the transcript chip and waits for the turns to actually be there. */
async function expandSource() {
  await clickWhenEnabled(main, "session-source");
  await main.waitFor(`!!${byTestId("source-panel")}`, { label: "the transcript turns" });
}

scenario("the seeded vault reaches the Inbox", async () => {
  // Proves the startup reconcile and the disk listing both saw files this
  // harness wrote before the process existed. Every assertion below is
  // meaningless if this one is wrong, which is why it is first.
  const expected = String(app.seeded.notes.filter((note) => note.project === "Inbox").length);
  await main.waitFor(`${byTestId("sidebar-inbox-count")}?.textContent.trim() === "${expected}"`, {
    label: `sidebar inbox count reaches ${expected}`,
  });
});

scenario("every seeded session is claimed by a note", async () => {
  // A one-line guard for the nastiest failure mode in the catalogue: one
  // character wrong between a note's `source:` and its session stem, and the
  // fixture silently stops being a retention fixture and becomes a
  // Needs-attention fixture instead — with the retention scenarios below still
  // passing, because the note is fine, it just points at nothing.
  //
  // The dock row is absent entirely at zero (`Dock.tsx` returns null), and it
  // keeps the row on a failed listing, so this cannot pass by the listing
  // having errored.
  assert.equal(await textOf(main, "needs-attention-nav"), null);
});

scenario("each surviving artifact gets its own chip, opened independently", async () => {
  await openInboxNote(titleOf("n_both0001"));
  await waitForText(main, "session-source", transcriptChipOf("n_both0001"));
  assert.equal(await textOf(main, "session-source-pruned"), null);

  // A chip each, and each opens ONE thing. The recording opens in place — the
  // rail holds a player — while the turns need the reading measure and land in
  // the body column, which is why they are two controls rather than one.
  await expandSource();
  assert.equal((await allTextOf(main, "session-turn")).length, 3);
  assert.equal(await textOf(main, "reveal-recording"), null, "audio stayed shut");

  await clickWhenEnabled(main, "session-audio");
  await main.waitFor(`!!${byTestId("reveal-recording")}`, { label: "the recording opens" });
  // ...and opening one did not close the other.
  assert.equal((await allTextOf(main, "session-turn")).length, 3);

  // The player stays MOUNTED when its chip closes — the deliberate reversal of
  // the old section's accepted consequence, which stopped playback the moment
  // you went back to reading. Hidden, not gone.
  await clickWhenEnabled(main, "session-audio");
  await main.waitFor(`!${byTestId("reveal-recording")}`, { label: "the recording shuts" });
  const player = await main.evaluate(`(() => {
    const audio = ${byTestId("recording-player")};
    return audio ? { mounted: true, visible: audio.offsetParent !== null } : { mounted: false };
  })()`);
  assert.ok(player.mounted, "the player unmounted; closing the chip must only hide it");
  assert.equal(player.visible, false, "the player stayed visible after closing the chip");
});

scenario("the retained recording loads through the asset protocol", async () => {
  // The one assertion that gates the asset-protocol scope, and the only one in
  // the repo that does. `assetProtocol.scope` in tauri.conf.json is static
  // (`$APPDATA/sessions/*.wav`), but `knowledge_base_dir` honours
  // `KODABI_KB_ROOT` — so before `setup` widened the scope to the resolved
  // vault, this element rendered a transport that would not play, and every
  // other scenario in this file still passed. Tauri refuses an out-of-scope
  // asset before the CSP is consulted, so the console scenario below could not
  // have caught it either.
  //
  // `readyState >= 1` is HAVE_METADATA: the fetch was served and decoded as
  // audio, which is also what proves the generated RIFF header is real rather
  // than merely present on disk.
  // No click first: the player is mounted at rest so the chip can read a real
  // duration off it, which means this probes the preload rather than a fetch a
  // disclosure triggered.
  await openInboxNote(titleOf("n_both0001"));
  await main.waitFor(`!!${byTestId("recording-player")}`, { label: "the player mounts" });

  const probe = await main.evaluate(`
    new Promise((resolve) => {
      const audio = ${byTestId("recording-player")};
      if (!audio) {
        resolve({ ok: false, detail: "no <audio> mounted" });
        return;
      }
      const settle = () =>
        resolve({
          ok: audio.readyState >= 1,
          detail: audio.error
            ? "media error code " + audio.error.code + " for " + audio.currentSrc
            : "readyState " + audio.readyState,
        });
      if (audio.readyState >= 1 || audio.error) {
        settle();
        return;
      }
      audio.addEventListener("loadedmetadata", settle, { once: true });
      audio.addEventListener("error", settle, { once: true });
      setTimeout(settle, 5000);
    })
  `);
  assert.ok(probe.ok, `the recording did not load: ${probe.detail}`);
});

scenario("a pruned recording leaves the transcript", async () => {
  await openInboxNote(titleOf("n_tran0002"));
  await waitForText(main, "session-source", transcriptChipOf("n_tran0002"));
  assert.equal(await textOf(main, "session-source-pruned"), null);
  // No artifact, no chip: the rail says nothing about a recording that is gone.
  assert.equal(await textOf(main, "session-audio"), null, "the recording is gone");

  await expandSource();
  assert.equal((await allTextOf(main, "session-turn")).length, 3);
  assert.equal(await textOf(main, "recording-player"), null, "the recording is gone");
});

scenario("a pruned transcript leaves the recording", async () => {
  await openInboxNote(titleOf("n_reco0003"));
  await main.waitFor(`!!${byTestId("session-audio")}`, { label: "the audio chip" });

  // At rest, before any click: this sentence's whole job is that "checked
  // against the source" never fails silently, which it would do until a click
  // if it sat behind a chip.
  assert.equal(await textOf(main, "session-source-pruned"), PRUNED);
  assert.equal(await textOf(main, "session-source"), null, "nothing to transcribe");

  await clickWhenEnabled(main, "session-audio");
  await main.waitFor(`!!${byTestId("reveal-recording")}`, { label: "the recording opens" });
  assert.equal((await allTextOf(main, "session-turn")).length, 0);
});

scenario("nothing survived means nothing to press", async () => {
  await openInboxNote(titleOf("n_none0004"));
  await main.waitFor(`!!${byTestId("session-artifacts")}`, { label: "the session panel" });

  // The panel mounts and fetches, and then offers no control at all — a chip
  // over an empty artifact would be worse than no chip.
  assert.equal(await textOf(main, "session-source"), null, "expected no transcript chip");
  assert.equal(await textOf(main, "session-audio"), null, "expected no audio chip");
  assert.equal(await textOf(main, "session-source-pruned"), PRUNED);
});

scenario("an empty transcript still counts as a transcript", async () => {
  await openInboxNote(titleOf("n_mpty0005"));

  // `transcript_available` is true — the file is there, it just holds nothing —
  // so there is a chip, a zero count, and no pruned sentence. This is a
  // documented soft spot rather than settled behaviour; the fixture pins it so a
  // change to it is visible rather than incidental.
  await waitForText(main, "session-source", transcriptChipOf("n_mpty0005"));
  assert.equal(await textOf(main, "session-source-pruned"), null);

  await expandSource();
  assert.equal((await allTextOf(main, "session-turn")).length, 0);
});

scenario("a long transcript is not capped", async () => {
  await openInboxNote(titleOf("n_turn0008"));
  await waitForText(main, "session-source", transcriptChipOf("n_turn0008"));

  await expandSource();
  // Two different claims, and the second is the one worth having: the chip
  // above counts words in the payload, this counts the turns that actually
  // reached the DOM. A future `.slice(0, 20)` or a virtualized list breaks only
  // this one, and `TranscriptTurns`'s doc comment promises neither ever happens.
  assert.equal((await allTextOf(main, "session-turn")).length, 50);
});

scenario("a keyword source never pairs", async () => {
  await openInboxNote(titleOf("n_keyw0009"));
  await settle(main);
  assert.equal(await textOf(main, "session-artifacts"), null);
});

scenario("a path source that is not a session source never pairs", async () => {
  // `chats/<stamp>.jsonl` passes `endsWith(".jsonl")` and fails
  // `startsWith("sessions/")`, so it must render exactly like the keyword note
  // above. This is the only place the first half of `isSessionSource` is under
  // test at all — drop it and every other scenario in this file still passes.
  await openInboxNote(titleOf("n_chat0010"));
  await settle(main);
  assert.equal(await textOf(main, "session-artifacts"), null);
});

scenario("no webview logged a CSP refusal or a degraded IPC path", async () => {
  // Last on purpose: by now two scenarios have mounted a real
  // <audio src={convertFileSrc(…)}>, which is the app's only `media-src`
  // consumer and was never reached by any test before this file existed.
  //
  // No sleep before reading the buffer. CDP delivers events and command replies
  // over one ordered socket, and a refusal is logged the moment the request is
  // blocked — strictly before the promise that request feeds can settle.
  //
  // deepEqual against [] rather than a length check, so a failure prints the
  // offending console lines themselves.
  assert.deepEqual(cspComplaints(main), [], "the main webview hit the CSP");
});
