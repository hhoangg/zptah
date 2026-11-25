// The SVG sprite and the small icon helpers. Desktop glyphs come from ./icons/*.svg (the icons the GPUI app draws),
// the phone and page chrome use the Hugeicons the web app uses (same icon names as web/app/src/ui/Icon.tsx), read
// from the web app's dependency at build time so the landing and the app cannot drift.
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';
import { PROV, P } from './data.mjs';

const ICON_DIR = path.join(import.meta.dirname, 'icons');

/** landing icon name -> Hugeicons export used by the web app for the same element (web/app/src/ui/Icon.tsx). */
const HUGE = {
  'sidebar-right': 'LayoutAlignRightIcon',
  'chevron-down': 'ArrowDown01Icon',
  right: 'ArrowRight01Icon',
  search: 'Search01Icon',
  user: 'UserCircleIcon',
  plus: 'Add01Icon',
  mic: 'Mic01Icon',
  'mono-arrow-up': 'ArrowUp02Icon',
  back: 'ArrowLeft01Icon',
  dots: 'MoreHorizontalIcon',
  'shield-alert': 'ShieldAlertIcon',
  'git-branch': 'GitBranchIcon',
  terminal: 'ComputerTerminal01Icon',
  phone: 'SmartPhone01Icon',
  download: 'Download01Icon',
  copy: 'Copy01Icon',
  globe: 'Globe02Icon',
  check: 'Tick02Icon',
  folder: 'Folder01Icon',
  cloud: 'CloudIcon',
  monitor: 'ComputerIcon',
};

const webAppRequire = createRequire(path.join(import.meta.dirname, '..', '..', 'app', 'package.json'));
const hugeDir = path.join(path.dirname(webAppRequire.resolve('@hugeicons/core-free-icons/package.json')), 'dist', 'esm');
const kebab = (s) => s.replace(/[A-Z]/g, (m) => '-' + m.toLowerCase());

async function hugeSymbol(name, exportName) {
  const mod = await import(pathToFileURL(path.join(hugeDir, exportName + '.js')).href);
  const parts = mod.default.map(([tag, attrs]) => {
    const a = Object.entries(attrs)
      .filter(([k]) => k !== 'key' && k !== 'strokeWidth' && !(k === 'stroke' && attrs[k] === 'currentColor'))
      .map(([k, v]) => ` ${kebab(k)}="${v}"`)
      .join('');
    return `<${tag}${a}/>`;
  });
  // The web app draws every chrome icon at strokeWidth 1.75 (ui/Icon.tsx).
  return `<symbol id="i-hg-${name}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">${parts.join('')}</symbol>`;
}
const hugeSymbols = await Promise.all(Object.entries(HUGE).map(([n, e]) => hugeSymbol(n, e)));

function sym(id, file, extra = '', uiStroke = false) {
  let s = fs.readFileSync(path.join(ICON_DIR, file + '.svg'), 'utf8').trim();
  const m = s.match(/<svg([^>]*)>([\s\S]*)<\/svg>/);
  let attrs = m[1];
  let inner = m[2];
  let vb = (attrs.match(/viewBox="([^"]*)"/) || [])[1];
  if (!vb) {
    const w = (attrs.match(/width="([\d.]+)"/) || [])[1] || 24;
    const h = (attrs.match(/height="([\d.]+)"/) || [])[1] || 24;
    vb = `0 0 ${w} ${h}`;
  }
  if (uiStroke) inner = inner.replace(/stroke="white"/g, 'stroke="currentColor"').replace(/fill="white"/g, 'fill="currentColor"');
  return `<symbol id="i-${id}" viewBox="${vb}" ${extra}>${inner}</symbol>`;
}

const custom = {
  terminal: '<rect x="3" y="4" width="18" height="16" rx="4"/><path d="M8 10l3 2-3 2M13 15h3"/>',
  file: '<path d="M14 3H8a3 3 0 0 0-3 3v12a3 3 0 0 0 3 3h8a3 3 0 0 0 3-3V8z"/><path d="M14 3v5h5"/>',
  chart: '<rect x="3" y="3" width="18" height="18" rx="4"/><path d="M8 16v-3M12 16V9M16 16v-5"/>',
  user: '<circle cx="12" cy="12" r="9"/><circle cx="12" cy="10" r="3"/><path d="M6.5 18.5c1.2-2.2 3-3 5.5-3s4.3.8 5.5 3"/>',
  dots: '<circle cx="5" cy="12" r="1.2" fill="currentColor"/><circle cx="12" cy="12" r="1.2" fill="currentColor"/><circle cx="19" cy="12" r="1.2" fill="currentColor"/>',
  mic: '<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21"/>',
  back: '<path d="M15 5l-7 7 7 7"/>',
  chevup: '<path d="M6 15l6-6 6 6"/>',
  right: '<path d="M9 5l7 7-7 7"/>',
  alert: '<circle cx="12" cy="12" r="9"/><path d="M12 8v5M12 16.2v.01"/>',
  grip: '<g fill="currentColor" stroke="none"><circle cx="9" cy="6" r="1.3"/><circle cx="15" cy="6" r="1.3"/><circle cx="9" cy="12" r="1.3"/><circle cx="15" cy="12" r="1.3"/><circle cx="9" cy="18" r="1.3"/><circle cx="15" cy="18" r="1.3"/></g>',
  copy: '<rect x="8" y="8" width="12" height="12" rx="3"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/>',
  cloud: '<path d="M7 18a4 4 0 0 1-.5-7.97A6 6 0 0 1 18 9.5 4.25 4.25 0 0 1 17.5 18z"/>',
  monitor: '<rect x="3" y="4" width="18" height="12" rx="2.5"/><path d="M8 20h8M12 16v4"/>',
  bell: '<path d="M6 16V11a6 6 0 0 1 12 0v5l1.5 2h-15zM10 20a2 2 0 0 0 4 0"/>',
  phone: '<rect x="6" y="2.5" width="12" height="19" rx="3"/><path d="M11 18.5h2"/>',
  download: '<path d="M12 4v11M7.5 11l4.5 4.5 4.5-4.5M5 20h14"/>',
  globe: '<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c3 3 3 15 0 18M12 3c-3 3-3 15 0 18"/>',
  starf: '<path d="M12 3l2.7 5.6 6.1.8-4.5 4.2 1.1 6-5.4-3-5.4 3 1.1-6L3.2 9.4l6.1-.8z" fill="currentColor"/>',
  star: '<path d="M12 3l2.7 5.6 6.1.8-4.5 4.2 1.1 6-5.4-3-5.4 3 1.1-6L3.2 9.4l6.1-.8z"/>',
  branch2: '<circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="9" r="2"/><path d="M6 7v10M18 11c0 4-6 3-12 6"/>',
  mono: '<rect x="4" y="6" width="16" height="13" rx="4"/><circle cx="9.5" cy="12" r="1" fill="currentColor"/><circle cx="14.5" cy="12" r="1" fill="currentColor"/><path d="M12 3v3"/>',
  workflow: '<rect x="3" y="4" width="6" height="5" rx="1.5"/><rect x="15" y="4" width="6" height="5" rx="1.5"/><rect x="9" y="15" width="6" height="5" rx="1.5"/><path d="M6 9v2.5h12V9M12 11.5V15"/>',
  filter: '<path d="M4 5h16l-6.5 7.5V19l-3 1.5v-8z"/>',
  refresh: '<path d="M20 11a8 8 0 1 0-2.3 5.7M20 5v6h-6"/>',
  diffi: '<path d="M7 4v8M3 8h8M13 18h8"/>',
};

export function sprite() {
  const out = [];
  for (const p of PROV) {
    const file = p.id;
    out.push(sym(p.id, file, p.mono ? 'fill="currentColor"' : ''));
  }
  out.push(sym('codex-glyph', 'codex-glyph', 'fill="currentColor"'));
  for (const n of ['plus', 'close', 'chevron-down', 'search', 'settings', 'sidebar', 'sidebar-right', 'zap', 'folder', 'note', 'square-plus', 'mono-arrow-up', 'git-branch', 'check', 'check-circle-2', 'clock', 'square-lock', 'git-pull-request', 'circle-dot', 'sparkles', 'pencil-edit', 'shield-alert', 'external-link', 'inbox', 'circle-dashed-hi'])
    out.push(sym(n, n, '', true));
  for (const n of ['inbox-github', 'inbox-linear', 'inbox-jira', 'inbox-gitlab-color', 'inbox-clickup', 'inbox-azuredevops'])
    out.push(sym(n, n, n === 'inbox-github' || n === 'inbox-linear' || n === 'inbox-azuredevops' ? 'fill="currentColor"' : '', false));
  for (const [k, v] of Object.entries(custom))
    out.push(`<symbol id="i-${k}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">${v}</symbol>`);
  out.push(...hugeSymbols);
  // pixel invader
  const pat = ['..#.....#..', '...#...#...', '..#######..', '.##.###.##.', '###########', '#.#######.#', '#.#.....#.#', '...##.##...'];
  let r = '';
  pat.forEach((row, y) => [...row].forEach((c, x) => { if (c === '#') r += `<rect x="${x}" y="${y}" width="1" height="1"/>`; }));
  out.push(`<symbol id="i-invader" viewBox="0 0 11 8" fill="currentColor" shape-rendering="crispEdges">${r}</symbol>`);
  return `<svg width="0" height="0" style="position:absolute" aria-hidden="true"><defs>${out.join('')}</defs></svg>`;
}

export const ic = (n, c = '') => `<svg class="ic ${c}" aria-hidden="true"><use href="#i-${n}"/></svg>`;
export const pm = (id, c = '') => `<svg class="pm ${P[id].mono ? 'mono ' : ''}${c}" aria-hidden="true"><use href="#i-${id}"/></svg>`;
export const tile = (id) => `<span class="tile" style="background:linear-gradient(180deg,${P[id].tile[0]},${P[id].tile[1]})${id === 'antigravity' ? ';color:#222' : ''}">${pm(id, id === 'antigravity' ? 'keepcolor' : 'wh')}</span>`;
export const esc = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

export function hl(line) {
  const re = /("[^"]*"|'[^']*'|\/\/.*|\b(?:const|export|function|return|import|from|interface|new|let|if|else|async|await)\b|\b\d+\b|\b(?:number|string|Map|Bucket)\b)/g;
  let out = '', last = 0, m;
  while ((m = re.exec(line))) {
    out += esc(line.slice(last, m.index));
    const t = m[0];
    const cls = /^["']/.test(t) ? 's' : t.startsWith('//') ? 'c' : /^\d/.test(t) ? 'n' : /^(number|string|Map|Bucket)$/.test(t) ? 't' : 'k';
    out += `<span class="x${cls}">${esc(t)}</span>`;
    last = m.index + t.length;
  }
  return out + esc(line.slice(last));
}

/** A Hugeicons glyph (the same icon the web app uses), for the phone and the page chrome. */
export const hic = (n, c = '') => {
  if (!HUGE[n]) throw new Error('no Hugeicons mapping for ' + n);
  return `<svg class="ic hg ${c}" aria-hidden="true"><use href="#i-hg-${n}"/></svg>`;
};
