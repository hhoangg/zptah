// The privacy policy page of zptah's Mono Browser (https://zptah.app/privacy). Static text, no
// script: it needs only landing.css. Conservative disclosure, see
// docs/superpowers/specs/2026-11-27-ardia-browser.md. Bump UPDATED on any change.
import { APP_ICON } from '../data.mjs';
import { BETA_SITE, SITE } from './page.mjs';

export const UPDATED = '2026-11-28';
const REPO = 'https://github.com/hhoangg/zptah';
const ISSUES = `${REPO}/issues`;
const CHANGELOG = `${REPO}/commits/main/web/landing/src/html/privacy.mjs`;

const TITLE = 'Privacy policy - zptah Mono Browser';
const DESCRIPTION = 'What zptah sends to the Ardia browser when a Mono uses it, what stays on your computer, what is never collected, and where data is stored.';

const SECTIONS = [
  [
    'what',
    'What the Mono Browser handles',
    [
      '<p>When you ask a Mono (an assistant in the zptah desktop app) to use your Ardia browser, zptah runs the <code>ardia</code> command on your computer for it. The zptah gate sends it what the Mono does, and some calls of its own:</p>',
      '<ul><li>commands: open a page, read it, take a snapshot or a screenshot, click, type, fill, press a key, scroll, list or run a flow;</li><li>the addresses (URLs) the Mono opens;</li><li>text the Mono types or fills, when your Mono acts at the Act level (typing into an ordinary field does not ask you first; risky steps do), never a password or a one-time code (zptah refuses those);</li><li>flows: listing, reading and running Ardia flows with their parameters, and saving the steps of a session as a flow;</li><li>uploads: after you approve a file, a private copy of it is handed to Ardia and goes to the site you are on;</li><li>zptah\'s own calls to the local <code>ardia</code> command: checking that Ardia is running and Control is on, listing profiles, reading the target of a step again before every step, reading a flow before it runs, closing the tabs it opened, and asking Ardia to quit.</li></ul>',
      '<p>What comes back is the content of the pages a Mono reads, including text, links, form labels and screenshots, and the titles and addresses of its tabs. A Mono only gets the tabs zptah opened for its conversation, and uses your own tab only when you ask it to and approve it.</p>',
    ],
  ],
  [
    'where',
    'Where it goes',
    [
      '<p>No zptah server is involved in browsing. Everything the gate does happens on your computer, between the zptah app and the Ardia browser you installed. zptah has no server that receives your browsing, and makes no network request of its own for the browser.</p>',
      '<p>Inside the app, what a Mono reads becomes part of that Mono\'s conversation. zptah sends that conversation to the AI provider you configured (for example Anthropic or OpenAI), as it does for everything else in the conversation. Which provider sees it, and under what terms, is your choice in zptah and is governed by that provider\'s own policy. The general privacy notes of zptah are in the <a href="' + REPO + '#readme">project readme</a>.</p>',
    ],
  ],
  [
    'never',
    'What is never collected',
    [
      '<ul><li>No analytics, no telemetry and no crash reports in the browser path of the zptah app.</li><li>No servers of our own receive your browsing data.</li><li>No sale and no transfer of data to third parties, other than the AI provider you chose, as described above and only to provide the feature you asked for.</li><li>No advertising and no use of data for ads.</li><li>No use of data to decide creditworthiness or for lending.</li><li>No use of data for any purpose unrelated to letting a Mono use your browser.</li></ul>',
    ],
  ],
  [
    'storage',
    'Where data is stored',
    [
      '<ul><li>In the zptah app on your computer: the audit log of the actions Monos took and the approvals you gave, kept for 90 days or 20,000 entries per Mono. It records the tool, the address (origin and path, never the query) and a short summary, never typed text. It is not uploaded anywhere.</li><li>In a private folder on your computer: copies of the files a Mono uploads, made only after you approve each upload and deleted when the conversation ends. Where there is no private folder, uploads are not available.</li><li>In the Mono\'s browser output folder on your computer: screenshots, page snapshots and console logs a Mono took, swept regularly, and kept longer only for the screenshots an audit entry points to. It is not uploaded by zptah.</li><li>In the Mono\'s conversation transcript on your computer, which holds what the Mono read and did, like any other conversation.</li><li>In Ardia: your logins, cookies and history, as for any use of Ardia. zptah does not copy them.</li></ul>',
    ],
  ],
  [
    'control',
    'Ardia\'s Control and who can drive Ardia',
    [
      '<p>The <code>ardia</code> command works only while Control is on in Ardia (Settings > Advanced). Control is Ardia\'s own model, not zptah\'s: while it is on and Ardia is running, any local program on your computer can drive Ardia and its logged-in profiles through the same command, not only zptah. zptah keeps that window short: on macOS it opens Ardia in the background only when a Mono needs it, shows when it started Ardia and lets you quit it, and quits an Ardia it started after 10 idle minutes by default. zptah\'s browser tools apply its checks: they ask for your approval before risky steps (payments, deletions, account security, sharing, posting publicly, accepting terms, uploads), never type passwords or one-time codes, and refuse sites on your blocked list or outside your allowed list. These checks apply only to what goes through zptah\'s tools. Claude is blocked from running <code>ardia</code> in its shell and Codex is told not to, but neither is airtight. Navigation a page starts by itself to a site outside your lists is caught after it happens, not before.</p>',
    ],
  ],
  [
    'contact',
    'Contact and change log',
    [
      '<p>Questions or requests: open an issue at <a href="' + ISSUES + '">' + ISSUES + '</a>. zptah is open source, so you can read exactly what it does in the <a href="' + REPO + '">repository</a>.</p>',
      '<p>Changes to this policy are made in the repository, where every change is dated and listed in the <a href="' + CHANGELOG + '">history of this page</a>. Last updated: ' + UPDATED + '.</p>',
    ],
  ],
];

/** The privacy page as an HTML string. `target` and `site` as in `renderPage`. */
export function renderPrivacy(target = 'prod', site = SITE) {
  const beta = target === 'beta';
  const origin = beta ? BETA_SITE : site;
  const head = [
    '<meta charset="utf-8">',
    '<meta name="viewport" content="width=device-width,initial-scale=1">',
    '<meta name="zptah-page" content="privacy">',
    `<title>${TITLE}</title>`,
    `<meta name="description" content="${DESCRIPTION}">`,
    '<meta name="theme-color" content="#0a0a0a">',
    '<meta name="color-scheme" content="dark">',
    beta ? '<meta name="robots" content="noindex, nofollow">' : `<link rel="canonical" href="${site}/privacy">`,
    '<link rel="icon" href="/favicon.ico" sizes="32x32">',
    '<link rel="icon" href="/favicon.svg" type="image/svg+xml">',
    '<meta property="og:type" content="website">',
    '<meta property="og:site_name" content="zptah">',
    `<meta property="og:title" content="${TITLE}">`,
    `<meta property="og:url" content="${origin}/privacy">`,
    '<link rel="stylesheet" href="/landing/landing.css">',
  ].join('\n');
  const home = beta ? `${SITE}/` : '/';
  return `<!doctype html>
<html lang="en">
<head>
${head}
</head>
<body>
<header class="nav-top"><a class="logo" href="${home}"><img src="${APP_ICON}" alt="" width="26" height="26"><b>zptah</b></a><nav></nav><a class="gh" href="${REPO}">GitHub</a></header>
<main id="top"><section class="sec doc"><div class="wrap"><h1>Privacy policy: zptah Mono Browser</h1><p class="lead">The Mono Browser lets the Monos of your zptah desktop app read and use your Ardia browser. This page says what it handles and what it does not. Last updated ${UPDATED}.</p>
<nav class="toc" aria-label="Contents">${SECTIONS.map(([id, t]) => `<a href="#${id}">${t}</a>`).join('')}</nav>
${SECTIONS.map(([id, t, body]) => `<section id="${id}"><h2>${t}</h2>\n${body.join('\n')}</section>`).join('\n')}
</div></section></main>
<footer><div class="wrap fw"><div class="fl"><img src="${APP_ICON}" alt="" width="22" height="22"><b>zptah</b><span>MIT licensed</span></div><div class="fr"><a href="${home}">Home</a><a href="${REPO}">GitHub</a></div></div></footer>
</body>
</html>
`;
}
