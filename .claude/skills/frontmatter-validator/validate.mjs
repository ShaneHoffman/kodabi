#!/usr/bin/env node
// Frontmatter validator for Kodabi notes.
//
// Enforces docs/FRONTMATTER_SCHEMA.md against a single note or a folder of notes,
// and (with --check-schema) verifies that schema still mirrors the MCP NoteSummary
// shape in docs/MCP_TOOL_SURFACE.md. Zero dependencies — runs on plain `node`.
//
// The frontmatter is parsed by hand rather than with a YAML library on purpose: the
// canonical-key-order and empty-`tags`-key checks depend on the *literal* top-level
// key sequence and the literal presence of an empty key, both of which a normalizing
// YAML parser would erase.

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative, resolve, dirname } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));

// ---- Schema the validator encodes (source of truth: docs/FRONTMATTER_SCHEMA.md) ----
const CANONICAL = [
  'id',
  'type',
  'category',
  'tracking',
  'title',
  'project',
  'date',
  'tags',
  'source',
  'confidence',
  'category_confidence',
];
const REQUIRED = ['id', 'type', 'project', 'date', 'source'];
const TYPE_ENUM = ['meeting', 'note', 'chat'];
const CATEGORY_ENUM = [
  'standup',
  'one-on-one',
  'client',
  'working-session',
  'review',
  'all-hands',
  'observer',
];
const TRACKING_ENUM = ['tracked', 'context-only'];
const ID_RE = /^n_[0-9a-z]{6,}$/;
const TAG_RE = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
const DATE_ONLY_RE = /^(\d{4})-(\d{2})-(\d{2})$/;
const DATETIME_RE =
  /^(\d{4})-(\d{2})-(\d{2})[Tt](\d{2}):(\d{2}):(\d{2})(?:\.\d+)?([Zz]|[+-]\d{2}:\d{2})$/;

// ---------------------------------------------------------------------------
// Frontmatter parser
// ---------------------------------------------------------------------------

function stripQuotes(s) {
  const t = s.trim();
  if (
    t.length >= 2 &&
    ((t[0] === '"' && t.at(-1) === '"') || (t[0] === "'" && t.at(-1) === "'"))
  ) {
    return t.slice(1, -1);
  }
  return t;
}

// Drop a trailing YAML line comment: a `#` that starts a comment (at the start of
// the value or preceded by whitespace) and sits outside quotes. Quote tracking keeps
// a literal `#` inside a value (e.g. `raw/a#b.jsonl`, `"a # b"`) intact.
function stripComment(s) {
  let quote = null;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (quote) {
      if (c === quote) quote = null;
    } else if (c === '"' || c === "'") {
      quote = c;
    } else if (c === '#' && (i === 0 || /\s/.test(s[i - 1]))) {
      return s.slice(0, i);
    }
  }
  return s;
}

// Split on top-level commas, ignoring commas inside quoted items.
function splitTopLevel(s) {
  const out = [];
  let cur = '';
  let quote = null;
  for (const c of s) {
    if (quote) {
      cur += c;
      if (c === quote) quote = null;
    } else if (c === '"' || c === "'") {
      quote = c;
      cur += c;
    } else if (c === ',') {
      out.push(cur);
      cur = '';
    } else {
      cur += c;
    }
  }
  out.push(cur);
  return out;
}

function parseInlineList(v) {
  const inner = v.trim().replace(/^\[/, '').replace(/\]$/, '');
  if (inner.trim() === '') return [];
  return splitTopLevel(inner)
    .map((x) => stripQuotes(x))
    .filter((x) => x !== '');
}

// Returns { hasFrontmatter, terminated, entries } where each entry is
// { key, line, kind: 'scalar'|'inlineList'|'blockList'|'empty', value, items }.
function parseFrontmatter(content) {
  const text = content.replace(/^﻿/, '');
  const lines = text.split(/\r?\n/);
  if (lines.length === 0 || lines[0].trim() !== '---') {
    return { hasFrontmatter: false, terminated: false, entries: [] };
  }
  let close = -1;
  for (let i = 1; i < lines.length; i++) {
    if (lines[i].trim() === '---') {
      close = i;
      break;
    }
  }
  if (close === -1) {
    return { hasFrontmatter: true, terminated: false, entries: [] };
  }

  const entries = [];
  let i = 1;
  while (i < close) {
    const raw = lines[i];
    const lineNo = i + 1; // 1-based file line number
    if (/^\s*$/.test(raw) || /^\s*#/.test(raw)) {
      i++;
      continue;
    }
    // Top-level key: no leading whitespace, `key: ...`.
    const m = raw.match(/^([^\s:][^:]*):(.*)$/);
    if (!m) {
      i++; // stray/indented line we don't model — ignore
      continue;
    }
    const key = m[1].trim();
    const valuePart = stripComment(m[2]).trim();
    let kind;
    let items = null;
    let value = null;

    if (valuePart === '') {
      // Empty inline value: look ahead for block-list items.
      const collected = [];
      let j = i + 1;
      while (j < close) {
        const l = lines[j];
        if (/^\s*$/.test(l) || /^\s*#/.test(l)) {
          j++;
          continue;
        }
        const li = l.match(/^\s*-\s*(.*)$/);
        if (li) {
          collected.push(stripQuotes(stripComment(li[1])));
          j++;
          continue;
        }
        break;
      }
      if (collected.length > 0) {
        kind = 'blockList';
        items = collected;
        i = j;
      } else {
        kind = 'empty';
        i++;
      }
    } else if (valuePart.startsWith('[')) {
      kind = 'inlineList';
      items = parseInlineList(valuePart);
      i++;
    } else {
      kind = 'scalar';
      value = stripQuotes(valuePart);
      i++;
    }
    entries.push({ key, line: lineNo, kind, value, items });
  }
  return { hasFrontmatter: true, terminated: true, entries };
}

// ---------------------------------------------------------------------------
// Date / value helpers
// ---------------------------------------------------------------------------

function isLeap(y) {
  return (y % 4 === 0 && y % 100 !== 0) || y % 400 === 0;
}

function isValidCalendarDate(y, mo, d) {
  if (mo < 1 || mo > 12) return false;
  const days = [31, isLeap(y) ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  return d >= 1 && d <= days[mo - 1];
}

function isValidIsoDate(v) {
  let m = DATE_ONLY_RE.exec(v);
  if (m) return isValidCalendarDate(+m[1], +m[2], +m[3]);
  m = DATETIME_RE.exec(v);
  if (m) {
    if (!isValidCalendarDate(+m[1], +m[2], +m[3])) return false;
    if (+m[4] > 23 || +m[5] > 59 || +m[6] > 59) return false;
    const off = m[7];
    if (off && off.toUpperCase() !== 'Z') {
      const [oh, om] = off.slice(1).split(':').map(Number);
      if (oh > 23 || om > 59) return false;
    }
    return true;
  }
  return false;
}

function scalarValue(e) {
  return e && e.kind === 'scalar' ? e.value : null;
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

// Returns an array of findings: { level: 'ERROR'|'WARN', code, field, message, line }.
function validateContent(content) {
  const findings = [];
  const add = (level, code, field, message, line = null) =>
    findings.push({ level, code, field, message, line });

  const fm = parseFrontmatter(content);
  if (!fm.hasFrontmatter) {
    add('ERROR', 'no-frontmatter', '(file)', 'no YAML frontmatter block — a note must start with `---`', 1);
    return findings;
  }
  if (!fm.terminated) {
    add('ERROR', 'no-frontmatter', '(file)', 'unterminated frontmatter — missing the closing `---`', 1);
    return findings;
  }

  const { entries } = fm;
  const byKey = new Map();
  for (const e of entries) {
    if (byKey.has(e.key)) {
      add('ERROR', 'duplicate-key', e.key, `duplicate key '${e.key}'`, e.line);
    } else {
      byKey.set(e.key, e);
    }
  }

  // Required fields
  for (const r of REQUIRED) {
    if (!byKey.has(r)) add('ERROR', 'missing-required', r, 'missing required field');
  }

  // Unknown keys (note-level schema drift)
  for (const e of entries) {
    if (!CANONICAL.includes(e.key)) {
      add(
        'ERROR',
        'unknown-key',
        e.key,
        `unknown field — not in the frontmatter schema (${CANONICAL.join(', ')})`,
        e.line,
      );
    }
  }

  // Canonical key order: present known keys must be strictly increasing.
  let prevIdx = -1;
  for (const e of entries) {
    const idx = CANONICAL.indexOf(e.key);
    if (idx === -1) continue; // unknown key handled above
    if (idx < prevIdx) {
      add(
        'ERROR',
        'key-order',
        e.key,
        `out of canonical order (expected: ${CANONICAL.join(', ')})`,
        e.line,
      );
    }
    prevIdx = idx;
  }

  // id
  const idE = byKey.get('id');
  if (idE) {
    const v = scalarValue(idE);
    if (!v) add('ERROR', 'bad-id', 'id', 'id must be a non-empty string', idE.line);
    else if (!ID_RE.test(v))
      add('ERROR', 'bad-id', 'id', `id '${v}' does not match ${ID_RE.source}`, idE.line);
  }

  // type
  const typeE = byKey.get('type');
  if (typeE) {
    const v = scalarValue(typeE);
    if (!v || !TYPE_ENUM.includes(v))
      add('ERROR', 'bad-type', 'type', `type '${v ?? ''}' is not one of ${TYPE_ENUM.join(' | ')}`, typeE.line);
  }

  // category — a closed set, and a meetings-only facet.
  const categoryE = byKey.get('category');
  if (categoryE) {
    const v = scalarValue(categoryE);
    if (!v || !CATEGORY_ENUM.includes(v))
      add(
        'ERROR',
        'bad-category',
        'category',
        `category '${v ?? ''}' is not one of ${CATEGORY_ENUM.join(' | ')}`,
        categoryE.line,
      );
    const typeVal = typeE ? scalarValue(typeE) : null;
    if (typeVal && typeVal !== 'meeting')
      add(
        'ERROR',
        'category-on-non-meeting',
        'category',
        `category belongs to a meeting; this note is type '${typeVal}'`,
        categoryE.line,
      );
  }

  // tracking — the per-meeting commitment-tracking override.
  const trackingE = byKey.get('tracking');
  if (trackingE) {
    const v = scalarValue(trackingE);
    if (!v || !TRACKING_ENUM.includes(v))
      add(
        'ERROR',
        'bad-tracking',
        'tracking',
        `tracking '${v ?? ''}' is not one of ${TRACKING_ENUM.join(' | ')}`,
        trackingE.line,
      );
  }

  // project
  const projE = byKey.get('project');
  const projectVal = projE ? scalarValue(projE) : null;
  if (projE && (!projectVal || projectVal.trim() === '')) {
    add('ERROR', 'bad-project', 'project', 'project must be a non-empty string', projE.line);
  }

  // date
  const dateE = byKey.get('date');
  if (dateE) {
    const v = scalarValue(dateE);
    if (!v || !isValidIsoDate(v))
      add(
        'ERROR',
        'bad-date',
        'date',
        `date '${v ?? ''}' is not a valid ISO-8601 date (YYYY-MM-DD) or timestamp with offset`,
        dateE.line,
      );
  }

  // source
  const srcE = byKey.get('source');
  const sourceVal = srcE ? scalarValue(srcE) : null;
  if (srcE && (!sourceVal || sourceVal.trim() === '')) {
    add('ERROR', 'bad-source', 'source', 'source must be a non-empty string', srcE.line);
  }

  // tags
  const tagsE = byKey.get('tags');
  if (tagsE) {
    const isEmpty =
      tagsE.kind === 'empty' ||
      ((tagsE.kind === 'inlineList' || tagsE.kind === 'blockList') && tagsE.items.length === 0);
    if (isEmpty) {
      add(
        'ERROR',
        'empty-tags',
        'tags',
        'tags key is present but empty — omit the key entirely when there are no tags',
        tagsE.line,
      );
    } else if (tagsE.kind === 'scalar') {
      add('ERROR', 'bad-tags', 'tags', 'tags must be a YAML list (e.g. [a, b]), not a scalar', tagsE.line);
    } else {
      for (const t of tagsE.items) {
        if (t.startsWith('#'))
          add('ERROR', 'bad-tag', 'tags', `tag '${t}' must not start with '#'`, tagsE.line);
        else if (!TAG_RE.test(t))
          add('ERROR', 'bad-tag', 'tags', `tag '${t}' is not lowercase kebab-case`, tagsE.line);
      }
    }
  }

  // confidence value
  const confE = byKey.get('confidence');
  const confPresent = !!confE;
  if (confE) {
    const v = scalarValue(confE);
    const num = v === null ? NaN : Number(v);
    if (v === null || v === '' || Number.isNaN(num))
      add('ERROR', 'confidence-range', 'confidence', `confidence '${v ?? ''}' is not a number`, confE.line);
    else if (num < 0 || num > 1)
      add('ERROR', 'confidence-range', 'confidence', `confidence ${num} is outside 0.0–1.0`, confE.line);
  }

  // confidence presence rules
  if (projectVal === 'Inbox' && !confPresent) {
    add(
      'ERROR',
      'inbox-confidence',
      'confidence',
      'project is Inbox but confidence is missing — an Inbox note carries the routing score that put it there',
      projE ? projE.line : null,
    );
  }
  if (confPresent && (sourceVal === 'import' || sourceVal === 'manual')) {
    add(
      'WARN',
      'confidence-presence',
      'confidence',
      `source '${sourceVal}' implies a hand-filed/imported note, which usually omits confidence`,
      confE.line,
    );
  }

  return findings;
}

// ---------------------------------------------------------------------------
// --check-schema: does the validator still mirror the source docs?
// ---------------------------------------------------------------------------

function arraysEqual(a, b) {
  return a.length === b.length && a.every((x, i) => x === b[i]);
}

function extractDefs(doc) {
  const re = /```json\s*([\s\S]*?)```/g;
  let m;
  while ((m = re.exec(doc))) {
    if (!m[1].includes('"NoteSummary"')) continue;
    try {
      const parsed = JSON.parse(m[1]);
      if (parsed.$defs && parsed.$defs.NoteSummary) return parsed.$defs;
    } catch {
      /* not the block we want — try the next one */
    }
  }
  return null;
}

function checkSchema() {
  const lines = [];
  const drift = [];
  const ok = (msg) => lines.push(`  OK     ${msg}`);
  const bad = (msg) => {
    lines.push(`  DRIFT  ${msg}`);
    drift.push(msg);
  };

  const repoRoot = resolve(__dirname, '..', '..', '..');
  const fsPath = join(repoRoot, 'docs', 'FRONTMATTER_SCHEMA.md');
  const mcpPath = join(repoRoot, 'docs', 'MCP_TOOL_SURFACE.md');

  lines.push('Schema mirror check — validator rules vs the source docs');
  lines.push('');
  lines.push(`FRONTMATTER_SCHEMA.md`);
  try {
    const fsDoc = readFileSync(fsPath, 'utf8');
    const orderLine = fsDoc.split(/\r?\n/).find((l) => /Canonical key order/i.test(l));
    // Pick the backtick-delimited span holding the field list (the one with commas),
    // not merely the first backtick span — so rewording the prose around it can't
    // capture an unrelated `word` and report spurious drift.
    const listSpan = orderLine
      ? [...orderLine.matchAll(/`([^`]+)`/g)].map((mm) => mm[1]).find((s) => s.includes(','))
      : null;
    if (!orderLine) bad('could not find the canonical key-order line');
    else if (!listSpan) bad('canonical key-order line has no backtick-quoted field list');
    else {
      const order = listSpan.split(',').map((s) => s.trim());
      if (arraysEqual(order, CANONICAL)) ok(`canonical key order: ${order.join(', ')}`);
      else
        bad(
          `canonical key order drift: doc [${order.join(', ')}] vs validator [${CANONICAL.join(', ')}]`,
        );
    }
    if (fsDoc.includes(ID_RE.source)) ok(`id pattern ${ID_RE.source}`);
    else bad(`id pattern ${ID_RE.source} not found in FRONTMATTER_SCHEMA.md`);
  } catch (e) {
    bad(`cannot read FRONTMATTER_SCHEMA.md: ${e.message}`);
  }

  lines.push('');
  lines.push('MCP_TOOL_SURFACE.md (NoteSummary)');
  try {
    const mcpDoc = readFileSync(mcpPath, 'utf8');
    const defs = extractDefs(mcpDoc);
    if (!defs) bad('could not parse the $defs JSON block containing NoteSummary');
    else {
      const ns = defs.NoteSummary;
      const props = Object.keys(ns.properties || {});
      const missing = CANONICAL.filter((f) => !props.includes(f));
      if (missing.length) bad(`NoteSummary is missing mirrored field(s): ${missing.join(', ')}`);
      else ok(`NoteSummary mirrors fields: ${CANONICAL.join(', ')}`);

      // `path` is the one NoteSummary field with no frontmatter counterpart
      // (the note's current location, not stored content). `title` is now a
      // real frontmatter field, so it is covered by CANONICAL above.
      const allowedExtras = new Set([...CANONICAL, 'path']);
      const extras = props.filter((p) => !allowedExtras.has(p));
      if (extras.length)
        bad(
          `NoteSummary has unmirrored field(s) that may need adding to the frontmatter schema: ${extras.join(', ')}`,
        );
      else ok('no unmirrored NoteSummary fields (besides path)');

      const idPat = defs.NoteId?.pattern;
      if (idPat === ID_RE.source) ok(`NoteId pattern ${idPat}`);
      else bad(`NoteId pattern drift: doc [${idPat}] vs validator [${ID_RE.source}]`);

      const en = defs.NoteType?.enum || [];
      if (arraysEqual([...en].sort(), [...TYPE_ENUM].sort())) ok(`NoteType enum {${en.join(', ')}}`);
      else bad(`NoteType enum drift: doc {${en.join(', ')}} vs validator {${TYPE_ENUM.join(', ')}}`);

      const cats = defs.MeetingCategory?.enum || [];
      if (arraysEqual([...cats].sort(), [...CATEGORY_ENUM].sort()))
        ok(`MeetingCategory enum {${cats.join(', ')}}`);
      else
        bad(
          `MeetingCategory enum drift: doc {${cats.join(', ')}} vs validator {${CATEGORY_ENUM.join(', ')}}`,
        );
    }
  } catch (e) {
    bad(`cannot read MCP_TOOL_SURFACE.md: ${e.message}`);
  }

  lines.push('');
  lines.push(drift.length ? `DRIFT: ${drift.length} issue(s) — the specs and validator disagree` : 'OK: docs and validator agree');
  return { drift, lines };
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

function gather(p) {
  // Resolve each entry defensively: an unreadable file or directory (broken symlink,
  // permission denied) is reported and skipped, never aborting the whole sweep.
  let st;
  try {
    st = statSync(p);
  } catch {
    console.error(`cannot read: ${relative(process.cwd(), p) || p}`);
    return [];
  }
  if (st.isDirectory()) {
    const out = [];
    let names;
    try {
      names = readdirSync(p);
    } catch {
      console.error(`cannot read directory: ${relative(process.cwd(), p) || p}`);
      return out;
    }
    for (const name of names) {
      if (name === 'node_modules' || name === '.git') continue;
      out.push(...gather(join(p, name)));
    }
    return out;
  }
  return p.toLowerCase().endsWith('.md') ? [p] : [];
}

function fmtFinding(x) {
  const loc = x.line ? ` (line ${x.line})` : '';
  return `  ${x.level.padEnd(5)}  ${String(x.field).padEnd(11)}  ${x.message}${loc}`;
}

function main() {
  const argv = process.argv.slice(2);
  if (argv.includes('--check-schema')) {
    const { drift, lines } = checkSchema();
    for (const l of lines) console.log(l);
    process.exit(drift.length ? 1 : 0);
  }

  const paths = argv.filter((a) => !a.startsWith('--'));
  if (paths.length === 0) {
    console.error('usage: node validate.mjs <file-or-folder> [more paths…]');
    console.error('       node validate.mjs --check-schema');
    process.exit(2);
  }

  let files = [];
  for (const p of paths) {
    try {
      files.push(...gather(resolve(p)));
    } catch {
      console.error(`cannot read: ${p}`);
    }
  }
  files = [...new Set(files)];
  if (files.length === 0) {
    console.error('no .md files found in the given path(s)');
    process.exit(2);
  }

  let passed = 0;
  let failed = 0;
  for (const f of files) {
    const findings = validateContent(readFileSync(f, 'utf8'));
    const errors = findings.filter((x) => x.level === 'ERROR');
    const rel = relative(process.cwd(), f) || f;
    if (errors.length === 0) {
      passed++;
      console.log(`PASS  ${rel}`);
      for (const w of findings.filter((x) => x.level === 'WARN')) console.log(fmtFinding(w));
    } else {
      failed++;
      console.log(`FAIL  ${rel}`);
      for (const x of findings) console.log(fmtFinding(x));
    }
  }
  console.log('');
  console.log(`${files.length} file(s) · ${passed} passed · ${failed} failed`);
  process.exit(failed ? 1 : 0);
}

const isMain = import.meta.url === pathToFileURL(process.argv[1] || '').href;
if (isMain) main();

export { validateContent, checkSchema, parseFrontmatter };
