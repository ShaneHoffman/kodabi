/**
 * `pnpm dev:sandbox` — runs the app against a throwaway vault instead of the
 * developer's real one.
 *
 * This is the launch every agent-driven session uses (the `/preview` skill, a
 * Kangentic task session, a screenshot run); see `.claude/rules/dev-sandbox.md`.
 * Plain `pnpm tauri dev` is deliberately left alone as the real-data workflow.
 *
 * All it does is set `KODABI_SANDBOX` to a worktree-local `.sandbox/`. The
 * derivation of everything else — vault root, index, config dir, WebView2
 * profile — happens in Rust (`kodabi_core::sandbox`), so this script cannot get
 * the destructive `KODABI_KB_ROOT`/`KODABI_INDEX_DB` pairing wrong, and neither
 * can anything else that sets the one variable.
 *
 * Worktree-local rather than a shared `%APPDATA%` sibling because Kangentic runs
 * several task worktrees at once: a shared sandbox would have two sessions
 * fighting over one settings file and one index, and the state would outlive the
 * branch that created it. `.sandbox/` is gitignored and dies with the worktree.
 *
 * The fixture world is seeded on first run so a preview opens onto believable
 * data (Briarwood Golf and friends, from the catalogue in `e2e/lib/vault.mjs`)
 * rather than an empty Inbox. Later runs reuse whatever is there, so state you
 * built up by hand survives.
 */

import { spawn } from "node:child_process";
import { readdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";

import { seedVault } from "../e2e/lib/vault.mjs";

const SANDBOX_DIR = fileURLToPath(new URL("../.sandbox", import.meta.url));

if (await isEmpty(SANDBOX_DIR)) {
  console.log(`Seeding a fresh sandbox vault at ${SANDBOX_DIR}`);
  const manifest = await seedVault(SANDBOX_DIR);
  console.log(
    `  ${manifest.scenarios.length} scenarios, ${manifest.notes.length} notes, ` +
      `${manifest.sessions.length} sessions\n`,
  );
} else {
  console.log(
    [
      `Reusing the sandbox vault at ${SANDBOX_DIR}`,
      "  Re-seed it with: pnpm seed:vault .sandbox [scenario...]  (quit the app first)",
      "",
    ].join("\n"),
  );
}

// One command string rather than an args array: `pnpm` is a `.cmd` shim on
// Windows that `spawn` refuses to exec directly, so this needs `shell: true` —
// and passing a separate args array alongside it is deprecated (DEP0190), since
// the shell concatenates rather than escapes them. The arguments here are
// constants with no interpolation, so there is nothing to escape.
const proc = spawn("pnpm tauri dev", {
  env: sandboxEnv(),
  stdio: "inherit",
  shell: true,
});

proc.on("exit", (code) => {
  process.exitCode = code ?? 1;
});

/** True when the directory is absent or holds nothing. */
async function isEmpty(dir) {
  const entries = await readdir(dir).catch((error) => {
    if (error.code === "ENOENT") {
      return [];
    }
    throw error;
  });
  return entries.length === 0;
}

/**
 * The child's environment: the switch, and none of the seams it drives.
 *
 * The two vault variables are stripped rather than left alone because a shell
 * that still carries them from an older manual preview would otherwise trip the
 * startup refusal — correct behaviour, but a confusing failure when the user
 * asked for nothing more than a sandbox. `KODABI_DISABLE_CHAT_DISTILL` is left
 * exactly as the caller set it, since Rust only defaults it (an explicit empty
 * value is how a sandboxed run turns chat distill back on).
 */
function sandboxEnv() {
  const env = { ...process.env, KODABI_SANDBOX: SANDBOX_DIR };
  delete env.KODABI_KB_ROOT;
  delete env.KODABI_INDEX_DB;
  return env;
}
