// Gives the user's front tab and window back when a popup a Mono's tab opened took the front
// (spec 6.5 rule 1 exception, 9.4). Chrome fronts a `target=_blank` popup or a popup window even
// when the tab that opened it is in the background. The extension never fronts anything itself,
// except here: within 2 s of a Mono's tab opening a popup, when the tab or window that just became
// the front IS that popup, it restores the front the user had before the popup, once. A switch the
// user makes by hand is never undone. This is the only file allowed to set `active` or `focused`
// (scripts/scan.mjs lists it by name, for those two properties only).

/** How long after an opening the front is given back. */
export const RESTORE_WINDOW_MS = 2000;
/** How many recent fronts are remembered. */
const HISTORY = 12;

/** @typedef {{ windowId: number, tabId: number }} Front */

/**
 * @param {object} deps
 * @param {any} deps.api
 * @param {() => number} deps.now
 * @param {(tab: number) => string | null} [deps.ownerOf] a tab a Mono owns is never a place to go back to
 */
export function createFrontGuard(deps) {
  const { api, now } = deps;
  const ownerOf = deps.ownerOf ?? (() => null);
  /** Recent fronts, oldest first. @type {(Front & { at: number })[]} */
  const history = [];
  /** The active tab of each window, as far as events told. @type {Map<number, number>} */
  const activeIn = new Map();
  /** @type {Front | null} */
  let front = null;
  /** @type {{ tabId: number, windowId: number | null, until: number, target: Front | null } | null} */
  let pending = null;
  let restoring = false;

  /** @param {Front} f */
  function remember(f) {
    front = f;
    activeIn.set(f.windowId, f.tabId);
    if (history.at(-1)?.tabId !== f.tabId || history.at(-1)?.windowId !== f.windowId) history.push({ ...f, at: now() });
    if (history.length > HISTORY) history.shift();
  }

  async function init() {
    try {
      for (const tab of await api.tabs.query({ active: true })) {
        if (typeof tab.id === "number") activeIn.set(tab.windowId, tab.id);
      }
      const [tab] = await api.tabs.query({ active: true, lastFocusedWindow: true });
      if (tab && typeof tab.id === "number" && !front) remember({ windowId: tab.windowId, tabId: tab.id });
    } catch {
      // Nothing to restore to.
    }
  }

  /**
   * The front the user had before the popup: the latest remembered front that is neither the popup
   * nor a tab a Mono owns.
   * @param {number} popupTab
   * @returns {Front | null}
   */
  function before(popupTab) {
    for (let i = history.length - 1; i >= 0; i -= 1) {
      const h = history[i];
      if (h.tabId !== popupTab && ownerOf(h.tabId) === null) return { windowId: h.windowId, tabId: h.tabId };
    }
    return null;
  }

  async function restore() {
    const target = pending?.target;
    pending = null;
    if (!target || restoring) return;
    restoring = true;
    try {
      // The user's own tab first, then its window, so the window shows the tab they were on.
      await api.tabs.update(target.tabId, { active: true });
      await api.windows.update(target.windowId, { focused: true });
      remember(target);
    } catch {
      // The tab or window closed meanwhile.
    } finally {
      restoring = false;
    }
  }

  /** @param {Front} now_ */
  function popupIsFront(now_) {
    if (!pending || now() > pending.until) return false;
    return now_.tabId === pending.tabId || (pending.windowId !== null && now_.windowId === pending.windowId && activeIn.get(now_.windowId) === pending.tabId);
  }

  /** @param {{ tabId: number, windowId: number }} info */
  function onActivated(info) {
    if (restoring) return;
    const f = { windowId: info.windowId, tabId: info.tabId };
    remember(f);
    if (popupIsFront(f)) void restore();
  }

  /** @param {number} windowId -1 when no Chrome window has focus */
  function onFocusChanged(windowId) {
    if (restoring || windowId < 0) return;
    const tabId = activeIn.get(windowId);
    if (tabId === undefined) return;
    const f = { windowId, tabId };
    remember(f);
    if (popupIsFront(f)) void restore();
  }

  api.tabs?.onActivated?.addListener(onActivated);
  api.windows?.onFocusChanged?.addListener(onFocusChanged);
  void init();

  /**
   * A Mono's tab opened `popupTab`. The front the user had is the latest one before it, whether
   * Chrome has fronted the popup already or not yet.
   * @param {number} popupTab
   */
  async function opening(popupTab) {
    const target = before(popupTab);
    /** @type {number | null} */
    let windowId = null;
    try {
      windowId = (await api.tabs.get(popupTab)).windowId;
    } catch {
      return;
    }
    pending = { tabId: popupTab, windowId, until: now() + RESTORE_WINDOW_MS, target };
    // The popup may already be in front (its activation came first).
    if (front && popupIsFront(front)) await restore();
  }

  return { opening, current: () => front };
}
