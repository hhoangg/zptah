import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { render } from './gen-risk-lexicon.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const spec = readFileSync(join(here, '../../../../../../../docs/superpowers/specs/2026-11-25-zptah-browser-extension.md'), 'utf8');
const manual = JSON.parse(readFileSync(join(here, 'risk-lexicon.manual.json'), 'utf8'));

test('committed lexicon and cases equal a fresh generation from the spec', () => {
  for (const [file, text] of Object.entries(render(spec, manual))) {
    assert.equal(readFileSync(join(here, file), 'utf8'), text, `${file} is stale: run gen-risk-lexicon.mjs`);
  }
});
