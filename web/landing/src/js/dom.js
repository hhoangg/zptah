// DOM helpers. The page is served under `require-trusted-types-for 'script'`, so nothing here (or anywhere in the
// landing scripts) may assign a string to innerHTML, outerHTML or insertAdjacentHTML: markup that exists up front is
// rendered into the page or into <template> elements at build time, and everything else is built from nodes.

export const $ = (selector, root) => (root || document).querySelector(selector);
export const $$ = (selector, root) => Array.from((root || document).querySelectorAll(selector));

const SVG_NS = 'http://www.w3.org/2000/svg';

/** An element with a class list and children (strings become text nodes; null and undefined are skipped). */
export function el(tag, cls, ...kids) {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  for (const kid of kids.flat()) {
    if (kid === null || kid === undefined) continue;
    node.append(typeof kid === 'string' ? document.createTextNode(kid) : kid);
  }
  return node;
}

/** `<svg class="ic ..."><use href="#i-NAME"/></svg>`: an icon of the sprite at the top of the page. */
export function ico(name, cls) {
  const svg = document.createElementNS(SVG_NS, 'svg');
  svg.setAttribute('class', 'ic ' + (cls || ''));
  svg.setAttribute('aria-hidden', 'true');
  const use = document.createElementNS(SVG_NS, 'use');
  use.setAttribute('href', '#i-' + name);
  svg.append(use);
  return svg;
}

/** `<svg class="pm [mono]"><use href="#i-PROVIDER"/></svg>`: a provider mark. */
export function mark(prov, mono) {
  const svg = document.createElementNS(SVG_NS, 'svg');
  svg.setAttribute('class', 'pm ' + (mono ? 'mono' : ''));
  const use = document.createElementNS(SVG_NS, 'use');
  use.setAttribute('href', '#i-' + prov);
  svg.append(use);
  return svg;
}

/** A clone of the content of a build-time <template>. */
export function fromTemplate(id) {
  const tpl = document.getElementById(id);
  if (!tpl) throw new Error('missing template ' + id);
  return tpl.content.cloneNode(true);
}

/** Text with `code` spans: backtick pairs become <code> elements. */
export function withCode(text) {
  const out = [];
  const re = /`([^`]+)`/g;
  let last = 0;
  let m;
  while ((m = re.exec(text))) {
    if (m.index > last) out.push(text.slice(last, m.index));
    out.push(el('code', '', m[1]));
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}
