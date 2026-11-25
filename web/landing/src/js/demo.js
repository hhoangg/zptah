// The hero demo: a scripted fake cursor drives a live replica of the desktop window and of the phone web app through
// five scenes. All markup of the two replicas is rendered at build time; this file only changes classes, text and
// small node trees (see dom.js: no HTML string sinks, the page runs under Trusted Types).
//
// Sections: shared clock, actors (cursor and finger), scene context, state helpers, scenes, controller.
import { $, $$, el, fromTemplate, ico, mark, withCode } from './dom.js';
import { pickRail } from './extras.js';

export function initDemo(data, RM) {
  const CHIPS = data.chips;
  const CHD = data.changes;
  const PROVT = data.providers; // id -> [tile top, tile bottom, mono, model name]
  const html = document.documentElement;
  const stage = $('#stage');
  const NARROW = matchMedia('(max-width:699px)');
  const CANCEL = { cancel: true };
  // The pristine replicas: every loop of the demo restarts from clones of these.
  const dwSaved = Array.from($('#dw').childNodes).map((n) => n.cloneNode(true));
  const phSaved = Array.from($('#ph').childNodes).map((n) => n.cloneNode(true));

  /* ---------- fit (scale) ---------- */
  function fit() {
    const dwrap = $('#dwrap');
    const dw = $('#dw');
    html.style.setProperty('--k', (dwrap.clientWidth / dw.offsetWidth).toFixed(4));
    const pw = $('.pcol').clientWidth - 22;
    const cap = innerWidth >= 1100 ? 1 : innerWidth >= 700 ? 0.85 : 0.9;
    html.style.setProperty('--pk', Math.min(pw / 390, cap).toFixed(4));
  }
  new ResizeObserver(fit).observe(stage);
  addEventListener('resize', fit);
  fit();

  /* ---------- shared clock ---------- */
  // One clock `T` advances only while the demo may run: not paused by the button, not while the pointer is over the
  // stage, only while the stage is in view and the tab is visible, and never under prefers-reduced-motion.
  let T = 0;
  let last = 0;
  let waiters = [];
  let anims = [];
  let userPaused = false;
  let hoverPaused = false;
  let inView = false;
  let run = null;
  const canRun = () => !userPaused && !hoverPaused && inView && !document.hidden && !RM.matches;
  function frame(ts) {
    const dt = Math.min(ts - (last || ts), 50);
    last = ts;
    if (canRun()) {
      T += dt;
      const a = anims;
      anims = [];
      a.forEach((n) => {
        const p = Math.min((T - n.t0) / n.dur, 1);
        n.fn(p);
        if (p >= 1) n.res();
        else anims.push(n);
      });
      const w = waiters;
      waiters = [];
      w.forEach((n) => {
        if (n.at <= T) n.res();
        else waiters.push(n);
      });
      if (run && run.est) {
        const ch = $$('.chip2')[run.scene];
        if (ch) ch.style.setProperty('--p', Math.min((T - run.t0) / run.est, 1));
      }
    }
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
  function cancelTok(tok) {
    tok.cancelled = true;
    waiters = waiters.filter((n) => {
      if (n.tok === tok) {
        n.res();
        return false;
      }
      return true;
    });
    anims = anims.filter((n) => {
      if (n.tok === tok) {
        n.res();
        return false;
      }
      return true;
    });
  }

  /* ---------- actors ---------- */
  const actors = { cur: { el: $('#cur'), x: 0, y: 0, shown: false }, fing: { el: $('#fing'), x: 0, y: 0, shown: false } };
  const ghost = $('#ghost');
  let ghostOn = false;
  function setA(a, x, y) {
    a.x = x;
    a.y = y;
    a.el.style.transform = 'translate(' + x + 'px,' + y + 'px)';
    if (ghostOn && a === actors.cur) ghost.style.transform = 'translate(' + x + 'px,' + y + 'px)';
  }
  function pos(node, o) {
    o = o || {};
    const s = stage.getBoundingClientRect();
    const r = node.getBoundingClientRect();
    return [
      r.left - s.left + r.width * (o.fx == null ? 0.5 : o.fx) + (o.dx || 0),
      r.top - s.top + r.height * (o.fy == null ? 0.5 : o.fy) + (o.dy || 0),
    ];
  }
  let hovered = [];
  function clearHov() {
    hovered.forEach((e) => e.classList.remove('hov'));
    hovered = [];
  }
  // On a narrow screen the sidebar is hidden, so the same step uses the title bar twin of the control.
  const ALT = { 'nav-new': 'tab-plus', 'nav-inbox': 'nav-inbox-m', 'ib-open': 'ib-open-m' };
  function TG(name, root) {
    const names = NARROW.matches && ALT[name] ? [ALT[name], name] : [name];
    for (const n of names) {
      const f = $$('[data-t="' + n + '"]', root).filter((e) => e.offsetParent !== null || e.classList.contains('push'));
      if (f.length) return f[0];
    }
    throw new Error('target not found: ' + name);
  }

  /* ---------- scene context ---------- */
  // `fast` plays a scene to its final state at once (used to build the state before a chosen scene, and for the static
  // reduced-motion view).
  function mk(tok, fast) {
    const chk = () => {
      if (tok.cancelled) throw CANCEL;
    };
    const c = { tok, fast, rm: RM.matches };
    c.j = (ms) => ms * (0.85 + Math.random() * 0.35);
    c.sleep = (ms) => {
      if (fast) {
        chk();
        return Promise.resolve();
      }
      return new Promise((res) => waiters.push({ at: T + ms, res, tok })).then(chk);
    };
    c.anim = (dur, fn) => new Promise((res) => anims.push({ t0: T, dur, fn, res, tok })).then(chk);
    c.move = (node, o, a) => {
      a = a || actors.cur;
      o = o || {};
      const [tx, ty] = pos(node, o);
      if (fast) {
        chk();
        setA(a, tx, ty);
        return Promise.resolve();
      }
      const other = a === actors.cur ? actors.fing : actors.cur;
      other.el.style.opacity = 0;
      if (!a.shown) {
        const s = stage.getBoundingClientRect();
        const pr = $('.pcol').getBoundingClientRect();
        if (a === actors.fing) setA(a, pr.left - s.left + pr.width * 0.6, pr.top - s.top + pr.height * 0.92);
        else setA(a, s.width * 0.55, s.height * 0.95);
        a.shown = true;
        a.el.getBoundingClientRect();
        a.el.style.opacity = 1;
      } else a.el.style.opacity = 1;
      const sx = a.x;
      const sy = a.y;
      const dx = tx - sx;
      const dy = ty - sy;
      const d = Math.hypot(dx, dy) || 1;
      const dur = (340 + d * 0.62) * (0.88 + Math.random() * 0.3);
      const bend = (Math.random() < 0.5 ? -1 : 1) * d * (0.07 + Math.random() * 0.13);
      const cx = sx + dx / 2 - (dy / d) * bend;
      const cy = sy + dy / 2 + (dx / d) * bend;
      return c
        .anim(dur, (p) => {
          const e = p < 0.5 ? 4 * p * p * p : 1 - Math.pow(-2 * p + 2, 3) / 2;
          const u = 1 - e;
          setA(a, u * u * sx + 2 * u * e * cx + e * e * tx, u * u * sy + 2 * u * e * cy + e * e * ty);
        })
        .then(() => setA(a, tx, ty));
    };
    c.hover = (node) => {
      if (fast) return;
      clearHov();
      node.classList.add('hov');
      hovered.push(node);
    };
    c.press = (a) => {
      a = a || actors.cur;
      if (fast) return Promise.resolve();
      a.el.classList.add('dn');
      const r = el('div', 'rip');
      r.style.left = a.x + 'px';
      r.style.top = a.y + 'px';
      if (a === actors.fing) r.style.transform = 'scale(1.6)';
      $('.curlayer').appendChild(r);
      setTimeout(() => r.remove(), 520);
      return c.sleep(100).then(() => a.el.classList.remove('dn'));
    };
    c.click = (node, o, a) => {
      a = a || actors.cur;
      return c
        .move(node, o, a)
        .then(() => {
          c.hover(node);
          return c.sleep(c.j(170));
        })
        .then(() => c.press(a))
        .then(() => {
          clearHov();
          return c.sleep(c.j(120));
        });
    };
    c.type = (node, text) => {
      const box = node.closest('.cbox');
      box.classList.add('focus');
      const ct = $('.ct', node);
      if (fast) {
        ct.textContent = text;
        box.classList.add('has-text');
        return Promise.resolve();
      }
      let i = 0;
      const step = () => {
        if (i >= text.length) return Promise.resolve();
        i++;
        ct.textContent = text.slice(0, i);
        box.classList.add('has-text');
        return c.sleep(24 + Math.random() * 21 + (text[i - 1] === ' ' ? 18 : 0)).then(step);
      };
      return step();
    };
    c.focus = (w) => {
      stage.dataset.focus = w;
      if (w === 'desktop') actors.fing.el.style.opacity = 0;
      return c.sleep(fast ? 0 : 450);
    };
    return c;
  }

  /* ---------- state helpers ---------- */
  function sfx(st, time) {
    if (st === 'working') return [el('i', 'spin'), el('span', 'sw', 'Working…')];
    if (st === 'approval') return [ico('alert', 'amb'), el('span', 'amb', 'Needs approval')];
    if (st === 'done') return [ico('check-circle-2', 'teal'), el('span', 'teal', 'Done')];
    if (st === 'draft') return [ico('circle-dashed-hi'), el('span', '', 'Draft')];
    return [el('span', '', time || 'now')];
  }
  function psfx(st) {
    if (st === 'working') return [el('span', 'pwk', el('i', 'pterm', '>_'), 'Working...')];
    if (st === 'approval') return [el('span', 'pamb', 'Needs approval')];
    return [el('span', '', 'now')];
  }
  const markNode = (prov) => mark(prov, PROVT[prov] && PROVT[prov][2]);
  function setSt(id, st) {
    const row = $('.srow[data-s="' + id + '"]');
    if (row) {
      $('.sfx', row).replaceChildren(...sfx(st));
      row.classList.toggle('draft', st === 'draft');
      const mkEl = $('.smk', row);
      mkEl.replaceChildren(st === 'working' ? el('i', 'spin') : markNode(mkEl.dataset.p));
    }
    const p = $('.prow[data-s="' + id + '"] .psf');
    if (p) p.replaceChildren(...psfx(st));
    const t = $('.tab[data-tab="' + id + '"]');
    if (t) t.classList.toggle('needs', st === 'approval');
  }
  function reveal(id) {
    $$('.srow[data-s="' + id + '"],.prow[data-s="' + id + '"],.tab[data-tab="' + id + '"]').forEach((e) => {
      e.classList.remove('hid');
      if (e.classList.contains('tab')) e.classList.add('pop-in');
    });
  }
  function setTitle(id, t) {
    ['.srow[data-s="X"] .stl', '.tab[data-tab="X"] .tl', '.prow[data-s="X"] .pti', '.chatv[data-s="X"] .pt'].forEach((q) => {
      const e = $(q.replace('X', id));
      if (e) e.textContent = t;
    });
  }
  function setProv(id, prov) {
    const d = PROVT[prov];
    const mkEl = $('.srow[data-s="' + id + '"] .smk');
    if (mkEl) {
      mkEl.dataset.p = prov;
      if (!$('.spin', mkEl)) mkEl.replaceChildren(markNode(prov));
    }
    ['.prow[data-s="X"] .phi', '.chatv[data-s="X"] .mi'].forEach((q) => {
      const h = $(q.replace('X', id));
      if (!h) return;
      const s = $('svg', h);
      $('use', s).setAttribute('href', '#i-' + prov);
      s.classList.toggle('mono', !!d[2]);
    });
    $$('.tab[data-tab="' + id + '"] .tile,.chatv[data-s="' + id + '"] .phdr .tile').forEach((t) => {
      t.style.background = 'linear-gradient(180deg,' + d[0] + ',' + d[1] + ')';
      $('use', t).setAttribute('href', '#i-' + prov);
    });
    const pm = $('.prow[data-s="' + id + '"] .pmd');
    if (pm) pm.textContent = d[3];
  }
  let TABS = { s1: ['s1'], s2: ['s2'], s3: ['s3'] };
  function setFocus(sid) {
    const d = CHD[sid];
    if (!d) return;
    $('#ds-a').textContent = '+' + d.n[0];
    $('#ds-d').textContent = '-' + d.n[1];
    $('#rp-a').textContent = '+' + d.n[0];
    $('#rp-d').textContent = '-' + d.n[1];
    $('#rp-br').textContent = d.branch;
    $('#rp-files').replaceChildren(fromTemplate('chd-' + sid + '-files'));
    $('#rv-wrap').replaceChildren(fromTemplate('chd-' + sid + '-review'));
  }
  function showTab(key) {
    const a = TABS[key][0];
    const b = TABS[key][1];
    const pA = $('#pA');
    const pB = $('#pB');
    $$('.chatv').forEach((v) => {
      v.classList.remove('on');
      (v.dataset.s === b ? pB : pA).appendChild(v);
    });
    $('.chatv[data-s="' + a + '"]').classList.add('on');
    if (b) $('.chatv[data-s="' + b + '"]').classList.add('on');
    $('#win').classList.toggle('split', !!b);
    $$('.tab').forEach((t) => t.classList.toggle('sel', t.dataset.tab === key));
    $$('.srow').forEach((r) => r.classList.toggle('act', r.dataset.s === a));
    $$('.nav').forEach((n) => n.classList.remove('act'));
    setFocus(a);
  }
  /* overlays: Inbox and Review open as their own title tabs */
  function openOv(name, tabId) {
    $$('.tab').forEach((t) => t.classList.remove('sel'));
    const t = $('.tab[data-tab="' + tabId + '"]');
    t.classList.remove('hid');
    t.classList.add('sel', 'pop-in');
    $$('.srow').forEach((r) => r.classList.remove('act'));
    $('#' + name).classList.add('show');
    if (name === 'inbox') $('[data-t="nav-inbox"]').classList.add('act');
  }
  function closeOv(name, tabId, back) {
    $('#' + name).classList.remove('show', 'picked');
    $('.tab[data-tab="' + tabId + '"]').classList.add('hid');
    $$('.ibr.sel').forEach((e) => e.classList.remove('sel'));
    showTab(back);
  }
  const V = (s) => $('.chatv[data-s="' + s + '"]');
  const COL = (s) => $('.col', V(s));
  function add(s, node) {
    COL(s).appendChild(node);
    return node;
  }
  // The phone shows the session the demo follows (s3). Only what the desktop has already drawn is added to it.
  const phoneFeed = (s) => (s === 's3' ? $('[data-t=pag]') : null);
  function mirror(s, node) {
    const g = phoneFeed(s);
    if (g) g.appendChild(node);
    return node;
  }
  function toolNode(k, verb, target, st) {
    if (k === 'bash') return el('div', 'tool ' + st, el('span', 'cmd', target));
    return el('div', 'tool ' + st, el('span', 'tv', verb), el('span', 'chip', ico('file'), el('span', '', target)));
  }
  function finishTool(node) {
    node.className = 'tool done';
  }
  async function tool(c, s, k, verb, target, ms, keepRunning) {
    const node = add(s, toolNode(k, verb, target, 'run'));
    node.dataset.k = k;
    mirror(s, el('div', 'pt2', (k === 'bash' ? '$ ' : verb + ' ') + target));
    if (keepRunning) return node;
    await c.sleep(c.j(ms));
    finishTool(node);
    await c.sleep(c.j(260));
    return node;
  }
  async function think(c, s, ms) {
    const node = add(s, el('div', 'think shim', 'Thinking…'));
    await c.sleep(c.j(ms));
    node.remove();
  }
  // The desktop paragraph and its phone twin grow together: the phone never shows a word the desktop has not
  // streamed yet (the phone paragraph is filled from the same `out` string, after the desktop's).
  async function stream(c, s, text) {
    const p = add(s, el('p', 'ag'));
    const feed = phoneFeed(s);
    const phoneP = feed ? mirror(s, el('p')) : null;
    const draw = (t, cursor) => {
      p.replaceChildren(...withCode(t), ...(cursor ? [el('i', 'cursorb')] : []));
      if (phoneP) phoneP.textContent = t;
    };
    if (c.fast) {
      draw(text, false);
      return;
    }
    const words = text.split(' ');
    let i = 0;
    let out = '';
    while (i < words.length) {
      const n = 1 + (Math.random() < 0.5 ? 1 : 0);
      out += (out ? ' ' : '') + words.slice(i, i + n).join(' ');
      i += n;
      draw(out, true);
      await c.sleep(34 + Math.random() * 40);
    }
    draw(text, false);
  }
  function fin(s, t) {
    const cb = $('.cbox', V(s));
    cb.classList.remove('busy');
    add(s, el('div', 'sfin', ico('check'), el('span', '', t)));
    $('.shelf', V(s)).appendChild(
      el('div', 'chg', el('span', '', '3 files'), el('span', 'sp'), el('b', '', 'Undo'), el('b', '', 'Keep'), el('b', 'rvw', 'Review')),
    );
  }
  function user(s, t, long) {
    const d = add(s, el('div', 'ub' + (long ? ' clamp' : ''), el('span'), long ? el('span', 'more', 'Show more') : null));
    $('span', d).textContent = t;
  }
  function submit(s) {
    const b = $('.cbox', V(s));
    b.classList.remove('focus', 'has-text');
    b.classList.add('busy');
    $('.ct', b).textContent = '';
  }
  const ITEM_PROMPT =
    'Work on this item. Read its description and discussion, inspect the project, and implement the required changes.\n\nLinear ENG-482: Webhook retries drop events under load\nhttps://linear.app/atlas/issue/ENG-482\n\nUnder sustained load the dispatcher retries without backoff and drops events once the queue is full.';

  /* ---------- scenes ---------- */
  const S = [];
  // 1. Start a session: new tab, pick Cursor in the model menu, type, send, tools stream in.
  S[0] = async (c) => {
    await c.sleep(1600);
    await c.click(TG('nav-new'));
    reveal('s2');
    setSt('s2', 'draft');
    TABS.s2 = ['s2'];
    showTab('s2');
    const v = V('s2');
    const cb = $('.cbox', v);
    await c.sleep(c.j(700));
    await c.click($('[data-t=model]', v));
    cb.classList.add('pk-open');
    await c.sleep(c.j(450));
    const mr = $('[data-t=modelrow]', v);
    await c.move(mr);
    c.hover(mr);
    cb.classList.add('fly-open');
    await c.sleep(c.j(500));
    const rc = $('[data-t="rail-claude"]', v);
    await c.move(rc);
    pickRail(v, 'claude');
    c.hover(rc);
    await c.sleep(c.j(350));
    const rr = $('[data-t="rail-cursor"]', v);
    await c.move(rr);
    pickRail(v, 'cursor');
    c.hover(rr);
    await c.sleep(c.j(400));
    const mdl = $('[data-t="mdl-cursor-0"]', v);
    await c.click(mdl);
    $$('.mrow', v).forEach((r) => r.classList.remove('on'));
    mdl.classList.add('on');
    $('.ml', v).textContent = 'Composer 2.5';
    $('.ef', v).textContent = 'High';
    setProv('s2', 'cursor');
    $('.mv', v).replaceChildren(markNode('cursor'), el('em', '', 'Composer 2.5'));
    cb.classList.remove('fly-open');
    await c.sleep(c.j(150));
    cb.classList.remove('pk-open');
    await c.sleep(c.j(350));
    const cin = $('[data-t=cin]', v);
    await c.click(cin);
    await c.type(cin, 'Add retry with exponential backoff to the sync client');
    await c.sleep(c.j(350));
    await c.click($('[data-t=send]', v));
    user('s2', 'Add retry with exponential backoff to the sync client');
    submit('s2');
    setTitle('s2', 'Retry backoff in sync client');
    setSt('s2', 'working');
    await c.sleep(300);
    await think(c, 's2', 1400);
    await tool(c, 's2', 'read', 'Read', 'src/sync/client.ts', 800);
    await tool(c, 's2', 'read', 'Read', 'src/sync/errors.ts', 700);
    await tool(c, 's2', 'edit', 'Edit', 'src/sync/client.ts', 1100);
    await tool(c, 's2', 'bash', 'Run', 'npm test -- sync', 0, true);
    await c.sleep(1200);
  };
  // 2. Split panes: drag a tab to the edge; both sessions finish side by side.
  S[1] = async (c) => {
    await c.sleep(500);
    const t1 = $('[data-tab="s1"]');
    await c.move(t1);
    c.hover(t1);
    await c.sleep(c.j(240));
    actors.cur.el.classList.add('dn');
    const g = $('.tl', t1).textContent;
    ghost.replaceChildren($('.tile', t1).cloneNode(true), el('span', '', g));
    ghost.style.display = 'flex';
    ghostOn = true;
    ghost.style.transform = 'translate(' + actors.cur.x + 'px,' + actors.cur.y + 'px)';
    clearHov();
    await c.sleep(c.j(160));
    $('#drop').classList.add('on');
    await c.move($('#drop'), { fx: 0.6 });
    await c.sleep(c.j(350));
    ghostOn = false;
    ghost.style.display = 'none';
    $('#drop').classList.remove('on');
    actors.cur.el.classList.remove('dn');
    c.press();
    TABS.s2 = ['s2', 's1'];
    $('.tab[data-tab="s1"]').classList.add('hid');
    const tt = $('.tab[data-tab="s2"]');
    if (!$('.stk', tt)) {
      const front = $('.tile', tt);
      const back = $('.tab[data-tab="s1"] .tile').cloneNode(true);
      front.classList.add('front');
      back.classList.add('back');
      const stk = el('span', 'stk');
      tt.insertBefore(stk, front);
      stk.appendChild(back);
      stk.appendChild(front);
    }
    showTab('s2');
    await c.sleep(900);
    const b2 = $('.tool.run', COL('s2'));
    await Promise.all([
      (async () => {
        await c.sleep(600);
        if (b2) finishTool(b2);
        await c.sleep(500);
        await stream(c, 's2', 'Retries now use exponential backoff with jitter, capped at five attempts. The sync tests pass.');
        fin('s2', 'Worked for 24s');
        setSt('s2', 'done');
      })(),
      (async () => {
        const r = $('.tool.run', COL('s1'));
        await c.sleep(1400);
        if (r) finishTool(r);
        const lv = $('.live', COL('s1'));
        if (lv) lv.remove();
        await c.sleep(500);
        await stream(c, 's1', 'The limiter is in place. Requests over the burst limit now get a 429 with a Retry-After header.');
        fin('s1', 'Worked for 1m 31s');
        setSt('s1', 'done');
      })(),
    ]);
    await c.sleep(1800);
  };
  // 3. Review changes: open the right panel and the diff of the first file.
  S[2] = async (c) => {
    await c.sleep(500);
    await c.click(TG('dstat'));
    $('#win').classList.add('rp-open');
    await c.sleep(c.j(900));
    const f = $('[data-t="file-0"]');
    await c.click(f);
    f.classList.add('sel');
    openOv('review', 'rv');
    await c.sleep(c.j(2800));
    const cm = $('[data-t=commit]');
    await c.move(cm, { dx: 20 });
    c.hover(cm);
    await c.sleep(c.j(1300));
    clearHov();
  };
  // 4. Inbox to session: pick the Linear ticket, open it as a new session that starts reading code.
  S[3] = async (c) => {
    await c.sleep(400);
    await c.click(TG('dstat'));
    $('#win').classList.remove('rp-open');
    await c.sleep(c.j(700));
    await c.click($('[data-t="tabx-rv"]'));
    closeOv('review', 'rv', 's2');
    await c.sleep(c.j(400));
    await c.click(TG('nav-inbox'));
    openOv('inbox', 'ib');
    await c.sleep(c.j(800));
    const row = TG('ib-lin');
    await c.move(row, { fx: 0.4 });
    c.hover(row);
    await c.sleep(c.j(450));
    await c.press();
    row.classList.add('sel');
    $('#inbox').classList.add('picked');
    clearHov();
    await c.sleep(c.j(1100));
    await c.click(TG('ib-open'));
    closeOv('inbox', 'ib', 's2');
    reveal('s3');
    setSt('s3', 'working');
    TABS.s3 = ['s3'];
    showTab('s3');
    user('s3', ITEM_PROMPT, true);
    submit('s3');
    await c.sleep(400);
    await think(c, 's3', 1200);
    await tool(c, 's3', 'read', 'Read', 'src/webhooks/dispatcher.ts', 800);
    await tool(c, 's3', 'read', 'Read', 'src/queue/retry.ts', 700);
    await c.sleep(600);
  };
  // 5. Approve from the phone: the agent asks, the push arrives, Allow once, the desktop carries on.
  S[4] = async (c) => {
    await c.focus('desktop');
    await c.sleep(400);
    await tool(c, 's3', 'edit', 'Edit', 'src/queue/retry.ts', 1200);
    await stream(c, 's3', 'Retries now back off exponentially with jitter and keep delivery order. Running the webhook tests next.');
    await tool(c, 's3', 'bash', 'Run', 'npm test -- --filter webhook', 0, true);
    await c.sleep(500);
    $('[data-t=appr]').classList.add('show');
    setSt('s3', 'approval');
    await c.sleep(c.j(1700));
    await c.focus('phone');
    await c.sleep(300);
    const push = $('#push');
    push.classList.add('show');
    await c.sleep(c.j(1200));
    const fg = actors.fing;
    if (c.rm) {
      $('#pv-list').classList.remove('on');
      $('#pv-chat').classList.add('on');
      $('[data-t=pappr]').classList.add('show');
      await c.move(push, { fx: 0.5 }, fg);
      return;
    }
    await c.click(push, { fx: 0.5 }, fg);
    push.classList.remove('hov', 'show');
    $('#pv-list').classList.remove('on');
    $('#pv-chat').classList.add('on');
    await c.sleep(c.j(700));
    $('[data-t=pappr]').classList.add('show');
    await c.sleep(c.j(1200));
    await c.click($('[data-t="p-allow"]'), {}, fg);
    $('[data-t=pappr]').classList.add('sent');
    await c.sleep(c.j(900));
    await c.focus('desktop');
    await c.sleep(c.j(400));
    $('[data-t=appr]').classList.remove('show');
    const rt = $('.tool.run', COL('s3'));
    if (rt) finishTool(rt);
    setSt('s3', 'working');
    $('[data-t=pappr]').classList.remove('show');
    const pt = $('[data-t=pag]');
    const working = el('p', 'wk', 'Working');
    working.style.color = 'rgba(235,235,235,.5)';
    pt.appendChild(working);
    await c.sleep(c.j(900));
    working.remove();
    await stream(c, 's3', 'All webhook tests pass. Events are retried in order and none are dropped under the load test.');
    fin('s3', 'Worked for 41s');
    setSt('s3', 'done');
    $('[data-t=pcomp]').classList.add('idle');
    await c.sleep(3600);
  };
  const EST = [19000, 13000, 11500, 14000, 19500];
  const N = S.length;

  /* ---------- controller ---------- */
  const chipEls = $$('.chip2');
  const cap = $('#cap');
  function setChip(i) {
    chipEls.forEach((e, k) => {
      e.classList.toggle('on', k === i);
      e.setAttribute('aria-selected', k === i);
      e.style.setProperty('--p', 0);
    });
    cap.replaceChildren(el('b', '', CHIPS[i][0] + '.'), ' ' + CHIPS[i][1]);
  }
  function resetAll() {
    $('#dw').replaceChildren(...dwSaved.map((n) => n.cloneNode(true)));
    $('#ph').replaceChildren(...phSaved.map((n) => n.cloneNode(true)));
    TABS = { s1: ['s1'], s2: ['s2'], s3: ['s3'] };
    ghostOn = false;
    ghost.style.display = 'none';
    hovered = [];
    $$('.rip').forEach((r) => r.remove());
    Object.keys(actors).forEach((k) => {
      actors[k].el.style.opacity = 0;
      actors[k].shown = false;
      actors[k].el.classList.remove('dn');
    });
    stage.dataset.focus = 'desktop';
  }
  const nextFrame = () => new Promise((r) => requestAnimationFrame(() => r()));
  async function playFrom(i) {
    if (run) cancelTok(run);
    const tok = { cancelled: false, scene: i, est: 0, t0: 0 };
    run = tok;
    try {
      stage.classList.add('nomo');
      resetAll();
      for (let j = 0; j < i; j++) await S[j](mk(tok, true));
      if (RM.matches) {
        // Static mode: the final state of the chosen scene, no motion, no timers.
        await S[i](mk(tok, true));
        setChip(i);
        if (NARROW.matches && i >= 3) stage.dataset.focus = 'phone';
        return;
      }
      await nextFrame();
      stage.classList.remove('nomo');
      let k = i;
      for (;;) {
        if (k >= N) {
          await mk(tok, false).sleep(2200);
          stage.classList.add('nomo');
          resetAll();
          await nextFrame();
          stage.classList.remove('nomo');
          k = 0;
        }
        setChip(k);
        tok.scene = k;
        tok.est = EST[k];
        tok.t0 = T;
        await S[k](mk(tok, false));
        k++;
      }
    } catch (e) {
      if (e !== CANCEL) console.error(e);
    } finally {
      if (run === tok && !tok.cancelled) stage.classList.remove('nomo');
    }
  }
  chipEls.forEach((e) => e.addEventListener('click', () => playFrom(+e.dataset.i)));

  const pp = $('#pp');
  pp.addEventListener('click', () => {
    userPaused = !userPaused;
    pp.classList.toggle('paused', userPaused);
    $('span', pp).textContent = userPaused ? 'Play' : 'Pause';
    pp.setAttribute('aria-label', userPaused ? 'Play demo' : 'Pause demo');
  });
  // Pause while a mouse is over the stage. Touch taps also fire mouse events, which would leave it paused: pointer
  // events say what the pointer is.
  stage.addEventListener('pointerenter', (e) => {
    if (e.pointerType === 'mouse') hoverPaused = true;
  });
  stage.addEventListener('pointerleave', () => {
    hoverPaused = false;
  });
  let started = false;
  new IntersectionObserver(
    (es) => {
      const r = es[0].intersectionRatio;
      inView = r >= 0.4;
      if (r >= 0.55 && !started) {
        started = true;
        playFrom(0);
      }
    },
    { threshold: [0, 0.2, 0.4, 0.55, 0.8, 1] },
  ).observe(stage);
  function rmApply() {
    pp.style.display = RM.matches ? 'none' : '';
    if (RM.matches) {
      started = true;
      playFrom(0);
    }
  }
  if (RM.addEventListener)
    RM.addEventListener('change', () => {
      rmApply();
      if (!RM.matches) {
        // Left the static mode: start now when the stage is visible, else when it scrolls into view.
        if (inView) playFrom(0);
        else started = false;
      }
    });
  rmApply();
}
