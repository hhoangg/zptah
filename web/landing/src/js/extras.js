// The small interactive parts outside the hero demo: provider flyout, OS chooser, copy buttons.
import { $, $$ } from './dom.js';

/** Selects a provider in a model flyout (the rail icon and its model list). */
export function pickRail(root, prov) {
  $$('.ri', root).forEach((r) => r.classList.toggle('on', r.dataset.p === prov));
  $$('.mlist', root).forEach((m) => m.classList.toggle('on', m.dataset.p === prov));
}

/** Providers section: hover or click a rail icon; it cycles by itself while in view and untouched. */
export function initProviders(reducedMotion) {
  const fly = $('#provfly');
  if (!fly) return;
  const ids = $$('.ri', fly).filter((r) => r.dataset.p).map((r) => r.dataset.p);
  let idx = 1;
  let userTouched = false;
  let visible = false;
  $$('.ri', fly).forEach((r) => {
    if (!r.dataset.p) return;
    ['mouseenter', 'click'].forEach((ev) =>
      r.addEventListener(ev, () => {
        userTouched = true;
        pickRail(fly, r.dataset.p);
      }),
    );
  });
  new IntersectionObserver(
    (entries) => {
      visible = entries[0].isIntersecting;
    },
    { threshold: 0.4 },
  ).observe(fly);
  setInterval(() => {
    if (visible && !userTouched && !document.hidden && !reducedMotion.matches) {
      pickRail(fly, ids[idx % ids.length]);
      idx++;
    }
  }, 1700);
  fly.addEventListener('mouseleave', () => {
    userTouched = false;
  });
}

/** Install section: macOS, Linux and Windows tabs, preselected from the platform. */
export function initInstall() {
  const os = $$('.os');
  const panels = $$('.opanel');
  const pick = (name) => {
    os.forEach((b) => b.classList.toggle('on', b.dataset.os === name));
    panels.forEach((p) => p.classList.toggle('on', p.dataset.os === name));
  };
  os.forEach((b) => b.addEventListener('click', () => pick(b.dataset.os)));
  const platform = (navigator.platform || '') + (navigator.userAgent || '');
  if (/Win/.test(platform)) pick('win');
  else if (/Linux/.test(platform) && !/Android/.test(platform)) pick('linux');
}

/** Copy buttons: put `data-copy` on the clipboard and say so for a moment. */
export function initCopy() {
  $$('.copy').forEach((b) => {
    b.addEventListener('click', () => {
      const label = $('span', b);
      const done = () => {
        label.textContent = 'Copied';
        setTimeout(() => {
          label.textContent = 'Copy';
        }, 1600);
      };
      if (navigator.clipboard) navigator.clipboard.writeText(b.dataset.copy).then(done, done);
      else done();
    });
  });
}
