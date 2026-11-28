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

describe('remote control, reveal and hero layout', () => {
  const css = () => readFileSync(path.join(out.prod, 'landing', 'landing.css'), 'utf8');
  it('has the remote-anywhere feature row and the hero line', () => {
    const html = page('prod');
    assert.match(html, /<article class="frow2[^"]*" id="remote">/);
    assert.match(html, /same LAN or Wi-Fi/);
    assert.match(html, /no VPN or port forwarding/);
    assert.match(html, /passkey/);
    assert.match(html, /end-to-end encrypted/);
    assert.match(html, /class="mock any"/);
    assert.match(html, /class="sub remote-line">[^<]*over the internet/);
    assert.ok(html.indexOf('remote-line') < html.indexOf('class="cta"'));
  });
  it('marks reveal targets and ships the reveal script', () => {
    const html = page('prod');
    assert.match(html, /<h2 class="reveal">/);
    assert.ok((html.match(/class="frow2 reveal-grp/g) ?? []).length >= 6);
    assert.ok(existsSync(path.join(out.prod, 'landing', 'js', 'reveal.js')));
    assert.match(readFileSync(path.join(out.prod, 'landing', 'js', 'main.js'), 'utf8'), /initReveal\(\)/);
    assert.match(readFileSync(path.join(out.prod, 'landing', 'js', 'boot.js'), 'utf8'), /reveal-on/);
    const boot = readFileSync(path.join(out.prod, 'landing', 'js', 'boot.js'), 'utf8');
    assert.match(boot, /classList\.add\('js'\)/);
    assert.match(boot, /location\.replace\('\/app'\)/);
  });
  it('hides content only under .reveal-on and has a reduced-motion rule', () => {
    const c = css();
    assert.match(c, /\.reveal-on \.reveal[^{]*\{opacity:0/);
    assert.doesNotMatch(c.replace(/\.reveal-on[^{]*\{[^}]*\}/g, ''), /\.reveal[^{]*\{[^}]*opacity:0/);
    assert.match(c, /@media\(prefers-reduced-motion:reduce\)\{\s*\.reveal-on \.reveal[^}]*opacity:1!important[^}]*transition:none!important/);
  });
  it('puts the brew box after the buttons in the hero', () => {
    const html = page('prod');
    const cta = html.slice(html.indexOf('<div class="cta">'), html.indexOf('<div class="wrap wide">'));
    assert.ok(cta.indexOf('class="cta-btns"') < cta.indexOf('class="brew'));
    assert.ok(cta.indexOf('Open web app') < cta.indexOf('class="brew'));
    assert.match(css(), /\.cta\{display:flex;flex-direction:column;align-items:center/);
  });
});

describe('privacy page (Ardia design, section 11)', () => {
  const privacy = (target) => readFileSync(path.join(out[target], 'landing', 'privacy', 'index.html'), 'utf8');

  it('is built for both targets, CSP-safe and self-contained', () => {
    for (const target of ['prod', 'beta']) {
      const html = privacy(target);
      assert.doesNotMatch(html, /<script|<style[\s>]|\son[a-z]+="/i);
      assert.match(html, /<meta name="zptah-page" content="privacy">/);
      assert.match(html, /href="\/landing\/landing\.css"/);
      for (const m of html.matchAll(/\s(?:src|href)="(https?:)?\/\/[^"]+"/g)) {
        const tag = html.slice(html.lastIndexOf('<', m.index), m.index + m[0].length);
        assert.ok(/^<a\s/.test(tag) || /rel="canonical"/.test(tag), `external resource: ${tag}`);
      }
    }
  });

  it('discloses what the Ardia design requires', () => {
    const html = privacy('prod');
    for (const re of [
      /run[s]? the <code>ardia<\/code> command/,
      /content of the pages a Mono reads/,
      /titles and addresses of its tabs/,
      /No zptah server is involved in browsing/,
      /AI provider you configured/,
      /No analytics, no telemetry and no crash reports/,
      /No sale and no transfer/,
      /No advertising/,
      /creditworthiness or for lending/,
      /audit log/,
      /Control is Ardia's own model/,
      /any local program on your computer can drive Ardia/,
      /private folder on your computer/,
      /zptah's own calls to the local <code>ardia<\/code> command/,
      /saving the steps of a session as a flow/,
      /does not ask you first/,
      /browser output folder on your computer/,
      /conversation transcript on your computer/,
      /neither is airtight/,
      /caught after it happens/,
      /Contact and change log/,
    ]) assert.match(html, re);
    assert.doesNotMatch(html, /cannot reach Ardia except/);
    for (const re of [/native messaging/i, /extension storage/i, /debugger/i, /Chrome Web Store/i, /Limited Use/]) {
      assert.doesNotMatch(html, re);
    }
  });

  it('prod: canonical, in the sitemap, linked from the footer; beta: noindex and absolute link', () => {
    assert.match(privacy('prod'), /<link rel="canonical" href="https:\/\/zptah\.app\/privacy">/);
    assert.doesNotMatch(privacy('prod'), /noindex/);
    assert.match(readFileSync(path.join(out.prod, 'sitemap.xml'), 'utf8'), /<loc>https:\/\/zptah\.app\/privacy<\/loc>/);
    assert.match(page('prod'), /<a href="\/privacy">Privacy<\/a>/);
    assert.match(page('beta'), /<a href="https:\/\/zptah\.app\/privacy">Privacy<\/a>/);
    assert.match(privacy('beta'), /<meta name="robots" content="noindex, nofollow">/);
    assert.equal(readFileSync(path.join(out.beta, 'privacy', 'index.html'), 'utf8'), privacy('beta'));
    assert.ok(!existsSync(path.join(out.prod, 'privacy')), 'prod keeps everything under landing/');
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
