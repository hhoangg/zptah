// Checks of the landing build output and sources: CSP-safe (no inline code, no HTML string sinks), self-hosted,
// correct links per target, noindex on beta. Run with `pnpm test:scripts` (node --test).
import assert from 'node:assert/strict';
import { mkdtempSync, readdirSync, readFileSync, rmSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { after, before, describe, it } from 'node:test';
import { fileURLToPath } from 'node:url';
import { build } from '../build.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const src = path.join(here, '..', 'src');
const dirs = [];
const out = {};

before(async () => {
  for (const target of ['prod', 'beta']) {
    const dir = mkdtempSync(path.join(tmpdir(), `landing-${target}-`));
    dirs.push(dir);
    await build({ target, out: dir });
    out[target] = dir;
  }
});
after(() => dirs.forEach((d) => rmSync(d, { recursive: true, force: true })));

const page = (target) => readFileSync(path.join(out[target], 'landing', 'index.html'), 'utf8');
function walk(dir) {
  return readdirSync(dir, { recursive: true, withFileTypes: true })
    .filter((e) => e.isFile())
    .map((e) => path.join(e.parentPath, e.name));
}

describe('page under the production CSP', () => {
  for (const target of ['prod', 'beta']) {
    it(`${target}: no inline script or style element, no inline event handler`, () => {
      const html = page(target);
      for (const m of html.matchAll(/<script\b([^>]*)>/g)) {
        assert.ok(/\bsrc="/.test(m[1]) || /type="application\/json"/.test(m[1]), `inline script: ${m[0]}`);
      }
      assert.doesNotMatch(html, /<style[\s>]/);
      assert.doesNotMatch(html, /\son[a-z]+="/i);
    });

    it(`${target}: loads nothing from another origin`, () => {
      const html = page(target);
      for (const m of html.matchAll(/\s(?:src|href)="(https?:)?\/\/[^"]+"/g)) {
        // Links the reader can follow are fine; resources are not.
        const tag = html.slice(html.lastIndexOf('<', m.index), m.index + m[0].length);
        assert.ok(/^<a\s/.test(tag) || /rel="canonical"/.test(tag), `external resource: ${tag}`);
      }
      assert.doesNotMatch(page(target) + readFileSync(path.join(out[target], 'landing', 'landing.css'), 'utf8'), /url\(\s*["']?https?:|@import|fonts\.(googleapis|gstatic)/);
    });
  }

  it('the scripts never assign a string to an HTML sink (Trusted Types)', () => {
    for (const file of walk(path.join(src, 'js'))) {
      const text = readFileSync(file, 'utf8').replace(/^\s*\/\/.*$/gm, '');
      assert.doesNotMatch(text, /\.(innerHTML|outerHTML)\s*=|insertAdjacentHTML|document\.write|\beval\(|new Function|createContextualFragment|DOMParser/, file);
    }
  });
});

describe('targets', () => {
  it('prod: relative app links, canonical, no noindex', () => {
    const html = page('prod');
    assert.match(html, /href="\/login"/);
    assert.match(html, /href="\/register"/);
    assert.doesNotMatch(html, /https:\/\/zptah\.app\/(login|register)/);
    assert.match(html, /<link rel="canonical" href="https:\/\/zptah\.app\/">/);
    assert.doesNotMatch(html, /noindex/);
    assert.match(html, /<meta name="zptah-page" content="landing">/);
    assert.ok(!existsSync(path.join(out.prod, 'index.html')), 'prod writes landing/, robots.txt and sitemap.xml only');
    assert.match(readFileSync(path.join(out.prod, 'sitemap.xml'), 'utf8'), /<loc>https:\/\/zptah\.app\/<\/loc>/);
    const robots = readFileSync(path.join(out.prod, 'robots.txt'), 'utf8');
    assert.match(robots, /^Disallow: \/app\$$/m);
    assert.match(robots, /^Disallow: \/pair$/m);
    assert.doesNotMatch(robots, /^Disallow: \/app$/m, 'a prefix rule would block /apple-touch-icon.png');
    assert.doesNotMatch(robots, /__SITE__/);
  });

  it('beta: absolute app links, noindex meta, site-root files, noindex header, production CSP', () => {
    const html = page('beta');
    assert.match(html, /href="https:\/\/zptah\.app\/login"/);
    assert.match(html, /href="https:\/\/zptah\.app\/register"/);
    assert.doesNotMatch(html, /href="\/(login|register)"/);
    assert.match(html, /<meta name="robots" content="noindex, nofollow">/);
    assert.doesNotMatch(html, /rel="canonical"/);
    assert.equal(readFileSync(path.join(out.beta, 'index.html'), 'utf8'), html);
    assert.equal(readFileSync(path.join(out.beta, 'robots.txt'), 'utf8'), 'User-agent: *\nDisallow: /\n');
    const headers = readFileSync(path.join(out.beta, '_headers'), 'utf8');
    assert.match(headers, /X-Robots-Tag: noindex, nofollow/);
    assert.match(headers, /require-trusted-types-for 'script'/);
    assert.match(headers, /script-src 'self'; style-src 'self'/);
    for (const f of ['favicon.ico', 'favicon.svg', 'apple-touch-icon.png']) assert.ok(existsSync(path.join(out.beta, f)), f);
  });

  it('has OG and Twitter meta with an image that exists', () => {
    const html = page('prod');
    for (const name of ['og:title', 'og:description', 'og:image', 'og:url', 'twitter:card', 'twitter:image']) assert.ok(html.includes(`"${name}"`), name);
    assert.ok(existsSync(path.join(out.prod, 'landing', 'og.png')));
  });

  it('uses the Hugeicons the web app uses for the phone', () => {
    assert.match(page('prod'), /<symbol id="i-hg-sidebar-right"/);
    assert.match(page('prod'), /<symbol id="i-hg-mic"/);
  });
});

// En dash and em dash, built from code points so this file does not contain them.
const DASHES = new RegExp('[' + String.fromCharCode(0x2013, 0x2014) + ']');

describe('sources', () => {
  it('contain no em or en dash', () => {
    const root = path.join(here, '..');
    for (const file of walk(root).filter((f) => !f.includes(`${path.sep}dist`) && /\.(mjs|js|css|jsonc|svg)$/.test(f))) {
      assert.doesNotMatch(readFileSync(file, 'utf8'), DASHES, file);
    }
  });
});
