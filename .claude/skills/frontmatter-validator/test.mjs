#!/usr/bin/env node
// Self-test for the frontmatter validator — the repeatable "Done when" acceptance check.
// Asserts the three FRONTMATTER_SCHEMA examples pass, each injected-violation fixture is
// flagged with its expected error, and the validator still mirrors the source docs.
// Exits non-zero if any assertion fails.

import { readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { validateContent, checkSchema } from './validate.mjs';

const __dirname = dirname(fileURLToPath(import.meta.url));
const fx = (p) => readFileSync(join(__dirname, 'fixtures', p), 'utf8');
const errors = (content) => validateContent(content).filter((f) => f.level === 'ERROR');
const warns = (content) => validateContent(content).filter((f) => f.level === 'WARN');
const hasError = (content, code) => errors(content).some((f) => f.code === code);
const hasWarn = (content, code) => warns(content).some((f) => f.code === code);

let pass = 0;
let fail = 0;
function check(name, cond, detail) {
  if (cond) {
    pass++;
    console.log(`  ok    ${name}`);
  } else {
    fail++;
    console.log(`  FAIL  ${name}`);
    if (detail) console.log(`        ${detail}`);
  }
}

console.log('valid fixtures (expect zero errors):');
for (const f of [
  'valid/meeting.md',
  'valid/note.md',
  'valid/chat.md',
  'valid/inline-comments.md',
  'valid/classified-meeting.md',
]) {
  const errs = errors(fx(f));
  check(`${f} passes`, errs.length === 0, errs.map((e) => `${e.code}: ${e.message}`).join('; '));
}

console.log('invalid fixtures (expect the injected violation):');
check(
  'invalid/missing-id.md flags missing id',
  errors(fx('invalid/missing-id.md')).some((e) => e.code === 'missing-required' && e.field === 'id'),
);
check('invalid/bad-type.md flags bad type', hasError(fx('invalid/bad-type.md'), 'bad-type'));
check('invalid/empty-tags.md flags empty tags', hasError(fx('invalid/empty-tags.md'), 'empty-tags'));
check(
  'invalid/wrong-key-order.md flags key order',
  hasError(fx('invalid/wrong-key-order.md'), 'key-order'),
);
check(
  'invalid/inbox-missing-confidence.md flags inbox-confidence',
  hasError(fx('invalid/inbox-missing-confidence.md'), 'inbox-confidence'),
);
check(
  'invalid/bad-category.md flags bad category',
  hasError(fx('invalid/bad-category.md'), 'bad-category'),
);
check(
  'invalid/category-on-note.md flags a category on a non-meeting',
  hasError(fx('invalid/category-on-note.md'), 'category-on-non-meeting'),
);
check(
  'invalid/bad-tracking.md flags the ledger spelling',
  hasError(fx('invalid/bad-tracking.md'), 'bad-tracking'),
);

console.log('warning fixtures (expect a non-fatal WARN, zero errors):');
{
  const c = fx('valid/manual-with-confidence.md');
  const errs = errors(c);
  check(
    'valid/manual-with-confidence.md has no errors',
    errs.length === 0,
    errs.map((e) => `${e.code}: ${e.message}`).join('; '),
  );
  check(
    'valid/manual-with-confidence.md warns confidence-presence',
    hasWarn(c, 'confidence-presence'),
  );
}

console.log('schema mirror check (expect agreement):');
const { drift } = checkSchema();
check('FRONTMATTER_SCHEMA <-> NoteSummary agree', drift.length === 0, drift.join('; '));

console.log('');
console.log(`${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
