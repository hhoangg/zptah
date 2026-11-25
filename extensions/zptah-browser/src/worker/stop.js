// The overlay's Stop for one tab (spec 10a): ends that Mono's work on that tab and gives the tab
// back to the user. The toolbar icon stays the stop for everything (act.js stopAll).
//
// States of a tab: running (owned, maybe with an overlay) -> stopping (this module is ending it,
// a second click joins the same stop) -> taken back (the user's tab, marked in ownership). The
// mark ends when the user drags the tab back into the group or closes it (ownership.js).

/**
 * @param {object} deps
 * @param {(tab: number) => string | null} deps.ownerOf the Mono that owns the tab
 * @param {(tab: number) => Promise<void>} deps.stopTab cancels the tab's prepares, detaches the debugger
 * @param {(tab: number) => Promise<void>} deps.clearEffects removes the cursor and the overlay
 * @param {(tab: number) => Promise<unknown>} deps.takeBack the tab becomes the user's (marked, ungrouped)
 * @param {(method: string, params: Record<string, unknown>) => unknown} deps.emit event to the app; returns false when the link is not ready
 * @param {{ get(key: string): Promise<Record<string, any>>, set(items: Record<string, unknown>): Promise<void> }} [deps.area] chrome.storage.session, for events the app has not received yet
 * @param {(tab: number) => void} [deps.onStopped]
 */
export function createStop(deps) {
  /** @type {Map<number, Promise<{ stopped: boolean }>>} */
  const running = new Map();
  const PENDING = "zptah.pending_stops";

  /** @returns {Promise<Record<string, unknown>[]>} */
  async function pending() {
    try {
      const v = (await deps.area?.get(PENDING))?.[PENDING];
      return Array.isArray(v) ? v : [];
    } catch {
      return [];
    }
  }

  /** Sends the stops the app has not seen (the link was down when the user pressed Stop). */
  async function flush() {
    const list = await pending();
    if (list.length === 0) return;
    const left = [];
    for (const p of list) if (deps.emit("tab.stopped", p) === false) left.push(p);
    try {
      await deps.area?.set({ [PENDING]: left });
    } catch {
      // The events were sent; a stale list is resent at most once more.
    }
  }

  /** @param {Record<string, unknown>} params */
  async function send(params) {
    if (deps.emit("tab.stopped", params) !== false) return;
    const list = await pending();
    try {
      await deps.area?.set({ [PENDING]: [...list, params].slice(-50) });
    } catch {
      // No storage: the stop still happened in the browser.
    }
  }

  /**
   * Stops the owner Mono on the tab. Does nothing when nobody owns it (already stopped, or the
   * user's tab): a double click or a late click cannot stop anything twice.
   * @param {number} tab
   */
  function stopTab(tab) {
    const joined = running.get(tab);
    if (joined) return joined;
    const mono = deps.ownerOf(tab);
    if (mono === null) return Promise.resolve({ stopped: false });
    const work = (async () => {
      // Order matters: nothing more may be dispatched before the tab changes hands.
      try {
        await deps.stopTab(tab);
      } catch {
        // Whatever could not be detached is gone with the tab or the debugger below.
      }
      try {
        await deps.clearEffects(tab);
      } catch {
        // The page may have navigated away.
      }
      await deps.takeBack(tab);
      await send({ tab, mono, by: "overlay" });
      deps.onStopped?.(tab);
      return { stopped: true };
    })().finally(() => running.delete(tab));
    running.set(tab, work);
    return work;
  }

  return { stopTab, flush, isStopping: (/** @type {number} */ tab) => running.has(tab) };
}

/**
 * Whether a runtime message is the overlay's Stop from the top frame of a tab of this extension's
 * own content script. @param {any} sender @param {string} extensionId
 * @returns {number | null} the tab id
 */
export function overlayStopSender(sender, extensionId) {
  if (!sender || sender.id !== extensionId) return null;
  if (sender.frameId !== 0) return null;
  const id = sender.tab?.id;
  return Number.isSafeInteger(id) && id >= 0 ? id : null;
}
