// The phone replica (the zptah web app on a phone) and the data the demo narrates.
import { hic, ic, pm } from '../icons.mjs';
import { S, MACHINE, PROJECT, ENG_TITLE, psfx } from './common.mjs';
import { APP_ICON } from '../data.mjs';

/* ---------- phone ---------- */
export const prow = (id) => {
  const s = S[id];
  return `<div class="prow${s.hid ? ' hid' : ''}" data-s="${id}" data-t="prow-${id}"><div class="pl1"><span class="pti">${s.title}</span><span class="psf">${psfx(s.st, s.time)}</span></div><div class="pl2">${hic('git-branch')}<span class="pbr">${s.branch}</span><span class="sp"></span><span class="pmd">${s.model}</span><span class="phi">${pm(s.prov)}</span></div></div>`;
};
export const PROMPT_HTML = `Work on this item. Read its description and discussion, inspect the project, and implement the required changes.\n\nLinear ENG-482: Webhook retries drop events under load\nhttps://linear.app/atlas/issue/ENG-482\n\nUnder sustained load the dispatcher retries without backoff and drops events once the queue is full.`;
export const phone = `<div class="screen" id="ph"><div class="sbar"><span>9:41</span><i class="isl"></i><span class="bat"><i></i><u></u></span></div>
<div class="pw"><div class="ptop"><span class="pc">${hic('sidebar-right')}</span><span class="psw"><i class="ring"></i><b>Alex's MacB…</b><i class="gdot"></i><em>Online</em>${hic('chevron-down', 'cv')}</span><span class="sp"></span><span class="pc">${hic('search')}</span><span class="pc">${hic('user')}</span></div>
<div class="pv on" id="pv-list"><div class="pl-lab">Projects</div><div class="pj"><svg class="inv" style="color:#459cf7"><use href="#i-invader"/></svg><b>${PROJECT}</b></div><div class="pj"><svg class="inv" style="color:#ea603e"><use href="#i-invader"/></svg><b>atlas-web</b></div><div class="pl-lab" style="margin-top:14px">Recent sessions</div>${prow('s3')}${prow('s2')}${prow('s1')}${prow('o1')}${prow('o2')}${prow('o3')}<div class="pfoot"><span>${hic('terminal')}Terminal</span></div></div>
<div class="pv" id="pv-chat"><div class="pch"><span class="pcb">${hic('back')}</span><b>${ENG_TITLE}</b><span class="pcb">${hic('dots')}</span></div><div class="pbody"><div class="day"><b>Today</b> 9:41 AM</div><div class="pub"><span>${PROMPT_HTML}</span></div><div class="pts">9:41 AM</div><div class="pag" data-t="pag"></div></div>
<div class="pappr" data-t="pappr"><div class="pa-h">${hic('shield-alert')}<span>Needs your approval</span></div><div class="pa-l">Run npm test</div><div class="pa-d">npm test -- --filter webhook</div><div class="pa-st">Answer sent. Waiting for the desktop.</div><div class="pa-b"><button class="pdeny">Deny</button><button class="pallow" data-t="p-allow">Allow once</button></div></div>
<div class="pcomp" data-t="pcomp"><span class="pc">${hic('plus')}</span><span class="pc">${hic('mic')}</span><span class="pph"><span class="pb">Send a follow-up...</span><span class="pi">Plan, ask, build...</span></span><span class="psend"><i class="stp"></i>${hic('mono-arrow-up', 'arr')}</span></div></div></div>
<div class="push" id="push" data-t="push"><img src="${APP_ICON}" alt=""><div><div class="ph1"><b>${MACHINE}</b><em>now</em></div><div class="ph2">${PROJECT}: Needs your input</div></div></div></div>`;
