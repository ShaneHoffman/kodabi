/**
 * A named catalogue of vault fixtures — the scenarios the E2E slices assert
 * against, and the ones `pnpm seed:vault` puts on screen for a manual preview.
 *
 * Scenarios are named after the RULE they exercise, not the data they hold, so
 * a review note can say "open `retention/recording-only`" and it means
 * something.
 *
 * FILES ONLY. Nothing here writes an index row: `IndexState::initialize` runs a
 * startup Reconcile over whatever is on disk, so the app indexes the fixture
 * itself and the seeder exercises the real convergence path rather than a
 * parallel one that would drift from what ships. That is also why
 * `launchKodabi` seeds BEFORE it spawns, and the ordering is load-bearing twice
 * over — `watch.rs` ignores everything under `sessions/`, so a transcript
 * written after launch is not merely late to the index, it is never seen at all.
 *
 * Timestamps are minted at seed time rather than frozen into the catalogue.
 * `retention::start_schedule` runs an immediate prune sweep at launch, and
 * settings are NOT isolated from the developer's real ones (see *Isolation* in
 * e2e/README.md), so on a machine whose retention policy is `keep_days` a
 * fixture dated last month would be deleted out from under the run: every
 * retention scenario would collapse into `retention/nothing` and the slice would
 * go red for something that is not a bug. A session captured ninety minutes ago
 * is past no cutoff anyone can set. Do not "simplify" this back to literal
 * stems.
 */

import { mkdir, readdir, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";

/**
 * The marker that makes a directory re-seedable.
 *
 * `seedVault` wipes before it writes, and the root is caller-supplied — so
 * without a marker the difference between "my scratch fixture" and "my actual
 * notes" would be one mistyped path. A directory this seeder wrote carries the
 * marker and is wiped freely; anything else is refused.
 */
export const MARKER_FILE = ".kodabi-fixture-vault";

/**
 * Where a seeded vault's index belongs.
 *
 * One function rather than a string repeated in the CLI, the docs and the
 * preview skill, because `KODABI_INDEX_DB` is half of a pair that is destructive
 * to get wrong and three copies of a path is three chances to drift. The leading
 * dot is doing real work: `is_project_segment` rejects it so the vault walk skips
 * the folder, and `watch.rs` takes `.md` files only so SQLite's own writes cannot
 * loop the reconciler.
 */
export function indexDbFor(root) {
  return join(root, ".index", "index.db");
}

/**
 * Exactly 8 lowercase base36 characters.
 *
 * `DeviceId::parse` rejects any other length, and `parse_session_filename` then
 * returns None — which costs a Needs-attention row its title AND its capture
 * instant (it falls back to mtime), silently, with nothing logged.
 */
const DEVICE_ID = "k4m2xp7q";

/** Mono 16-bit PCM at this rate; see `wav`. */
const WAV_SAMPLE_RATE = 22_050;

// ---------------------------------------------------------------------------
// The catalogue
// ---------------------------------------------------------------------------

/** Three turns of a budget conversation, the default transcript body. */
const BUDGET_TURNS = [
  ["you", 0, 4_200, "Morning. Ready to walk the Q3 budget?"],
  ["them", 4_200, 11_800, "Yes, though the irrigation line moved again."],
  ["unknown", 65_000, 71_000, "Could we circle back to the drainage bid?"],
];

/** The at-ceiling meeting, which is the one fixture meant to read as real. */
const CEILING_TURNS = [
  ["you", 0, 5_400, "Let's start with the revised irrigation number."],
  ["them", 5_400, 14_900, "Forty-two thousand, and GreenFlow leads the shortlist."],
  ["you", 61_000, 68_500, "Two alternates as well, so the bid stays honest."],
  ["them", 120_000, 129_000, "I'll send the memo to jane@example.com this week."],
];

const DRAINAGE_TURNS = [
  ["you", 0, 6_100, "The clubhouse drainage survey came back short."],
  ["them", 6_100, 13_400, "Then we re-bid it before the irrigation work starts."],
];

const WAVED_OFF_TURNS = [["you", 0, 3_300, "Wrong meeting, sorry."]];

/**
 * Fifty turns of alternating chatter.
 *
 * Generated rather than written out because the count IS the fixture: this
 * exists to prove `Turns` renders every segment it is handed, and the 75-second
 * spacing pushes the last offset past an hour so `formatOffset`'s `h:mm:ss`
 * branch renders too.
 */
const FIFTY_TURNS = Array.from({ length: 50 }, (_, index) => [
  index % 2 === 0 ? "you" : "them",
  index * 75_000,
  index * 75_000 + 9_000,
  index % 2 === 0
    ? `Point ${index + 1}: the irrigation schedule still assumes the old shift.`
    : "Noted. I'll fold that into the Briarwood Golf revision.",
]);

/**
 * Every scenario, as data rather than builder functions.
 *
 * Data because three consumers need to read the catalogue without executing it:
 * `--list` prints `why`, a slice reads titles off the manifest, and
 * `assertCatalogue` needs to see all ten at once to check the properties no
 * single scenario can state about itself.
 *
 * A session declares `minutesAgo` and the artifacts that survived; `artifacts:
 * []` still reserves the stem, which is how a note can point at a session whose
 * every file retention already took. A note's `source` is `{ session: key }`,
 * `{ chatMinutesAgo: n }`, or one of the five schema keywords.
 */
export const SCENARIOS = {
  "retention/both": {
    why: "Both artifacts survived — the maximal toggle, `Source · recording · 3 segments`.",
    sessions: [
      {
        key: "both",
        minutesAgo: 90,
        slug: "both-artifacts",
        artifacts: ["jsonl", "wav"],
        wavSeconds: 8,
        turns: BUDGET_TURNS,
      },
    ],
    notes: [
      {
        id: "n_both0001",
        project: "Inbox",
        file: "both-artifacts-survived",
        type: "meeting",
        title: "Both artifacts survived",
        dateFrom: "both",
        tags: ["budgeting"],
        source: { session: "both" },
        confidence: 0.92,
        body: "The maximal reading surface. Expect `Source · recording · 3 segments` at rest, and both the player and the turns behind the one toggle.",
      },
    ],
  },

  "retention/transcript-only": {
    why: "Retention kept the transcript and pruned the recording — `Source · 3 segments`.",
    sessions: [
      {
        key: "transcript",
        minutesAgo: 150,
        slug: "transcript-only",
        artifacts: ["jsonl"],
        turns: BUDGET_TURNS,
      },
    ],
    notes: [
      {
        id: "n_tran0002",
        project: "Inbox",
        file: "transcript-only",
        type: "meeting",
        title: "Transcript only",
        dateFrom: "transcript",
        source: { session: "transcript" },
        confidence: 0.88,
        body: "Expect `Source · 3 segments`, no pruned sentence, and no player once opened.",
      },
    ],
  },

  "retention/recording-only": {
    why: "The transcript is gone and the recording is not — the pruned sentence sits at rest, the player stays behind the toggle.",
    sessions: [
      {
        key: "recording",
        minutesAgo: 210,
        slug: "recording-only",
        artifacts: ["wav"],
        wavSeconds: 5,
      },
    ],
    notes: [
      {
        id: "n_reco0003",
        project: "Inbox",
        file: "recording-only",
        type: "meeting",
        title: "Recording only",
        dateFrom: "recording",
        source: { session: "recording" },
        confidence: 0.81,
        body: "Expect `Source · recording` plus the visible no-longer-stored sentence, and only the player behind the toggle.",
      },
    ],
  },

  "retention/nothing": {
    why: "Both artifacts pruned — the sentence alone, and nothing at all to press.",
    sessions: [{ key: "pruned", minutesAgo: 270, slug: "nothing-survived", artifacts: [] }],
    notes: [
      {
        id: "n_none0004",
        project: "Inbox",
        file: "nothing-survived",
        type: "meeting",
        title: "Nothing survived",
        dateFrom: "pruned",
        source: { session: "pruned" },
        confidence: 0.74,
        body: "Expect the sentence and zero buttons in this section. A disclosure over an empty panel would be worse than no disclosure.",
      },
    ],
  },

  "retention/empty-transcript": {
    why: "A transcript that exists and holds nothing — `Source · 0 segments`, opening onto an empty panel.",
    sessions: [
      { key: "empty", minutesAgo: 330, slug: "empty-transcript", artifacts: ["jsonl"], turns: [] },
    ],
    notes: [
      {
        id: "n_mpty0005",
        project: "Inbox",
        file: "empty-transcript",
        type: "meeting",
        title: "Empty transcript",
        dateFrom: "empty",
        source: { session: "empty" },
        confidence: 0.69,
        body: "A captured session where nothing was ever transcribed. Expect `Source · 0 segments` and an empty panel. The soft spot is real; this fixture pins it rather than pretending it is not there.",
      },
    ],
  },

  "composition/at-ceiling": {
    why: "A filed session note at the composition ceiling — three clusters, four controls (back link, Edit, Delete note, Source). Preview-only.",
    sessions: [
      {
        key: "budget",
        minutesAgo: 45,
        slug: "q3-budget-sync",
        artifacts: ["jsonl", "wav"],
        wavSeconds: 8,
        turns: CEILING_TURNS,
      },
    ],
    notes: [
      {
        id: "n_ceil0006",
        // The only note outside the Inbox, and deliberately: the ceiling counts
        // the back link, which only points somewhere from a project.
        project: "Briarwood Golf",
        file: "q3-budget-and-irrigation-shortlist",
        type: "meeting",
        title: "Q3 budget and irrigation contractor shortlist",
        dateFrom: "budget",
        tags: ["budgeting", "phase-2"],
        source: { session: "budget" },
        confidence: 0.94,
        body: [
          "# Summary",
          "",
          "Reviewed Q3 budget allocation for the course renovation and agreed on the",
          "irrigation contractor shortlist.",
          "",
          "## Decisions",
          "",
          "- Approved the revised irrigation budget of $42,000.",
          "- Selected GreenFlow Systems as the lead contractor for bidding.",
          "",
          "## Action items",
          "",
          "- [ ] Jane to send the signed budget memo to finance.",
          "- [ ] Priya to request formal bids from GreenFlow and two alternates.",
        ].join("\n"),
      },
    ],
  },

  "sessions/needs-attention": {
    why: "Two captures no note ever claimed — one active, one behind the dismissed shelf. The only scenario that writes no note, which is exactly what leaves its sessions unclaimed.",
    sessions: [
      {
        key: "lost",
        minutesAgo: 25,
        slug: "lost-drainage-call",
        artifacts: ["jsonl", "wav"],
        wavSeconds: 5,
        turns: DRAINAGE_TURNS,
      },
      {
        key: "wavedOff",
        minutesAgo: 35,
        slug: "waved-off-capture",
        artifacts: ["jsonl", "dismissed"],
        turns: WAVED_OFF_TURNS,
      },
    ],
    notes: [],
  },

  "confidence/low-score": {
    why: "An Inbox note the router placed with little confidence — the row reads `31% match`, well under the 0.6 auto-file threshold.",
    sessions: [],
    notes: [
      {
        id: "n_conf0007",
        project: "Inbox",
        file: "check-clubhouse-drainage",
        type: "note",
        title: "Check whether the irrigation contractor also handles clubhouse drainage",
        dateFrom: "today",
        tags: ["idea"],
        source: "quick-capture",
        confidence: 0.31,
        body: "Routing found evidence for two projects and preferred neither by enough. Expect a low match score on the Inbox row.",
      },
    ],
  },

  "transcript/fifty-turns": {
    why: "Fifty segments, to prove the transcript is uncapped and unvirtualized — and to push the last offset past an hour.",
    sessions: [
      { key: "long", minutesAgo: 390, slug: "fifty-turns", artifacts: ["jsonl"], turns: FIFTY_TURNS },
    ],
    notes: [
      {
        id: "n_turn0008",
        project: "Inbox",
        file: "fifty-turn-transcript",
        type: "meeting",
        title: "Fifty turn transcript",
        dateFrom: "long",
        source: { session: "long" },
        confidence: 0.9,
        // No .wav on purpose: fifty turns at 75s spacing is a 61-minute
        // meeting, and an 8-second placeholder tone beside it would be the
        // fixture lying about what it holds.
        body: "Expect `Source · 50 segments`, every one of them rendered, and the last offsets in `h:mm:ss`.",
      },
    ],
  },

  "source/keyword-only": {
    why: "The two shapes that must NOT pair: a keyword source, and a path source that is not a session source.",
    sessions: [],
    notes: [
      {
        id: "n_keyw0009",
        project: "Inbox",
        file: "a-thought-typed-by-hand",
        type: "note",
        title: "A thought typed by hand",
        dateFrom: "today",
        source: "quick-capture",
        confidence: 0.45,
        body: "Expect no source section below this body at all, and no artifact fetch.",
      },
      {
        id: "n_chat0010",
        project: "Inbox",
        file: "chat-about-the-drainage-bid",
        type: "chat",
        title: "Chat about the drainage bid",
        dateFrom: "today",
        // The `isSessionSource` boundary: this passes `endsWith(".jsonl")` and
        // fails `startsWith("sessions/")`, so it must render exactly like the
        // keyword note above. The chat file is deliberately NOT written — the
        // schema requires readers to tolerate a `source` path that no longer
        // resolves, and modelling a second on-disk format for a file nothing in
        // this flow reads would be weight for nothing.
        chatMinutesAgo: 55,
        source: { chat: true },
        confidence: 0.58,
        body: "A path source that is not a session source. Expect no source section, for the same reason as the keyword note.",
      },
    ],
  },
};

/** The catalogue in declared order — the CLI's `--list`, and the "all" default. */
export const SCENARIO_NAMES = Object.keys(SCENARIOS);

// ---------------------------------------------------------------------------
// Emitters
// ---------------------------------------------------------------------------

const pad = (value) => String(value).padStart(2, "0");

/** `%Y%m%dT%H%M%S%3fZ` — kodabi_core::naming::TIMESTAMP_FORMAT. */
function utcStamp(when) {
  return when.toISOString().replace(/[-:]/g, "").replace(".", "");
}

/**
 * RFC 3339 carrying the machine's own offset — the canonical `date` shape in
 * docs/FRONTMATTER_SCHEMA.md, and what `distill` itself writes.
 *
 * A naive timestamp is rejected outright, and a rejected note does not fail
 * loudly: `Note::from_markdown` errors, the file drops out of the disk listing,
 * and the scenario simply is not there.
 */
function localIso(when) {
  const offsetMinutes = -when.getTimezoneOffset();
  const sign = offsetMinutes < 0 ? "-" : "+";
  const absolute = Math.abs(offsetMinutes);
  return (
    `${when.getFullYear()}-${pad(when.getMonth() + 1)}-${pad(when.getDate())}` +
    `T${pad(when.getHours())}:${pad(when.getMinutes())}:${pad(when.getSeconds())}` +
    `${sign}${pad(Math.floor(absolute / 60))}:${pad(absolute % 60)}`
  );
}

/** The local calendar date, which is what quick capture writes. */
function localDate(when) {
  return `${when.getFullYear()}-${pad(when.getMonth() + 1)}-${pad(when.getDate())}`;
}

/**
 * One JSONL line per kodabi_core::raw_session::TranscriptSegment.
 *
 * `speaker` is always present and always null — the struct carries
 * `#[serde(default)]` but serializes it explicitly for parity with the
 * documented schema, so omitting the key is not the same shape.
 */
function segment(index, [channel, startMs, endMs, text]) {
  return JSON.stringify({
    index,
    channel,
    speaker: null,
    start_ms: startMs,
    end_ms: endMs,
    text,
  });
}

/**
 * A real, playable mono 16-bit PCM WAV: a 44-byte RIFF header plus a quiet
 * 440 Hz tone.
 *
 * Generated, never committed — this is a public AGPL repo and it stays
 * text-only. Playable rather than a bare header because `audio_sibling` only
 * checks `is_file()`, but the `<audio>` transport needs valid PCM to exercise
 * playback at all.
 */
function wav(seconds) {
  const frames = WAV_SAMPLE_RATE * seconds;
  const data = Buffer.alloc(frames * 2);
  for (let index = 0; index < frames; index += 1) {
    const value = Math.round(6000 * Math.sin((2 * Math.PI * 440 * index) / WAV_SAMPLE_RATE));
    data.writeInt16LE(value, index * 2);
  }
  const header = Buffer.alloc(44);
  header.write("RIFF", 0);
  header.writeUInt32LE(36 + data.length, 4);
  header.write("WAVE", 8);
  header.write("fmt ", 12);
  header.writeUInt32LE(16, 16); // fmt chunk size
  header.writeUInt16LE(1, 20); // PCM
  header.writeUInt16LE(1, 22); // mono
  header.writeUInt32LE(WAV_SAMPLE_RATE, 24);
  header.writeUInt32LE(WAV_SAMPLE_RATE * 2, 28); // byte rate
  header.writeUInt16LE(2, 32); // block align
  header.writeUInt16LE(16, 34); // bits per sample
  header.write("data", 36);
  header.writeUInt32LE(data.length, 40);
  return Buffer.concat([header, data]);
}

/** Always a decimal point: `1.0`, never `1`. `String(1.0)` in JS is `"1"`. */
const decimal = (value) => (Number.isInteger(value) ? value.toFixed(1) : String(value));

/**
 * Refuses a value the emitter would have to quote, rather than shipping a YAML
 * quoter this fixture has no use for. Fixture text stays plain.
 */
function plainScalar(key, value) {
  if (!/^[A-Za-z][A-Za-z0-9 ',&.-]*$/.test(value)) {
    throw new Error(`fixture ${key} ${JSON.stringify(value)} would need YAML quoting`);
  }
  return value;
}

/**
 * The canonical frontmatter block, byte for byte (docs/FRONTMATTER_SCHEMA.md,
 * *Serialization contract*): keys in canonical order, the closing fence, then
 * exactly one blank line before the body.
 *
 * Three rules a hand-written fixture gets wrong every time, and each of them
 * makes the note VANISH rather than fail — `Note::from_markdown` rejects it, so
 * the file drops out of the disk listing and reconcile files it under
 * `report.skipped`, with nothing on screen to say so:
 *   - `tags: []` is a hard error; omit the key entirely when empty.
 *   - `confidence` must carry a decimal point.
 *   - an Inbox note without `confidence` is invalid, because the sentinel is
 *     always routing-scored (`Routing::from_project_and_confidence`).
 */
function frontmatter({ id, type, title, project, date, tags, source, confidence }) {
  const lines = ["---", `id: ${id}`, `type: ${type}`];
  if (title) {
    lines.push(`title: ${plainScalar("title", title)}`);
  }
  lines.push(`project: ${plainScalar("project", project)}`, `date: ${date}`);
  if (tags.length > 0) {
    lines.push(`tags: [${tags.join(", ")}]`);
  }
  lines.push(`source: ${source}`);
  if (confidence !== null) {
    lines.push(`confidence: ${decimal(confidence)}`);
  }
  lines.push("---", "");
  return `${lines.join("\n")}\n`;
}

// ---------------------------------------------------------------------------
// Catalogue invariants
// ---------------------------------------------------------------------------

/**
 * The properties a scenario cannot state about itself, checked once at import
 * so a catalogue edit fails at `node -e "import(…)"` rather than three layers
 * into a preview.
 *
 * Every one of these fails SILENTLY at runtime. A duplicate id makes reconcile
 * drop the loser into `report.duplicates`, so the note still lists from disk but
 * is unsearchable and unaddressable — a half-failure, which is worse than a
 * whole one. A `source` naming a session no scenario writes turns a retention
 * fixture into a phantom Needs-attention row. And a repeated `minutesAgo` would
 * collide two stems into one file.
 */
function assertCatalogue() {
  const ids = new Set();
  const instants = new Set();
  const filesByProject = new Map();

  for (const [name, scenario] of Object.entries(SCENARIOS)) {
    const sessionKeys = new Set(scenario.sessions.map((entry) => entry.key));

    for (const entry of scenario.sessions) {
      if (instants.has(entry.minutesAgo)) {
        throw new Error(`${name}: minutesAgo ${entry.minutesAgo} is already taken`);
      }
      instants.add(entry.minutesAgo);
    }

    for (const note of scenario.notes) {
      if (!/^n_[0-9a-z]{6,}$/.test(note.id)) {
        throw new Error(`${name}: note id ${note.id} does not match the schema pattern`);
      }
      if (ids.has(note.id)) {
        throw new Error(`${name}: note id ${note.id} is a duplicate`);
      }
      ids.add(note.id);

      if (note.project === "Inbox" && typeof note.confidence !== "number") {
        throw new Error(`${name}: Inbox note ${note.id} must carry a confidence`);
      }
      for (const tag of note.tags ?? []) {
        if (!/^[a-z0-9]+(-[a-z0-9]+)*$/.test(tag)) {
          throw new Error(`${name}: tag ${JSON.stringify(tag)} is not schema-shaped`);
        }
      }
      if (typeof note.source === "object" && note.source.session !== undefined) {
        if (!sessionKeys.has(note.source.session)) {
          throw new Error(
            `${name}: note ${note.id} claims session "${note.source.session}", which the scenario does not write`,
          );
        }
      }
      if (note.chatMinutesAgo !== undefined) {
        if (instants.has(note.chatMinutesAgo)) {
          throw new Error(`${name}: chatMinutesAgo ${note.chatMinutesAgo} is already taken`);
        }
        instants.add(note.chatMinutesAgo);
      }

      const seen = filesByProject.get(note.project) ?? new Set();
      if (seen.has(note.file)) {
        throw new Error(`${name}: ${note.project}/${note.file}.md is a duplicate`);
      }
      seen.add(note.file);
      filesByProject.set(note.project, seen);
    }
  }
}

assertCatalogue();

// ---------------------------------------------------------------------------
// Seeding
// ---------------------------------------------------------------------------

/**
 * Takes ownership of `root`, wiping it if this seeder is allowed to.
 *
 * The marker is the entire safety mechanism, so this runs before any write and
 * `force` is never implied. Dot-entries survive the wipe: the index this fixture
 * tells you to open lives in `.index/`, and on Windows an open SQLite file
 * cannot be removed — a wipe that took it out would fail every time the app was
 * still running. Reconcile converges the surviving index against the new
 * scenario set at the next startup anyway.
 */
async function claimRoot(root, force) {
  const entries = await readdir(root).catch((error) => {
    if (error.code === "ENOENT") {
      return null;
    }
    throw error;
  });
  if (entries === null || entries.length === 0) {
    return;
  }
  if (!entries.includes(MARKER_FILE) && !force) {
    throw new Error(
      `${root} is not empty and carries no ${MARKER_FILE} — refusing to wipe it.\n` +
        `Point at a fresh directory, or pass --force if you are certain.`,
    );
  }
  for (const entry of entries) {
    if (entry.startsWith(".")) {
      continue;
    }
    await rm(join(root, entry), { recursive: true, force: true });
  }
}

/**
 * Writes the named scenarios into `root` and returns a manifest of what landed.
 *
 * The manifest exists so a slice asserts against the titles and stems actually
 * written rather than restating catalogue strings — a catalogue edit then cannot
 * leave a test chasing a title that is gone.
 */
export async function seedVault(root, scenarioNames = SCENARIO_NAMES, { force = false } = {}) {
  // Before `claimRoot`, so a typo in a scenario name can never trigger a wipe.
  const unknown = scenarioNames.filter((name) => !(name in SCENARIOS));
  if (unknown.length > 0) {
    throw new Error(
      `unknown scenario${unknown.length === 1 ? "" : "s"}: ${unknown.join(", ")}\n` +
        `known: ${SCENARIO_NAMES.join(", ")}`,
    );
  }

  await claimRoot(root, force);
  await mkdir(join(root, "sessions"), { recursive: true });
  await writeFile(
    join(root, MARKER_FILE),
    "Written by e2e/lib/vault.mjs. Its presence lets `pnpm seed:vault` wipe this directory.\n",
  );

  const base = new Date();
  const at = (minutesAgo) => new Date(base.getTime() - minutesAgo * 60_000);
  const manifest = { root, indexDb: indexDbFor(root), scenarios: [...scenarioNames], notes: [], sessions: [] };
  const stems = new Map();

  for (const name of scenarioNames) {
    const scenario = SCENARIOS[name];

    for (const entry of scenario.sessions) {
      const capturedAt = at(entry.minutesAgo);
      const stem = `${utcStamp(capturedAt)}-${DEVICE_ID}-${entry.slug}`;
      stems.set(`${name}:${entry.key}`, { stem, capturedAt });

      const written = [];
      if (entry.artifacts.includes("jsonl")) {
        const body = (entry.turns ?? []).map((turn, index) => segment(index, turn)).join("\n");
        // A trailing newline after the last line, and nothing at all when there
        // are no turns — an empty file is the `Source · 0 segments` state, which
        // `read_raw_session` parses fine.
        await writeFile(join(root, "sessions", `${stem}.jsonl`), body === "" ? "" : `${body}\n`);
        written.push(`${stem}.jsonl`);
      }
      if (entry.artifacts.includes("wav")) {
        // The exact same stem, because `naming::audio_sibling` is
        // `with_extension("wav")` — derived from the transcript path, never
        // composed alongside it.
        await writeFile(join(root, "sessions", `${stem}.wav`), wav(entry.wavSeconds));
        written.push(`${stem}.wav`);
      }
      if (entry.artifacts.includes("dismissed")) {
        // One RFC 3339 line. The content is never parsed; presence is the whole
        // signal.
        await writeFile(
          join(root, "sessions", `${stem}.dismissed`),
          `${capturedAt.toISOString().replace(/\.\d{3}Z$/, "Z")}\n`,
        );
        written.push(`${stem}.dismissed`);
      }

      manifest.sessions.push({
        scenario: name,
        key: entry.key,
        stem,
        capturedAt: capturedAt.toISOString(),
        files: written,
      });
    }

    for (const note of scenario.notes) {
      let source;
      if (typeof note.source !== "object") {
        source = note.source;
      } else if (note.source.chat) {
        source = `chats/${utcStamp(at(note.chatMinutesAgo))}-${DEVICE_ID}.jsonl`;
      } else {
        source = `sessions/${stems.get(`${name}:${note.source.session}`).stem}.jsonl`;
      }

      const date =
        note.dateFrom === "today"
          ? localDate(base)
          : localIso(stems.get(`${name}:${note.dateFrom}`).capturedAt);

      const dir = join(root, ...note.project.split("/"));
      await mkdir(dir, { recursive: true });
      const path = join(dir, `${note.file}.md`);
      // `frontmatter` already ends with the closing fence AND the one blank
      // separator line, so the body follows it directly. A second newline here
      // would put two blank lines after the fence.
      await writeFile(
        path,
        `${frontmatter({
          id: note.id,
          type: note.type,
          title: note.title,
          project: note.project,
          date,
          tags: note.tags ?? [],
          source,
          confidence: note.confidence ?? null,
        })}${note.body}\n`,
      );

      manifest.notes.push({
        scenario: name,
        id: note.id,
        project: note.project,
        title: note.title,
        file: `${note.file}.md`,
        path,
        source,
      });
    }
  }

  return manifest;
}
