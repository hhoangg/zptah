// The whole page: head (meta, OG, canonical), header, hero with the demo stage, feature rows, providers, install,
// footer. `renderPage` is pure: the build script passes the target (prod or beta) and writes the string.
import { APP_ICON, PROV } from '../data.mjs';
import { hic, ic, sprite } from '../icons.mjs';
import { desktop, flyout, lights } from './desktop.mjs';
import { CH, fileRows, reviewBody } from './diff.mjs';
import { feats } from './features.mjs';
import { phone } from './phone.mjs';

export const SITE = 'https://zptah.app';
export const BETA_SITE = 'https://beta.zptah.app';
const TITLE = 'zptah - every coding agent, one window';
const DESCRIPTION =
  'zptah is an open source native desktop app that runs Claude Code, Codex, Cursor and more as tabs and splits, with worktrees, diff review and a phone companion.';
const OG_DESCRIPTION = 'Run the coding agents you already use as tabs and splits, keep each task in its own worktree, and approve from your phone.';

export const chips = [
  ['Start a session', 'Pick any of 11 agents, type, send. Tool calls stream in as they run.'],
  ['Split panes', 'Drag a tab to an edge. Two agents side by side, both still working.'],
  ['Review changes', 'Open the diff stat, pick a file, and read the real diff before you commit.'],
  ['Inbox to session', 'Open a Linear, GitHub or Jira ticket as a new session in one click.'],
  ['Approve from your phone', 'Tap the push, then Allow once in the app. The desktop carries on.'],
];

const brew = 'brew install --cask hhoangg/zptah/zptah';
const copyBtn = (cls = '') =>
  `<div class="brew ${cls}"><code>$ ${brew}</code><button class="copy" data-copy="${brew}" aria-label="Copy install command">${hic('copy')}<span>Copy</span></button></div>`;

const RELEASES = 'https://github.com/hhoangg/zptah/releases';
const REPO = 'https://github.com/hhoangg/zptah';

/** Where the web app lives for a target: relative on the official host, absolute on the beta host. */
export function appLinks(target) {
  const base = target === 'beta' ? SITE : '';
  return { login: `${base}/login`, register: `${base}/register`, privacy: `${base}/privacy` };
}

/** The data the demo script reads (JSON in a data block, so no inline script is needed). */
function demoData() {
  const changes = Object.fromEntries(Object.keys(CH).map((k) => [k, { n: CH[k].n, branch: CH[k].branch }]));
  const providers = Object.fromEntries(PROV.map((p) => [p.id, [p.tile[0], p.tile[1], p.mono ? 1 : 0, p.model]]));
  return JSON.stringify({ chips, changes, providers }).replace(/</g, '\\u003c');
}

/** Per-session file list and diff, cloned into the right panel and the review pane when the demo switches session. */
const changeTemplates = () =>
  Object.keys(CH)
    .map((k) => `<template id="chd-${k}-files">${fileRows(k)}</template><template id="chd-${k}-review">${reviewBody(k)}</template>`)
    .join('');

function head(target, site) {
  const beta = target === 'beta';
  const origin = beta ? BETA_SITE : site;
  const lines = [
    '<meta charset="utf-8">',
    '<meta name="viewport" content="width=device-width,initial-scale=1">',
    '<meta name="zptah-page" content="landing">',
    `<title>${TITLE}</title>`,
    `<meta name="description" content="${DESCRIPTION}">`,
    '<meta name="theme-color" content="#0a0a0a">',
    '<meta name="color-scheme" content="dark">',
  ];
  if (beta) lines.push('<meta name="robots" content="noindex, nofollow">');
  else lines.push(`<link rel="canonical" href="${site}/">`);
  lines.push(
    '<link rel="icon" href="/favicon.ico" sizes="32x32">',
    '<link rel="icon" href="/favicon.svg" type="image/svg+xml">',
    '<link rel="apple-touch-icon" href="/apple-touch-icon.png" sizes="180x180">',
    '<meta property="og:type" content="website">',
    '<meta property="og:site_name" content="zptah">',
    `<meta property="og:title" content="${TITLE}">`,
    `<meta property="og:description" content="${OG_DESCRIPTION}">`,
    `<meta property="og:url" content="${origin}/">`,
    `<meta property="og:image" content="${origin}/landing/og.png">`,
    '<meta property="og:image:type" content="image/png">',
    '<meta property="og:image:width" content="1200">',
    '<meta property="og:image:height" content="630">',
    '<meta property="og:image:alt" content="The zptah desktop window running an agent session next to the zptah web app on a phone">',
    '<meta name="twitter:card" content="summary_large_image">',
    `<meta name="twitter:title" content="${TITLE}">`,
    `<meta name="twitter:description" content="${OG_DESCRIPTION}">`,
    `<meta name="twitter:image" content="${origin}/landing/og.png">`,
    '<link rel="stylesheet" href="/landing/landing.css">',
    '<script src="/landing/js/boot.js"></script>',
  );
  return lines.join('\n');
}

/**
 * The landing page as an HTML string. `target` is `prod` (relative app links) or `beta` (absolute, noindex). `site` is the
 * origin of the host that serves it (canonical, og:url, og:image): the RP_ORIGIN of the build, `https://zptah.app` officially.
 */
export function renderPage(target = 'prod', site = SITE) {
  const link = appLinks(target);
  const provFly = flyout('claude', 'Claude Opus 5').replace('class="fly"', 'class="fly big" id="provfly"');
  return `<!doctype html>
<html lang="en">
<head>
${head(target, site)}
</head>
<body>
${sprite()}
<header class="nav-top"><a class="logo" href="#top"><img src="${APP_ICON}" alt="" width="26" height="26"><b>zptah</b></a><nav><a href="#features">Features</a><a href="#providers">Providers</a><a href="#install">Install</a></nav><a class="gh" href="${REPO}">${ic('star')}GitHub</a><div class="auth"><a class="signin" href="${link.login}">Sign in</a><a class="start" href="${link.register}">Get started</a></div></header>
<main id="top">
<section class="hero"><div class="wrap"><div class="tag">Open source · MIT · does not sell tokens</div><h1>Every coding agent. One window.</h1><p class="sub">zptah runs the agents you already use as tabs and splits, keeps each task in its own worktree, and pings your phone when one needs you.</p>
<p class="sub remote-line">Away from your desk? Control your agents from your phone over the internet. No shared Wi-Fi, no VPN.</p>
<div class="cta"><div class="cta-btns"><a class="btn pri" href="${RELEASES}">${hic('download')}Download for macOS</a><a class="btn" href="${link.login}">${hic('globe')}Open web app</a></div>${copyBtn()}</div></div>
<div class="wrap wide"><div class="stage" id="stage" data-focus="desktop">
<div class="dcol"><div class="dwrap" id="dwrap"><div id="dw">${desktop}</div></div></div>
<div class="pcol"><div class="pwrap"><div class="phone"><div class="bezel">${phone}</div></div><div class="pcap">${hic('phone')}zptah.app on your phone</div></div></div>
<div class="curlayer" aria-hidden="true"><div id="cur"><svg width="22" height="26" viewBox="0 0 22 26"><path d="M1.5 1.5v18.2l4.7-4.4 3.1 7.3 3.3-1.4-3.1-7.2 6.6-.1z" fill="#fff" stroke="#111" stroke-width="1.5" stroke-linejoin="round"/></svg></div><div id="fing"><i></i></div><div id="ghost"></div></div></div>
<div class="ctrl" id="ctrl"><div class="chips" role="tablist">${chips.map((c, i) => `<button class="chip2${i === 0 ? ' on' : ''}" role="tab" data-i="${i}" aria-selected="${i === 0}"><span class="n">${i + 1}</span><span class="lb">${c[0]}</span><i class="pg"></i></button>`).join('')}</div>
<div class="capr"><p class="cap" id="cap" aria-live="polite"><b>${chips[0][0]}.</b> ${chips[0][1]}</p><button class="pp" id="pp" aria-label="Pause demo"><svg viewBox="0 0 24 24" class="ic"><path class="pa" d="M8 5v14M16 5v14" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"/><path class="pplay" d="M7 5l12 7-12 7z" fill="currentColor"/></svg><span>Pause</span></button></div></div>
</div></section>

<section class="sec" id="features"><div class="wrap"><h2 class="reveal">Built for people who run many agents</h2><p class="lead reveal">Everything the demo just did, plus the parts you meet on day two.</p>
${feats.map(([k, t, d, v, id], i) => `<article class="frow2 reveal-grp${i % 2 ? ' rev' : ''}"${id ? ` id="${id}"` : ''}><div class="ft"><span class="kick">${k}</span><h3>${t}</h3><p>${d}</p></div><div class="fv">${v}</div></article>`).join('')}</div></section>

<section class="sec alt" id="providers"><div class="wrap"><div class="pgrid"><div class="ptxt"><h2>Eleven agents, one composer</h2><p class="lead">Pick the provider and model from the same menu the app uses. zptah drives the CLIs you already have installed and signed in to, so there are no tokens to buy from us. Missing CLIs are detected at startup and disabled with an install hint.</p><p class="hintp">Hover a provider on the rail.</p></div><div class="pfv">${provFly}</div></div></div></section>

<section class="sec" id="install"><div class="wrap"><h2>Get zptah</h2><p class="lead">macOS, Linux and Windows. Free and MIT licensed.</p>
<div class="igrid"><div class="term"><div class="tbar">${lights}<span>zsh</span></div><div class="tbody"><div class="tl1"><span class="tpr">~ %</span> <span class="tcmd">${brew}</span><button class="copy" data-copy="${brew}" aria-label="Copy install command">${hic('copy')}<span>Copy</span></button></div><div class="to">==&gt; Downloading zptah_aarch64.zip</div><div class="to">==&gt; Moving App 'zptah.app' to '/Applications/zptah.app'</div><div class="to ok">zptah was installed</div><div><span class="tpr">~ %</span> open -a zptah<i class="caret2"></i></div></div></div>
<div class="chooser"><div class="osw" role="tablist"><button class="os on" data-os="mac">${hic('download')}macOS</button><button class="os" data-os="linux">${hic('terminal')}Linux</button><button class="os" data-os="win">${hic('monitor')}Windows</button></div>
<div class="opanel on" data-os="mac"><p>Apple Silicon. Use brew (left) or download the zip.</p><a class="dl-row" href="${RELEASES}"><b>zptah.zip</b><em>macOS arm64</em>${hic('download')}</a></div>
<div class="opanel" data-os="linux"><p>x86_64. Pick the package your distro uses.</p><a class="dl-row" href="${RELEASES}"><b>zptah.AppImage</b><em>Any distro</em>${hic('download')}</a><a class="dl-row" href="${RELEASES}"><b>zptah.deb</b><em>Debian, Ubuntu</em>${hic('download')}</a><a class="dl-row" href="${RELEASES}"><b>zptah.rpm</b><em>Fedora, EL 10</em>${hic('download')}</a></div>
<div class="opanel" data-os="win"><p>x86_64 installer.</p><a class="dl-row" href="${RELEASES}"><b>zptah-setup.exe</b><em>Windows x64</em>${hic('download')}</a></div>
<a class="webrow" href="${link.login}">${hic('globe')}<span><b>Prefer the browser?</b> Open the web app at zptah.app, pair your desktop and use it from a phone.</span></a></div></div></div></section>
</main>
<footer><div class="wrap fw"><div class="fl"><img src="${APP_ICON}" alt="" width="22" height="22"><b>zptah</b><span>MIT licensed</span></div><div class="fr"><a href="${REPO}">GitHub</a><a href="${RELEASES}">Releases</a><a href="${link.login}">Web app</a><a href="${link.privacy}">Privacy</a></div><p class="cr">Built on the work of the open source authors of the project zptah grew from. Thank you.</p></div></footer>
${changeTemplates()}
<script type="application/json" id="landing-data">${demoData()}</script>
<script type="module" src="/landing/js/main.js"></script>
</body>
</html>
`;
}
