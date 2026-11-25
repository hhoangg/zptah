// Feature rows: one small UI vignette per feature.
import { hic, ic, pm } from '../icons.mjs';
import { CH, row } from './diff.mjs';
import { stIs, stPR } from './desktop.mjs';

/* ---------- feature rows ---------- */
export const wtRow = (cls, name, right, extra = '') => `<div class="wtr ${cls}">${ic('git-branch')}<b>${name}</b>${extra}<span class="sp"></span>${right}</div>`;
export const V = {};
V.worktrees = `<div class="mock wt"><div class="mh"><b>Worktrees</b><span class="btn-p sm">New worktree</span></div>
${wtRow('', 'main', '<em>Current checkout</em>', '<i class="tick">' + ic('check') + '</i>')}
${wtRow('a1', 'feat/rate-limit', pm('claude') + '<i class="spin"></i>')}
${wtRow('a2', 'fix/sync-retry', pm('cursor') + '<span class="teal">Done</span>')}
${wtRow('a3', 'eng-482-webhook-retries', pm('claude') + '<span class="amb">Needs approval</span>')}
${wtRow('a4', 'feat/dark-mode', pm('codex') + '<i class="spin"></i>')}
<div class="mf">Each session gets its own checkout and branch</div></div>`;
V.inbox = `<div class="mock ibm"><div class="srcs">${['github', 'gitlab-color', 'linear', 'jira', 'clickup', 'azuredevops'].map((s, i) => `<span class="sc src ${s}${i === 2 ? ' on' : ''}" style="--i:${i}">${ic('inbox-' + s)}</span>`).join('')}<span class="sp"></span><span class="addc">${ic('plus')}Add connection</span></div>
<div class="ibr ia1 unr"><div class="l1"><span class="src linear">${ic('inbox-linear')}</span>${stIs}<span class="kind">Assigned to you</span><span class="tm2">3m</span><i class="udot"></i></div><div class="l2">ENG-482 Webhook retries drop events under load</div><div class="l3">${ic('folder')}<span>Linear · Backend</span><span class="sp"></span><span class="btn-p sm go">Open in new session</span></div></div>
<div class="ibr ia2"><div class="l1"><span class="src github">${ic('inbox-github')}</span>${stPR}<span class="kind">Review requested</span><span class="tm2">9m</span></div><div class="l2">Add structured logging to the worker</div><div class="l3">${ic('folder')}<span>atlas-api</span><span class="sp"></span><span class="btn-p sm go">Review on Claude Code</span></div></div>
<div class="ibr ia3"><div class="l1"><span class="src jira" style="color:#2684ff">${ic('inbox-jira')}</span>${stIs}<span class="kind">Assigned to you</span><span class="tm2">1h</span></div><div class="l2">PAY-1093 Invoice export times out</div><div class="l3">${ic('folder')}<span>Jira · Payments</span><span class="sp"></span><span class="btn-p sm go">Open in new session</span></div></div></div>`;
V.monos = `<div class="mock mn2"><div class="mrow3"><svg class="inv" style="color:#34d399"><use href="#i-invader"/></svg><div><b>Release Mono</b><span>${pm('claude')}Claude Opus 5 · remembers 14 notes</span></div></div>
<div class="mem"><div>Prefers squash merges</div><div>Staging deploys happen on Fridays</div></div>
<div class="hab">${ic('clock')}<span>Habit</span><b>Weekdays 09:00</b><em>Triage the inbox</em></div>
<div class="wfl"><span class="nd">${ic('terminal')}Session</span><i></i><span class="nd n2">${ic('terminal')}Shell</span><i></i><span class="nd n3">${ic('mono')}Ask a Mono</span><i></i><span class="nd n4">${ic('workflow')}Condition</span></div><div class="mf">Workflows run as live graphs</div></div>`;
V.remote = `<div class="mock rm"><div class="mh"><b>Run on</b><span class="mut">zptah-host over SSH</span></div>
<div class="hr on">${ic('monitor')}<b>This Mac</b><em>Local</em><span class="sp"></span><i class="tick">${ic('check')}</i></div>
<div class="hr h1">${ic('terminal')}<b>build-box</b><em>Linux</em><span class="sp"></span><i class="gdot"></i><span>Online</span></div>
<div class="hr h2">${ic('monitor')}<b>mac-mini</b><em>macOS</em><span class="sp"></span><i class="gdot"></i><span>Online</span></div>
<div class="hr h3">${ic('terminal')}<b>nas</b><em>Linux</em><span class="sp"></span><i class="gdot off"></i><span>Offline</span></div>
<div class="mf">Host sessions keep running when you close the desktop</div></div>`;
V.cloud = `<div class="mock cl3"><div class="mh"><b>${pm('claude')}ENG-482 · Webhook retries</b><span class="chipc">${ic('cloud')}Codespace · <span class="teal">Available</span></span></div>
<div class="lane"><div class="ln">${ic('monitor')}<b>This computer</b></div><div class="mv2"><span class="btn-p sm c1">Continue in cloud</span><span class="btn-p sm c2">Continue locally</span></div><div class="ln r">${ic('cloud')}<b>GitHub Codespace</b></div></div>
<div class="track"><i></i></div><div class="mf">Move a running session and keep its context</div></div>`;
V.review = `<div class="mock rvm"><div class="rv-fh"><span class="cv2">${ic('chevron-down')}</span><b>client.ts</b><span class="fdir">src/sync</span><span class="sp"></span><span class="pl">+10</span><span class="mi2">-1</span></div><div class="rv-hh">@@ -20,5 +20,14 @@</div>${CH.s2.lines.slice(0, 8).map((l) => row(...l)).join('')}<div class="chg"><span>3 files</span><span class="sp"></span><b>Undo</b><b class="kp">Keep</b><b class="rvw">Review</b></div></div>`;
V.anywhere = `<div class="mock any"><div class="mh"><b>Remote control</b><span class="mut">Not on the same Wi-Fi</span></div>
<div class="any-flow"><div class="any-n"><span class="any-ic">${hic('phone')}</span><b>Your phone</b><em>Mobile data</em></div><div class="any-l"><i></i></div><div class="any-n any-r"><span class="any-ic">${ic('cloud')}</span><b>zptah.app</b><em>Relay</em></div><div class="any-l any-l2"><i></i></div><div class="any-n"><span class="any-ic">${ic('monitor')}</span><b>Your desktop</b><em>Home or office</em></div></div>
<div class="any-pts"><div>${ic('check')}Desktop connects out, no open ports</div><div>${ic('check')}Session traffic is end-to-end encrypted</div><div>${ic('check')}Sign in with a passkey</div></div>
<div class="mf">Pair once, revoke any device anytime</div></div>`;
export const feats = [
  ['Remote control', 'Reach your desktop from anywhere', 'Control your desktop agents from your phone or any browser over the internet. You do not need to be on the same LAN or Wi-Fi, and there is no VPN or port forwarding to set up: the desktop connects out to the zptah.app relay. Pair once, sign in with a passkey, and revoke devices anytime. Session traffic between your browser and your desktop is end-to-end encrypted, so the relay cannot read it.', V.anywhere, 'remote'],
  ['Worktrees', 'A worktree for every task', 'Give each session its own checkout and branch. Switch from the worktree picker, and agents stop stepping on each other.', V.worktrees],
  ['Inbox', 'Tickets become sessions', 'GitHub, GitLab, Linear, Jira, ClickUp and Azure DevOps in one list. Open any item in a new session, or ask an agent about it first.', V.inbox],
  ['Monos', 'Agents that remember', 'A Mono keeps a memory and a personality, runs habits on a schedule and drives workflows drawn as live run graphs.', V.monos],
  ['Remote machines', 'Run it on another box', 'zptah-host runs sessions on another Windows, Linux or macOS machine over SSH, paired from Settings. Close the laptop and they keep going.', V.remote],
  ['Cloud sessions', 'Start local, finish in the cloud', 'Run in a GitHub Codespace or a Docker Sandbox, and move the session between your computer and the cloud.', V.cloud],
  ['Review', 'Read the diff, then keep it', 'A real diff view with per-turn changes. Undo or keep each turn, then commit from the Changes panel.', V.review],
];

