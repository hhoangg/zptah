// The desktop window replica: sidebar, tabs, chat views, composer with model menu, inbox, review pane, right panel.
import { ic, esc, pm, tile } from '../icons.mjs';
import { PROV, P } from '../data.mjs';
import { S, MODELS, PROJECT, ENG_TITLE, srow, nav } from './common.mjs';
import { CH, row, fileRows, reviewBody } from './diff.mjs';

/* ---------- transcript pieces ---------- */
export const fileChip = (t) => `<span class="chip">${ic('file')}<span>${esc(t)}</span></span>`;
export const tool = (k, verb, target, st = 'done') => k === 'bash'
  ? `<div class="tool ${st}" data-k="bash"><span class="cmd">${esc(target)}</span></div>`
  : `<div class="tool ${st}" data-k="${k}"><span class="tv">${verb}</span>${fileChip(target)}</div>`;
export const bubble = (t) => `<div class="ub"><span>${t}</span></div>`;

/* ---------- model menu + flyout ---------- */
export const flyout = (cur, curModel) => `<div class="fly"><div class="rail"><i class="ri fav" data-t="rail-fav">${ic('starf')}</i>${PROV.map((p) => `<i class="ri${p.id === cur ? ' on' : ''}" data-t="rail-${p.id}" data-p="${p.id}" title="${p.name}">${pm(p.id)}</i>`).join('')}</div><div class="fbody"><div class="fsearch">${ic('search')}<span>Search models…</span></div>${PROV.map((p) => `<div class="mlist${p.id === cur ? ' on' : ''}" data-p="${p.id}"><div class="fl-h">${p.name}</div>${MODELS[p.id].map((m, i) => `<div class="mrow${p.id === cur && m === curModel ? ' on' : ''}" data-t="mdl-${p.id}-${i}">${pm(p.id)}<span>${m}</span>${ic('check', 'pck')}</div>`).join('')}<div class="pinfo"><i class="gdot"></i>Detected on this machine</div></div>`).join('')}</div></div>`;
export const menu = (cur, curModel) => `<div class="pop" data-t="pop"><div class="mhd">Model and settings</div><div class="mrow2"><span>Effort</span><b>High</b>${ic('right', 'rc')}</div><div class="mrow2"><span>Fast</span><i class="tg"></i></div><div class="mrow2 modelrow" data-t="modelrow"><span>Model</span><b class="mv">${pm(cur)}<em>${curModel}</em></b>${ic('right', 'rc')}</div>${flyout(cur, curModel)}</div>`;

export const meter = `<svg class="meter" viewBox="0 0 14 14" aria-hidden="true"><circle cx="7" cy="7" r="5" fill="none" stroke="rgba(235,235,235,.25)" stroke-width="2"/><circle cx="7" cy="7" r="5" fill="none" stroke="rgba(235,235,235,.75)" stroke-width="2" stroke-dasharray="10.5 31.4" transform="rotate(-90 7 7)" stroke-linecap="round"/></svg>`;

export const composer = (id, o = {}) => {
  const p = P[S[id].prov];
  const model = o.model || S[id].model;
  return `<div class="cwrap"><div class="cbox${o.busy ? ' busy' : ''}" data-t="cbox">
<div class="cin" data-t="cin"><span class="ph">Ask, build, or describe a change…</span><span class="ct"></span><i class="caret"></i></div>
<div class="ctl"><button class="pill sq" tabindex="-1">${ic('plus')}</button>
<button class="pill mpill" data-t="model" tabindex="-1"><span class="mi">${pm(p.id)}</span><span class="ml">${model}</span><span class="ef">${o.ef || 'High'}</span>${ic('chevron-down', 'cv')}</button>
<button class="pill apill" tabindex="-1">${ic('square-lock')}<span>Supervised</span>${ic('chevron-down', 'cv')}</button>
<span class="sp"></span>${meter}
<button class="send" data-t="send" tabindex="-1">${ic('mono-arrow-up', 'sga')}<i class="sqi"></i><span class="sl">Send</span></button></div>
${menu(p.id, model)}</div>
<div class="wsbar">${ic('git-branch')}<span>Current checkout</span><b>${S[id].branch}</b></div></div>`;
};

export const phdr = (id) => `<div class="phdr"><span class="grip">${ic('grip')}</span>${tile(S[id].prov)}<span class="pt">${S[id].title}</span><span class="px">${ic('close')}</span></div>`;
export const view = (id, on, body, extra = '', cOpt = {}) => `<section class="chatv${on ? ' on' : ''}" data-s="${id}">${phdr(id)}<div class="tx"><div class="col" data-t="col">${body}</div></div><div class="shelf">${extra}</div>${composer(id, cOpt)}</section>`;

export const v1 = view('s1', true,
  bubble('Add a token-bucket rate limiter to the API middleware and cover it with tests') +
  `<div class="sfin">${ic('check')}<span>Thought</span></div>` +
  `<p class="ag">I will check how requests flow through the middleware chain and where the limits are configured.</p>` +
  tool('read', 'Read', 'src/middleware/index.ts') + tool('read', 'Read', 'src/config/limits.ts') + tool('edit', 'Edit', 'src/config/limits.ts') +
  `<p class="ag">I added a limiter that keeps one bucket per API key and refills it on every request. Limits live in <code>src/config/limits.ts</code>, so they can change without touching the middleware.</p>` +
  tool('edit', 'Edit', 'src/middleware/rateLimit.ts') + tool('edit', 'Write', 'test/rateLimit.test.ts') + tool('bash', '', 'npm test -- rateLimit', 'run') +
  `<div class="live"><span class="shim">Working…</span><span class="lt">1m 12s</span></div>`,
  '', { busy: true });
export const v2 = view('s2', false, '', '', { ef: 'Default' });
export const apprD = `<div class="appr" data-t="appr"><div class="appr-r">${ic('square-lock', 'amb')}<span class="at">Run a command?</span><span class="ab"><button class="b-deny">Deny</button><button class="b-sess">Allow for this session</button><button class="b-once" data-t="d-once">Allow once</button></span></div><div class="ad2">npm test -- --filter webhook</div></div>`;
export const v3 = view('s3', false, '', apprD);

export const tabEl = (id, on, hid, label, tileHtml) => `<div class="tab${on ? ' sel' : ''}${hid ? ' hid' : ''}" data-tab="${id}" data-t="tab-${id}">${tileHtml || tile(S[id].prov)}<span class="tl">${label}</span><i class="tdot"></i><span class="tx2" data-t="tabx-${id}">${ic('close')}</span></div>`;
export const neutral = (icon) => `<span class="tile neutral">${ic(icon)}</span>`;

/* ---------- inbox ---------- */
export const ibRow = (id, src, status, kind, title, time, repo, unread) => `<div class="ibr${unread ? ' unr' : ''}" data-t="ib-${id}"><div class="l1"><span class="src ${src}">${ic('inbox-' + src)}</span>${status}<span class="kind">${kind}</span><span class="tm2">${time}</span><i class="udot"></i></div><div class="l2">${title}</div><div class="l3">${ic('folder')}<span>${repo}</span></div><div class="inl"><button class="btn-p" data-t="ib-open-m">Open in new session</button></div></div>`;
export const stPR = `<span class="st g">${ic('git-pull-request')}</span>`;
export const stIs = `<span class="st g">${ic('circle-dot')}</span>`;
export const inbox = `<div class="inbox" id="inbox"><div class="ib-list"><div class="ib-h"><div class="srcs"><span class="sc on">All</span><span class="sc">${ic('inbox-github')}</span><span class="sc">${ic('inbox-linear')}</span><span class="sc"><span style="color:#2684ff">${ic('inbox-jira')}</span></span><span class="sc"><span style="color:#fc6d26">${ic('inbox-gitlab-color')}</span></span></div><div class="ib-tb"><div class="ib-s">${ic('search')}<span>Filter inbox</span></div><span class="ib2" title="Filter inbox">${ic('filter')}</span><span class="ib2" title="Mark all as read">${ic('check')}</span><span class="ib2" title="Refresh">${ic('refresh')}</span></div><div class="addc">${ic('plus')}Add connection</div></div>
${ibRow('gh1', 'github', stPR, 'Review requested', 'Add structured logging to the worker', '9m', 'atlas-api', true)}
${ibRow('lin', 'linear', stIs, 'Assigned to you', 'ENG-482 Webhook retries drop events under load', '3m', 'Linear · Backend', true)}
${ibRow('jira', 'jira', stIs, 'Assigned to you', 'PAY-1093 Invoice export times out', '1h', 'Jira · Payments', false)}
${ibRow('gh2', 'github', stIs, 'Mentioned you', 'Flaky: rate limit test fails on CI', '2h', 'atlas-api', false)}
</div><div class="ib-det"><div class="ib-empty">Select an inbox item</div><div class="ib-d" data-t="ib-d"><div class="d-src"><span class="src linear">${ic('inbox-linear')}</span><span>Linear</span><span class="st g">${ic('circle-dot')}</span><span class="kind">Assigned to you</span></div><h3>ENG-482 Webhook retries drop events under load</h3><div class="d-meta"><span class="av">A</span>Alex <i></i>Backend <i></i>Urgent <i></i>3m ago</div><p>Under sustained load the dispatcher retries without backoff and drops events once the queue is full. Add exponential backoff with jitter and keep delivery order.</p><div class="d-act"><button class="btn-p" data-t="ib-open">Open in new session</button><span class="ib2">${ic('folder')}</span><span class="ib2">${ic('copy')}</span><span class="ib2">${ic('external-link')}</span></div><div class="d-rel">Related threads</div><div class="d-none">No sessions yet</div></div></div></div>`;

export const review = `<div class="review" id="review"><div class="rv-tb"><span class="ibtn">${ic('chevup')}</span><span class="ibtn">${ic('chevron-down')}</span><span class="cnt2">1 of 1</span><span class="sp"></span><span class="rvt">Collapse all</span></div><div class="rv-wrap" id="rv-wrap">${reviewBody('s1')}</div></div>`;

export const rp = `<aside class="rp" id="rp"><div class="rp-in">
<div class="rp-tabs"><span class="rt">${ic('folder')}</span><span class="rt">${ic('git-branch')}</span><span class="rt">${ic('terminal')}</span></div>
<div class="rp-h"><b>Changes</b><span class="pl" id="rp-a">+${CH.s1.n[0]}</span><span class="mi2" id="rp-d">-${CH.s1.n[1]}</span><span class="sp"></span>${ic('git-branch')}<span class="br" id="rp-br">${CH.s1.branch}</span></div>
<div class="rp-msg"><span>Message (⌘↵ to commit)</span>${ic('sparkles')}</div>
<div class="rp-c"><button class="btn-c" data-t="commit">${ic('check')}Commit</button><button class="btn-cv">${ic('chevron-down')}</button></div>
<div class="rp-sec">${ic('chevron-down', 'cv')}<span>Changes</span><i class="cnt">3</i></div><div id="rp-files">${fileRows('s1')}</div>
</div></aside>`;

export const lights = `<span class="lights"><i style="background:#ff5f57"></i><i style="background:#febc2e"></i><i style="background:#28c840"></i></span>`;
export const desktop = `<div class="win" id="win" role="img" aria-label="zptah desktop app showing an agent session working">
<aside class="sb"><div class="sb-top">${lights}<span class="ibtn">${ic('sidebar')}</span></div>
<div class="sb-proj">${ic('folder')}<b>${PROJECT}</b><span>main</span>${ic('chevron-down', 'cv')}</div>
<nav class="sb-nav">${nav('today', 'clock', 'Today', '<i class="badge">4</i>')}${nav('new', 'square-plus', 'New', '<i class="hint">⌘T</i>')}${nav('inbox', 'inbox', 'Inbox', '<i class="adot"></i>')}${nav('notes', 'note', 'Notes')}${nav('auto', 'zap', 'Automations')}${nav('contrib', 'chart', 'Contributors')}</nav>
<div class="sb-list"><div class="sl-l">Today</div>${srow('s3')}${srow('s2')}${srow('s1', true)}${srow('o1')}${srow('o2')}${srow('o3')}<div class="sl-l">Yesterday</div>${srow('y1')}${srow('y2')}${srow('y3')}</div>
<div class="sb-foot"><span class="ibtn">${ic('settings')}</span><span class="upd">v0.9.0</span></div></aside>
<div class="mn"><div class="tabs"><span class="nl">${lights}</span>${tabEl('s1', true, false, S.s1.title)}${tabEl('s2', false, true, S.s2.title)}${tabEl('s3', false, true, ENG_TITLE)}${tabEl('ib', false, true, 'Inbox', neutral('inbox'))}${tabEl('rv', false, true, 'Review', neutral('diffi'))}<span class="ibtn plus" data-t="tab-plus">${ic('plus')}</span><span class="ibtn inb" data-t="nav-inbox-m">${ic('inbox')}</span><span class="sp"></span><span class="dstat" data-t="dstat"><span class="pl" id="ds-a">+${CH.s1.n[0]}</span> <span class="mi2" id="ds-d">-${CH.s1.n[1]}</span></span><span class="ibtn" data-t="rp-toggle">${ic('sidebar-right')}</span></div>
<div class="mbody"><div class="panes" id="panes"><div class="pane" id="pA">${v1}${v2}${v3}</div><div class="pane" id="pB"></div><div class="drop" id="drop"></div></div>${inbox}${review}</div>
<div class="mfoot"><span class="fb usage">${meter}62%</span><span class="fb">${ic('terminal')}Terminal</span><span class="fb">${ic('cloud')}Cloud</span></div></div>${rp}</div>`;

