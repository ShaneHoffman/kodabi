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
const hasError = (content, code) => errors(content).some((f) => f.code === code);

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
for (const f of ['valid/meeting.md', 'valid/note.md', 'valid/chat.md']) {
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

console.log('schema mirror check (expect agreement):');
const { drift } = checkSchema();
check('FRONTMATTER_SCHEMA <-> NoteSummary agree', drift.length === 0, drift.join('; '));

console.log('');
console.log(`${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
