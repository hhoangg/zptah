// Generates risk-lexicon.json and lexicon-cases.json from spec Appendix A plus
// risk-lexicon.manual.json (the hand-written entries, settings and cases).
//
//   node extensions/zptah-browser/shared/gen-risk-lexicon.mjs          write the files
//   node extensions/zptah-browser/shared/gen-risk-lexicon.mjs --check  fail when they are stale
//
// Edit the manual file or the spec appendix, never the generated JSON.
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const SPEC = join(here, '../../../docs/superpowers/specs/2026-11-25-zptah-browser-extension.md');

const strip = (s) =>
  s.replace(/đ/g, 'd').replace(/Đ/g, 'D').normalize('NFD').replace(/\p{M}/gu, '');

function parseAppendix(spec) {
  const app = spec.split('## Appendix A.')[1];
  const sections = {};
  let cur = null;
  for (const line of app.split('\n')) {
    const h = line.match(/^### (A\.\d+)/);
    if (h) {
      cur = h[1];
      sections[cur] = [];
      continue;
    }
    const m = line.match(/^\| (.+?) \| `(.+?)` \| (.*?) \|$/);
    if (m && cur && m[1] !== 'Phrase') sections[cur].push([m[1].trim(), m[3].trim()]);
  }
  return sections;
}

export function build(spec, manual) {
  const sections = parseAppendix(spec);
  const entries = [];
  const seen = new Set();
  const add = (phrase, cls, lang = 'vi', kw = {}) => {
    const key = `${phrase.toLowerCase()}\u0000${cls}`;
    if (seen.has(key)) return;
    seen.add(key);
    entries.push({ phrase, class: cls, lang, ...kw });
  };
  const SEND = new Set(['Gửi', 'Gửi ngay', 'Trả lời tất cả', 'Chuyển tiếp']);
  for (const [p] of sections['A.1']) add(p, 'payment', 'vi', p === 'Mua' ? { max_words: 3 } : {});
  for (const [p] of sections['A.11']) add(p, 'payment');
  for (const [p] of sections['A.2']) if (p !== 'Xoá') add(p, 'destructive');
  for (const [p] of sections['A.3']) add(p, 'account_security');
  for (const [p] of sections['A.4']) add(p, 'sharing');
  for (const [p] of sections['A.5'])
    add(p, SEND.has(p) ? 'send' : 'post', 'vi', p === 'Gửi' || p === 'Đăng' ? { max_words: 3 } : {});
  for (const [p] of sections['A.6']) add(p, 'terms');
  for (const [p] of sections['A.7']) add(p, 'consent');
  for (const [p, t] of sections['A.8']) {
    let kind = t.match(/`(\w+)`/)[1];
    if (p === 'Tên chủ thẻ') kind = 'card_number';
    add(p, `field_${kind}`);
  }
  for (const [p] of sections['A.9']) add(p, 'context');
  const DISMISS = new Set(['Hủy', 'Hủy bỏ', 'Đóng', 'Để sau', 'Bỏ qua', 'Không', 'Quay lại']);
  const PREFIX = new Set(['Đăng nhập', 'Đăng ký', 'Đăng xuất']);
  for (const [p] of sections['A.10']) {
    const kw = {};
    if (p === 'Hủy') kw.ctx = true;
    if (PREFIX.has(p)) kw.prefix = true;
    add(p, DISMISS.has(p) ? 'dismiss' : 'not_risky', 'vi', kw);
  }
  for (const e of manual.entries) {
    const { phrase, class: cls, lang, ...kw } = e;
    add(phrase, cls, lang, kw);
  }
  const lexicon = { ...manual.settings, entries };
  return { lexicon, cases: buildCases(entries, manual.cases) };
}

function buildCases(entries, handwritten) {
  const RISK = new Set(['payment', 'destructive', 'account_security', 'sharing', 'terms']);
  const AMBIG = new Set(['go', 'moi', 'dang', 'dong', 'co']);
  const byPhrase = new Map();
  for (const e of entries) {
    const key = e.phrase.normalize('NFC').toLowerCase();
    if (!byPhrase.has(key)) byPhrase.set(key, []);
    byPhrase.get(key).push(e);
  }
  const expectFor = (es) => {
    const ex = { categories: [] };
    for (const e of es) {
      const c = e.class;
      if (RISK.has(c)) ex.categories.push(c);
      else if (c === 'send') ex.send = true;
      else if (c === 'post') ex.post = true;
      else if (c === 'consent') ex.consent = true;
      else if (c === 'not_risky') ex.not_risky = true;
      else if (c === 'dismiss' && !e.ctx) ex.dismiss = true;
    }
    ex.categories = [...new Set(ex.categories)].sort();
    return ex;
  };
  const cases = [];
  for (const es of byPhrase.values()) {
    const classes = new Set(es.map((e) => e.class));
    if ([...classes].some((c) => c.startsWith('field_'))) continue;
    if (classes.has('context') || es.some((e) => e.ctx)) continue;
    // A sign-up entry turns into payment next to a price, so its plain case is covered by hand.
    const name = es[0].phrase;
    const ex = expectFor(es);
    cases.push({ name, expect: ex, note: 'display form' });
    if (name.toUpperCase() !== name) cases.push({ name: name.toUpperCase(), expect: ex, note: 'upper case' });
    const s = strip(name);
    if (s !== name && es[0].lang === 'vi') {
      const words = s.toLowerCase().split(/\s+/);
      if (words.length === 1 && AMBIG.has(words[0])) {
        cases.push({ name: s, page_vi: false, expect: { categories: [] }, note: 'ambiguous stripped word off a Vietnamese page' });
        cases.push({ name: s, page_vi: true, expect: ex, note: 'ambiguous stripped word on a Vietnamese page' });
      } else {
        cases.push({ name: s, expect: ex, note: 'without diacritics' });
      }
    }
  }
  return [...cases, ...handwritten];
}

const dump = (o) => `${JSON.stringify(o, null, 1)}\n`;

export function render(spec, manual) {
  const { lexicon, cases } = build(spec, manual);
  return {
    'risk-lexicon.json': dump(lexicon),
    'lexicon-cases.json': dump({
      version: 1,
      comment:
        'Name classifier cases shared by the Rust tests and the extension tests. Generated by gen-risk-lexicon.mjs from the lexicon plus the hand-written cases of risk-lexicon.manual.json.',
      cases,
    }),
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const spec = readFileSync(SPEC, 'utf8');
  const manual = JSON.parse(readFileSync(join(here, 'risk-lexicon.manual.json'), 'utf8'));
  const out = render(spec, manual);
  const check = process.argv.includes('--check');
  let stale = false;
  for (const [file, text] of Object.entries(out)) {
    const path = join(here, file);
    if (check) {
      if (readFileSync(path, 'utf8') !== text) {
        console.error(`${file} is stale: run node extensions/zptah-browser/shared/gen-risk-lexicon.mjs`);
        stale = true;
      }
    } else {
      writeFileSync(path, text);
    }
  }
  if (stale) process.exit(1);
}
