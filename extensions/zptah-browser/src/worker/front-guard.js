// Gives the user's front tab and window back when something the Mono opened took the front
// (spec 6.5 rule 1 exception, 9.4). Chrome fronts a `target=_blank` popup or a popup window even
// when the tab that opened it is in the background; the extension never fronts anything itself,
// except here: within 2 s of a Mono's tab opening a new tab or window it restores the tab and
// window the user had in front, once. This is the only file allowed to set `active` or `focused`
// (scripts/scan.mjs lists it by name).

/** How long after an opening the front is given back. */
export const RESTORE_WINDOW_MS = 2000;

/**
 * @param {object} deps
 * @param {any} deps.api
 * @param {() => number} deps.now
 */
export function createFrontGuard(deps) {
  const { api, now } = deps;
  /** What the user has in front right now, as far as events told. @type {{ windowId: number, tabId: number } | null} */
  let front = null;
  /** The front when a Mono's tab opened something, until the window ends. @type {{ windowId: number, tabId: number, until: number } | null} */
  let pending = null;
  let restoring = false;

  /** Reads the current front once (the worker may start after the user has been browsing). */
  async function init() {
    try {
      const [tab] = await api.tabs.query({ active: true, lastFocusedWindow: true });
      if (tab && typeof tab.id === "number" && !front) front = { windowId: tab.windowId, tabId: tab.id };
    } catch {
      // Nothing to restore to.
    }
  }

  /** A tab of a Mono opened another tab or window: remember what the user had in front. */
  function opening() {
    if (front) pending = { ...front, until: now() + RESTORE_WINDOW_MS };
  }

  async function restore() {
    const target = pending;
    if (!target || restoring || now() > target.until) return;
    restoring = true;
    pending = null;
    try {
      // The user's own tab first, then its window, so the window shows the tab they were on.
      await api.tabs.update(target.tabId, { active: true });
      await api.windows.update(target.windowId, { focused: true });
      front = { windowId: target.windowId, tabId: target.tabId };
    } catch {
      // The tab or window closed meanwhile.
    } finally {
      restoring = false;
    }
  }

  /** @param {{ tabId: number, windowId: number }} info */
  function onActivated(info) {
    if (restoring) return;
    const was = front;
    front = { windowId: info.windowId, tabId: info.tabId };
    if (pending && was && now() <= pending.until && (info.tabId !== pending.tabId || info.windowId !== pending.windowId)) void restore();
  }

  /** @param {number} windowId -1 when no Chrome window has focus */
  function onFocusChanged(windowId) {
    if (restoring || windowId < 0) return;
    if (front) front = { ...front, windowId };
    if (pending && now() <= pending.until && windowId !== pending.windowId) void restore();
  }

  api.tabs?.onActivated?.addListener(onActivated);
  api.windows?.onFocusChanged?.addListener(onFocusChanged);
  void init();

  return { opening, init, onActivated, onFocusChanged, current: () => front };
}
