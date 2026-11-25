// The privacy policy page of the zptah Browser extension (https://zptah.app/privacy). Static text,
// no script: it needs only landing.css. Conservative disclosure, see
// docs/superpowers/specs/2026-11-25-zptah-browser-extension.md section 13.3 and decision 6.
// Keep it in step with extensions/zptah-browser/store/listing.md, and bump UPDATED on any change.
import { APP_ICON } from '../data.mjs';
import { BETA_SITE, SITE } from './page.mjs';

export const UPDATED = '2026-11-26';
const REPO = 'https://github.com/hhoangg/zptah';
const ISSUES = `${REPO}/issues`;
const CHANGELOG = `${REPO}/commits/main/web/landing/src/html/privacy.mjs`;

const TITLE = 'Privacy policy - zptah Browser';
const DESCRIPTION = 'What the zptah Browser extension handles, where it goes, what is never collected, and where data is stored.';

const SECTIONS = [
  [
    'what',
    'What the extension handles',
    [
      '<p>When you ask a Mono (an assistant in the zptah desktop app) to use your browser, the zptah Browser extension handles:</p>',
      '<ul><li>the content of the pages a Mono reads, including text, links, form labels and screenshots of those pages;</li><li>the titles and addresses (URLs) of your tabs;</li><li>the targets of the actions a Mono takes (which button, field or link), and what it types when you asked it to.</li></ul>',
      '<p>The extension never reads passwords: a Mono is refused when it tries to type into a password field. It does nothing on a page until a Mono reads or acts on it, and it does not work in private (Incognito) windows.</p>',
    ],
  ],
  [
    'where',
    'Where it goes',
    [
      '<p>Only to the zptah app on your computer, over Chrome native messaging. The extension has no server, makes no network requests of its own and has no update or onboarding downloads.</p>',
      '<p>Inside the app, what a Mono reads becomes part of that Mono\'s conversation. zptah sends that conversation to the AI provider you configured (for example Anthropic or OpenAI), as it does for everything else in the conversation. Which provider sees it, and under what terms, is your choice in zptah and is governed by that provider\'s own policy. The general privacy notes of zptah are in the <a href="' + REPO + '#readme">project readme</a>.</p>',
    ],
  ],
  [
    'never',
    'What is never collected',
    [
      '<ul><li>No analytics, no telemetry and no crash reports, in the extension or in the zptah app\'s browser path.</li><li>No servers of our own receive your browsing data.</li><li>No sale and no transfer of data to third parties, other than the AI provider you chose, as described above and only to provide the feature you asked for.</li><li>No advertising and no use of data for ads.</li><li>No use of data to decide creditworthiness or for lending.</li><li>No use of data for any purpose unrelated to the extension\'s single purpose.</li></ul>',
      '<p>The extension\'s use of information received from Chrome follows the Chrome Web Store User Data Policy, including its Limited Use requirements.</p>',
    ],
  ],
  [
    'storage',
    'Where data is stored',
    [
      '<ul><li>In the browser\'s extension storage: the pairing key with the zptah app, which Mono owns which tab, and your cursor settings.</li><li>In the zptah app on your computer: the audit log of the actions Monos took and the approvals you gave, kept for 90 days or 20,000 entries per Mono. It is not uploaded anywhere.</li></ul>',
      '<p>Removing the extension deletes its storage.</p>',
    ],
  ],
  [
    'permissions',
    'Why the extension asks for broad access',
    [
      '<p>The extension asks for access to all sites because Monos work on the sites you ask them to use, and it cannot know in advance which. Each Mono has its own allowed and blocked site lists in zptah, and risky steps (payments, deletions, account security, sharing, posting publicly, accepting terms, entering codes, uploads) need your approval first. The optional debugger permission is requested only if you allow it. It is used for sites that ignore simulated input, for rich text editors and for page dialogs: it may attach before a delete or payment step so the confirmation dialog can be read, stays attached while the assistant works in that tab, and is detached when the work ends, after two idle minutes or when you press Stop. Chrome shows its debugging bar while it is in use.</p>',
    ],
  ],
  [
    'contact',
    'Contact and change log',
    [
      '<p>Questions or requests: open an issue at <a href="' + ISSUES + '">' + ISSUES + '</a>. zptah is open source, so you can read exactly what the extension does in the <a href="' + REPO + '">repository</a>.</p>',
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
<main id="top"><section class="sec doc"><div class="wrap"><h1>Privacy policy: zptah Browser</h1><p class="lead">The zptah Browser extension lets the Monos of your zptah desktop app read and use your browser tabs. This page says what it handles and what it does not. Last updated ${UPDATED}.</p>
<nav class="toc" aria-label="Contents">${SECTIONS.map(([id, t]) => `<a href="#${id}">${t}</a>`).join('')}</nav>
${SECTIONS.map(([id, t, body]) => `<section id="${id}"><h2>${t}</h2>\n${body.join('\n')}</section>`).join('\n')}
</div></section></main>
<footer><div class="wrap fw"><div class="fl"><img src="${APP_ICON}" alt="" width="22" height="22"><b>zptah</b><span>MIT licensed</span></div><div class="fr"><a href="${home}">Home</a><a href="${REPO}">GitHub</a></div></div></footer>
</body>
</html>
`;
}
