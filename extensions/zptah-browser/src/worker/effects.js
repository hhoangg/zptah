// In-page effects (spec 10, 10a): the Mono's cursor while it acts, and the glow with the Stop
// button for as long as it controls a tab. All of it is best effort and never fails an action.
//
// The overlay of a tab is shown from the Mono's first action on it until the run ends (task.end)
// and for as long as the debugger is attached. The cursor needs three switches (the extension's
// global one, the site not on the off list, the Mono's own) and the overlay needs the site not
// on the off list. Neither is ever drawn on restricted pages, challenge frames, or tabs the
// Mono does not own, and the overlay only in the top frame.
import { runInFrames } from "./inject.js";
import { checkUrl, isChallengeFrame, isRestricted } from "./origins.js";
import { loadPrefs, siteIsOff } from "./prefs.js";

/** An action in the last minute keeps the Mono "active" for the badge (spec 11.2). */
export const ACTIVE_MS = 60_000;
/** The glow pulses this long after an action. */
const PULSE_MS = 3000;
/** No action and no run signal for this long (no debugger either): the overlay goes. */
export const IDLE_MS = 5 * 60_000;
const MOVE_WAIT_MS = 270;

/**
 * @typedef {import("./origins.js").RosterEntry} Entry
 * @typedef {{ mono: string, entry: Entry, first: number, last: number, runOver: boolean }} TabState
 */

/**
 * @param {object} deps
 * @param {any} deps.api chrome
 * @param {() => number} deps.now
 * @param {(ms: number) => Promise<void>} deps.sleep
 * @param {(tab: number) => string | null} deps.ownerOf
 * @param {{ get(key: string): Promise<Record<string, unknown>> }} deps.area
 * @param {(tab: number) => boolean} deps.debuggerAttached
 * @param {() => string} [deps.lang]
 * @param {() => void} [deps.onActivity] the badge count may have changed
 */
export function createEffects(deps) {
  const { api, now, sleep } = deps;
  /** @type {Map<number, TabState>} */
  const tabs = new Map();
  /** Last action of a Mono, whatever the tab. @type {Map<string, number>} */
  const lastAction = new Map();
  /** @type {{ at: number, prefs: import("./prefs.js").Prefs } | null} */
  let cached = null;

  async function prefs() {
    const t = now();
    if (cached && t - cached.at < 1000) return cached.prefs;
    const prefs = await loadPrefs(deps.area);
    cached = { at: t, prefs };
    return prefs;
  }
  api.storage?.onChanged?.addListener?.(() => {
    cached = null;
  });

  const lang = () => ((deps.lang?.() ?? "en").toLowerCase().startsWith("vi") ? "vi" : "en");

  /** Where in-page effects may be drawn at all. @param {Entry} entry @param {string} url */
  function pageOk(entry, url) {
    if (isRestricted(url) || isChallengeFrame(url)) return false;
    return /^https?:/i.test(url) && checkUrl(entry, url) === "ok";
  }

  /** @param {number} tab @param {string} op @param {unknown} args @param {number} [frameId] */
  async function inFrame(tab, op, args, frameId = 0) {
    try {
      const out = await runInFrames(api, tab, [frameId], op, args);
      return out.find((o) => o.frameId === frameId)?.result;
    } catch {
      return undefined;
    }
  }

  /** @param {number} tab @param {TabState} st */
  async function showOverlay(tab, st) {
    const url = await tabUrl(tab);
    if (!url || !pageOk(st.entry, url)) return false;
    const p = await prefs();
    if (siteIsOff(p.off_sites, url)) return false;
    await inFrame(tab, "overlay_show", {
      color: st.entry.color ?? "#5f6368",
      emoji: st.entry.emoji ?? "",
      name: st.entry.name ?? "",
      lang: lang(),
      pulse: now() - st.last < PULSE_MS,
    });
    return true;
  }

  /** @param {number} tab */
  async function tabUrl(tab) {
    try {
      const t = await api.tabs.get(tab);
      return String(t?.url ?? t?.pendingUrl ?? "");
    } catch {
      return "";
    }
  }

  /**
   * Before an action runs: the overlay is up, and the cursor moves to the target first.
   * @param {Entry} entry @param {{ tab: number, frameId: number, id: string, action: any }} rec
   * @param {string} key the prepare's element key
   */
  async function beforeAct(entry, rec, key) {
    try {
      const t = now();
      const st = touch(rec.tab, entry, t);
      await showOverlay(rec.tab, st);
      if (entry.cursor !== true) return;
      const url = await tabUrl(rec.tab);
      const p = await prefs();
      if (!p.cursor || !url || !pageOk(entry, url) || siteIsOff(p.off_sites, url)) return;
      if (rec.action?.ref === undefined && rec.action?.from === undefined) return; // nothing to point at
      const pt = await inFrame(rec.tab, "act_point", { token: rec.id, key }, rec.frameId);
      if (!pt || typeof pt.x !== "number" || typeof pt.y !== "number") return;
      const shown = await inFrame(
        rec.tab,
        "cursor_move",
        {
          x: pt.x,
          y: pt.y,
          color: entry.color ?? "#5f6368",
          emoji: entry.emoji ?? "",
          name: entry.name ?? "",
          kind: String(rec.action.kind),
          // The element's accessible name only; typed text is never shown.
          target: rec.action.kind === "type" || rec.action.kind === "fill" ? undefined : pt.name,
          lang: lang(),
          click: rec.action.kind === "click",
        },
        rec.frameId,
      );
      if (shown?.shown) await sleep(MOVE_WAIT_MS);
    } catch {
      // Effects never fail an action.
    }
  }

  /** @param {number} tab @param {Entry} entry @param {number} t @returns {TabState} */
  function touch(tab, entry, t) {
    let st = tabs.get(tab);
    if (!st || st.mono !== entry.id) {
      st = { mono: entry.id, entry, first: t, last: t, runOver: false };
      tabs.set(tab, st);
    }
    st.entry = entry;
    st.last = t;
    st.runOver = false;
    lastAction.set(entry.id, t);
    deps.onActivity?.();
    return st;
  }

  /** The Mono's run ended: its overlays go, except where the debugger is still attached. @param {string} mono */
  async function taskEnd(mono) {
    for (const [tab, st] of [...tabs]) {
      if (st.mono !== mono) continue;
      st.runOver = true;
      if (!deps.debuggerAttached(tab)) await clear(tab);
    }
    deps.onActivity?.();
  }

  /** Removes the cursor and the overlay of a tab. @param {number} tab */
  async function clear(tab) {
    tabs.delete(tab);
    await inFrame(tab, "cursor_hide", {});
    await inFrame(tab, "overlay_hide", {});
    deps.onActivity?.();
  }

  async function clearAll() {
    for (const tab of [...tabs.keys()]) await clear(tab);
  }

  /** The main frame loaded a new document: the overlay is drawn again. @param {number} tab */
  async function reapply(tab) {
    const st = tabs.get(tab);
    if (!st || deps.ownerOf(tab) !== st.mono) return;
    if (st.runOver && !deps.debuggerAttached(tab)) return;
    await showOverlay(tab, st);
  }

  /** The tab left the Mono's hands or its origins. @param {number} tab */
  function forget(tab) {
    tabs.delete(tab);
    deps.onActivity?.();
  }

  /** Hides the effects of a tab while `fn` takes a picture of it. @template T @param {number} tab @param {() => Promise<T>} fn */
  async function around(tab, fn) {
    if (!tabs.has(tab)) return fn();
    await inFrame(tab, "effects_suspend", { on: true });
    try {
      return await fn();
    } finally {
      await inFrame(tab, "effects_suspend", { on: false });
    }
  }

  /** From the keepalive tick: idle overlays go (a stuck run must not leave a glow forever). */
  async function sweep() {
    const t = now();
    for (const [tab, st] of [...tabs]) {
      if (deps.debuggerAttached(tab)) continue;
      if (deps.ownerOf(tab) !== st.mono || t - st.last > IDLE_MS) await clear(tab);
    }
  }

  /** Monos with a run in progress or an action in the last minute. */
  function activeMonos() {
    const t = now();
    const ids = new Set();
    for (const st of tabs.values()) if (!st.runOver) ids.add(st.mono);
    for (const [mono, at] of lastAction) if (t - at < ACTIVE_MS) ids.add(mono);
    return ids.size;
  }

  return { beforeAct, taskEnd, clear, clearAll, reapply, forget, around, sweep, activeMonos, has: (/** @type {number} */ tab) => tabs.has(tab), states: () => tabs };
}
