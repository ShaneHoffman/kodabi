#!/usr/bin/env node
// Release version tool for Kodabi.
//
// The tag a release ships under must equal `version` in BOTH package.json and
// src-tauri/tauri.conf.json — .github/workflows/release.yml asserts it before it
// compiles anything, and a mismatch is only discovered after a tag has already
// been pushed. This script is the pre-push half of that assertion, plus the
// bump itself. Zero dependencies — runs on plain `node`.
//
// The bump rewrites the single `"version"` line in place rather than
// round-tripping through JSON.parse/stringify: both files are hand-maintained
// and a reformat would bury a one-field change in a whole-file diff.
//
//   node version.mjs check [--version X.Y.Z]   report both fields (and the tag)
//   node version.mjs set X.Y.Z                 write both fields

import { readFileSync, writeFileSync } from 'node:fs';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

const __dirname = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(__dirname, '..', '..', '..');

export const VERSION_FILES = ['package.json', 'src-tauri/tauri.conf.json'];

// Plain MAJOR.MINOR.PATCH. Deliberately stricter than semver: tauri.conf.json's
// `version` feeds the Windows installer's version resource, which is numeric
// only, so a prerelease suffix would not survive the round trip.
const VERSION_RE = /^\d+\.\d+\.\d+$/;

// Exactly one top-level `"version": "…"` per file; asserted, never assumed.
const VERSION_LINE_RE = /^(\s*"version"\s*:\s*")([^"]*)(")/gm;

// ---------------------------------------------------------------------------
// Pure logic (unit-tested by test.mjs)
// ---------------------------------------------------------------------------

export function isValidVersion(value) {
  return typeof value === 'string' && VERSION_RE.test(value);
}

/** Strip a single leading `v`, so `v1.2.3` and `1.2.3` are both accepted input. */
export function normalizeVersion(value) {
  return typeof value === 'string' ? value.replace(/^v/, '') : value;
}

/**
 * Decide whether a release may proceed.
 * @param {{versions: Record<string,string>, target?: string, existingTags?: string[]}} state
 * @returns {{level: 'ERROR'|'OK', code: string, message: string}[]}
 */
export function checkState({ versions, target, existingTags = [] }) {
  const findings = [];
  const entries = Object.entries(versions);

  for (const [file, version] of entries) {
    if (version === null || version === undefined) {
      findings.push({ level: 'ERROR', code: 'VERSION_MISSING', message: `${file}: no "version" field found` });
    } else if (!isValidVersion(version)) {
      findings.push({
        level: 'ERROR',
        code: 'BAD_VERSION',
        message: `${file}: "${version}" is not MAJOR.MINOR.PATCH`,
      });
    }
  }

  const distinct = [...new Set(entries.map(([, v]) => v))];
  if (distinct.length > 1) {
    const detail = entries.map(([f, v]) => `${f}=${v}`).join(', ');
    findings.push({
      level: 'ERROR',
      code: 'VERSION_MISMATCH',
      message: `the version fields disagree (${detail}); release.yml rejects this after the tag is pushed`,
    });
  }

  if (target !== undefined) {
    if (!isValidVersion(target)) {
      findings.push({ level: 'ERROR', code: 'BAD_TARGET', message: `requested version "${target}" is not MAJOR.MINOR.PATCH` });
    } else {
      for (const [file, version] of entries) {
        if (version !== target) {
          findings.push({
            level: 'ERROR',
            code: 'TARGET_MISMATCH',
            message: `${file} says ${version}, but the release targets ${target}`,
          });
        }
      }
      if (existingTags.includes(`v${target}`)) {
        findings.push({
          level: 'ERROR',
          code: 'TAG_EXISTS',
          message: `tag v${target} already exists; a shipped tag is never moved — cut the next patch version instead`,
        });
      }
    }
  }

  if (findings.length === 0) {
    const shipping = target ?? distinct[0];
    findings.push({ level: 'OK', code: 'READY', message: `both version fields are ${shipping}` });
  }
  return findings;
}

/**
 * Replace the single `"version"` line in a JSON document, preserving all other
 * formatting. Throws when the file does not contain exactly one such line.
 */
export function replaceVersion(source, next, label = 'file') {
  const matches = [...source.matchAll(VERSION_LINE_RE)];
  if (matches.length !== 1) {
    throw new Error(`${label}: expected exactly one "version" line, found ${matches.length}`);
  }
  VERSION_LINE_RE.lastIndex = 0;
  return source.replace(VERSION_LINE_RE, (_m, before, _old, after) => `${before}${next}${after}`);
}

export function readVersion(source) {
  const matches = [...source.matchAll(VERSION_LINE_RE)];
  VERSION_LINE_RE.lastIndex = 0;
  return matches.length === 1 ? matches[0][2] : null;
}

// ---------------------------------------------------------------------------
// I/O
// ---------------------------------------------------------------------------

function readVersions(root = REPO_ROOT) {
  const versions = {};
  for (const file of VERSION_FILES) {
    versions[file] = readVersion(readFileSync(join(root, file), 'utf8'));
  }
  return versions;
}

function localTags(root = REPO_ROOT) {
  try {
    return execFileSync('git', ['-C', root, 'tag', '-l'], { encoding: 'utf8' }).split('\n').filter(Boolean);
  } catch {
    return [];
  }
}

function main(argv) {
  const [command, ...rest] = argv;
  if (command === 'check') {
    const i = rest.indexOf('--version');
    const target = i >= 0 ? normalizeVersion(rest[i + 1]) : undefined;
    const findings = checkState({ versions: readVersions(), target, existingTags: localTags() });
    for (const f of findings) console.log(`${f.level.padEnd(5)} ${f.code.padEnd(16)} ${f.message}`);
    return findings.some((f) => f.level === 'ERROR') ? 1 : 0;
  }

  if (command === 'set') {
    const next = normalizeVersion(rest[0]);
    if (!isValidVersion(next)) {
      console.log(`ERROR BAD_VERSION      "${rest[0]}" is not MAJOR.MINOR.PATCH`);
      return 1;
    }
    for (const file of VERSION_FILES) {
      const path = join(REPO_ROOT, file);
      const source = readFileSync(path, 'utf8');
      writeFileSync(path, replaceVersion(source, next, file));
      console.log(`set   ${file} -> ${next}`);
    }
    return 0;
  }

  console.log('usage: version.mjs check [--version X.Y.Z] | set X.Y.Z');
  return 1;
}

if (process.argv[1] && process.argv[1].endsWith('version.mjs')) {
  process.exit(main(process.argv.slice(2)));
}
