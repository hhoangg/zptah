// Shared demo data (machine, sessions, models) and the small row builders both replicas use.
import { ic, pm, tile } from '../icons.mjs';


export const MACHINE = "Alex's MacBook";
export const PROJECT = 'atlas-api';
export const ENG_TITLE = 'ENG-482 · Webhook retries drop events under load';

export const S = {
  s1: { prov: 'claude', title: 'Rate limiter middleware', branch: 'feat/rate-limit', st: 'working', time: '4m', model: 'Claude Opus 5' },
  s2: { prov: 'codex', title: 'New session', branch: 'fix/sync-retry', st: 'draft', time: 'now', model: 'Codex', hid: true },
  s3: { prov: 'claude', title: ENG_TITLE, branch: 'eng-482-webhook-retries', st: 'draft', time: 'now', model: 'Claude Opus 5', hid: true },
  o1: { prov: 'codex', title: 'Fix flaky retry test', branch: 'fix/flaky-retry', st: 'done', time: '12m', model: 'Codex' },
  o2: { prov: 'opencode', title: 'Migrate eslint config', branch: 'chore/eslint-9', st: 'time', time: '38m', model: 'OpenCode' },
  o3: { prov: 'grok', title: 'Review PR 212', branch: 'main', st: 'time', time: '1h', model: 'Grok 4.6' },
  y1: { prov: 'devin', title: 'Bump axios to 1.9', branch: 'chore/axios', st: 'time', time: '1d', model: 'Devin' },
  y2: { prov: 'hermes', title: 'Dashboard empty state', branch: 'feat/empty', st: 'time', time: '1d', model: 'Hermes' },
  y3: { prov: 'pi', title: 'Docs: webhook guide', branch: 'docs/webhooks', st: 'time', time: '2d', model: 'Pi' },
};
export const MODELS = { codex: ['Codex'], claude: ['Claude Opus 5'], cursor: ['Composer 2.5', 'Cursor Grok 4.6'], grok: ['Grok 4.6'], opencode: ['OpenCode'], antigravity: ['Antigravity'], pi: ['Pi'], omp: ['omp'], fx: ['fx'], hermes: ['Hermes Agent'], devin: ['Devin'] };

export const sfx = (st, time) => {
  if (st === 'working') return `<i class="spin"></i><span class="sw">Working…</span>`;
  if (st === 'approval') return `${ic('alert', 'amb')}<span class="amb">Needs approval</span>`;
  if (st === 'done') return `${ic('check-circle-2', 'teal')}<span class="teal">Done</span>`;
  if (st === 'draft') return `${ic('circle-dashed-hi')}<span>Draft</span>`;
  return `<span>${time}</span>`;
};
export const psfx = (st, time) => {
  if (st === 'working') return `<span class="pwk"><i class="pterm">&gt;_</i>Working...</span>`;
  if (st === 'approval') return `<span class="pamb">Needs approval</span>`;
  return `<span>${time === 'now' || st === 'draft' ? 'now' : time}</span>`;
};
export const smk = (s) => (s.st === 'working' ? `<i class="spin"></i>` : pm(s.prov));
export const srow = (id, active) => {
  const s = S[id];
  return `<div class="srow${active ? ' act' : ''}${s.hid ? ' hid' : ''}${s.st === 'draft' ? ' draft' : ''}" data-s="${id}" data-t="row-${id}"><span class="smk" data-p="${s.prov}">${smk(s)}</span><span class="stl">${s.title}</span><span class="sfx">${sfx(s.st, s.time)}</span></div>`;
};
export const nav = (n, icon, label, extra = '') => `<div class="nav" data-t="nav-${n}">${ic(icon)}<span>${label}</span>${extra}</div>`;
