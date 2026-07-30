/**
 * `pnpm seed:vault <dir> [scenario…]` — writes the fixture catalogue to a
 * directory so the app can be previewed against known states.
 *
 * The catalogue itself lives in `e2e/lib/vault.mjs`, shared with the E2E
 * harness, so a preview and a slice can never disagree about what
 * `retention/recording-only` means.
 *
 * Most of what this file does is print. That is deliberate: pointing the app at
 * a seeded vault takes TWO environment variables and setting only the first is
 * destructive — `IndexState::initialize` hands the KB root to a startup
 * reconcile job, which would converge the developer's real index against this
 * fixture and delete every row for the notes it could no longer see. The
 * variables are always-on rather than `#[cfg(debug_assertions)]` so the harness
 * exercises the path that ships, which means an environment variable really can
 * relocate someone's notes. This output is the last thing standing between a
 * copy-paste and that.
 */

import { resolve } from "node:path";

import { MARKER_FILE, SCENARIOS, SCENARIO_NAMES, seedVault } from "./lib/vault.mjs";

const USAGE = [
  "usage: pnpm seed:vault <dir> [scenario…]",
  "",
  "  <dir>        where to write the fixture vault; no scenarios means all of them",
  "  --list       print the catalogue and exit",
  "  --force      wipe <dir> even without a .kodabi-fixture-vault marker",
  "",
  "pnpm eats flags, so pass them after a separator: pnpm seed:vault -- --list",
].join("\n");

const argv = process.argv.slice(2);
const flags = new Set(argv.filter((argument) => argument.startsWith("--")));
const positionals = argv.filter((argument) => !argument.startsWith("--"));

try {
  await main();
} catch (error) {
  // The message alone, never a stack: every error reachable here is a usage
  // mistake or a locked file, and a stack trace buries the sentence that says
  // which.
  console.error(error.message);
  process.exitCode = 1;
}

async function main() {
  if (flags.has("--help")) {
    console.log(USAGE);
    return;
  }

  if (flags.has("--list")) {
    for (const name of SCENARIO_NAMES) {
      console.log(`${name}\n    ${SCENARIOS[name].why}\n`);
    }
    return;
  }

  if (positionals.length === 0) {
    throw new Error(USAGE);
  }

  // Absolute, so the environment lines printed below are pasteable verbatim
  // from any directory.
  const root = resolve(positionals[0]);
  const names = positionals.length > 1 ? positionals.slice(1) : SCENARIO_NAMES;

  const manifest = await seedVault(root, names, { force: flags.has("--force") }).catch((error) => {
    // A running app holds `sessions/*.wav` open during playback, and Windows
    // will not unlink an open file. The raw errno reads like a permissions
    // problem, which sends people to the wrong fix.
    if (error.code === "EPERM" || error.code === "EBUSY") {
      throw new Error(`${root} is in use — quit Kodabi before re-seeding it.`);
    }
    throw error;
  });

  report(manifest);
}

/** What landed, then how to point the app at it. */
function report(manifest) {
  console.log(
    `Seeded ${manifest.scenarios.length} scenario${manifest.scenarios.length === 1 ? "" : "s"} into ${manifest.root}\n`,
  );

  const width = Math.max(...manifest.scenarios.map((name) => name.length));
  for (const name of manifest.scenarios) {
    const notes = manifest.notes.filter((note) => note.scenario === name);
    const sessions = manifest.sessions.filter((session) => session.scenario === name);
    const files = sessions.reduce((total, session) => total + session.files.length, 0);
    console.log(
      `  ${name.padEnd(width)}  ${plural(notes.length, "note")}, ${plural(files, "session file")}`,
    );
  }

  console.log(
    [
      "",
      "Point the app at it. Set BOTH lines, in the same shell you launch from:",
      "",
      `  $env:KODABI_KB_ROOT="${manifest.root}"`,
      `  $env:KODABI_INDEX_DB="${manifest.indexDb}"`,
      "  pnpm tauri dev",
      "",
      "Setting only the first is DESTRUCTIVE: the startup reconcile job would",
      "converge your real index against this fixture and drop every row for the",
      "notes it can no longer see.",
      "",
      "When you are done, or the next preview keeps using the fixture:",
      "",
      "  Remove-Item Env:KODABI_KB_ROOT, Env:KODABI_INDEX_DB",
      "",
      `Re-seeding this directory is one command — the ${MARKER_FILE} marker makes`,
      "it wipeable. Any other non-empty directory is refused.",
    ].join("\n"),
  );
}

function plural(count, noun) {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}
