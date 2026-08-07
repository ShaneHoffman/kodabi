#!/usr/bin/env node
// Self-test for the release version tool — the repeatable "Done when" check.
// Asserts the preflight logic rejects every mistake that release.yml would only
// catch after a tag was already pushed, and that the bump preserves formatting.
// Exits non-zero if any assertion fails.

import { checkState, replaceVersion, readVersion, isValidVersion, normalizeVersion } from './version.mjs';

let pass = 0;
let fail = 0;
function check(name, cond, detail) {
  if (cond) {
    pass++;
    console.log(`  ok    ${name}`);
  } else {
    fail++;
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ''}`);
  }
}

const codes = (findings) => findings.filter((f) => f.level === 'ERROR').map((f) => f.code);
const both = (v) => ({ 'package.json': v, 'src-tauri/tauri.conf.json': v });

// ---- version shape --------------------------------------------------------
check('accepts MAJOR.MINOR.PATCH', isValidVersion('0.1.0'));
check('rejects a v prefix', !isValidVersion('v0.1.0'));
check('rejects a prerelease suffix', !isValidVersion('1.0.0-beta.1'));
check('rejects two-part versions', !isValidVersion('1.0'));
check('normalizeVersion strips one leading v', normalizeVersion('v1.2.3') === '1.2.3');

// ---- agreement between the two files --------------------------------------
check('matching fields are READY', codes(checkState({ versions: both('0.1.0') })).length === 0);
check(
  'disagreeing fields are an error',
  codes(checkState({ versions: { 'package.json': '0.1.0', 'src-tauri/tauri.conf.json': '0.2.0' } })).includes(
    'VERSION_MISMATCH',
  ),
);
check(
  'a missing field is an error',
  codes(checkState({ versions: { 'package.json': null, 'src-tauri/tauri.conf.json': '0.1.0' } })).includes(
    'VERSION_MISSING',
  ),
);
check(
  'a malformed field is an error',
  codes(checkState({ versions: both('0.1') })).includes('BAD_VERSION'),
);

// ---- the requested target --------------------------------------------------
check('target equal to both fields is READY', codes(checkState({ versions: both('0.2.0'), target: '0.2.0' })).length === 0);
check(
  'target ahead of the tree is an error',
  codes(checkState({ versions: both('0.1.0'), target: '0.2.0' })).includes('TARGET_MISMATCH'),
);
check(
  'an already-published tag is an error',
  codes(checkState({ versions: both('0.1.0'), target: '0.1.0', existingTags: ['v0.1.0'] })).includes('TAG_EXISTS'),
);
check(
  'an unrelated existing tag is fine',
  codes(checkState({ versions: both('0.2.0'), target: '0.2.0', existingTags: ['v0.1.0', 'models-v1'] })).length === 0,
);

// ---- the bump preserves everything but the version -------------------------
const sample = '{\n  "productName": "Kodabi",\n  "version": "0.1.0",\n  "identifier": "com.kodabi.app"\n}\n';
const bumped = replaceVersion(sample, '0.2.0');
check('bump rewrites the version', readVersion(bumped) === '0.2.0');
check('bump preserves surrounding lines', bumped === sample.replace('0.1.0', '0.2.0'));
check('bump leaves other fields untouched', bumped.includes('"productName": "Kodabi"'));

// A nested `"version"` on its own line is genuinely ambiguous — refuse rather than
// pick one. The same key inline inside another object never matches at all, because
// the pattern anchors to the start of a line.
let threw = false;
try {
  replaceVersion('{\n  "version": "1.0.0",\n  "nested": {\n    "version": "2.0.0"\n  }\n}\n', '3.0.0');
} catch {
  threw = true;
}
check('a second version line throws rather than guessing', threw);

const inline = '{\n  "version": "1.0.0",\n  "nested": { "version": "2.0.0" }\n}\n';
check('an inline nested version is ignored', readVersion(inline) === '1.0.0');
check('bumping ignores the inline nested version', replaceVersion(inline, '3.0.0').includes('{ "version": "2.0.0" }'));

console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail === 0 ? 0 : 1);
