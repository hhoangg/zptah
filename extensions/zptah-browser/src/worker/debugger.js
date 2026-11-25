// The debugger fallback (spec 6.3). One wrapper, one allowlist: the extension never forwards a CDP
// method from the app, and every command goes through `send`, which refuses anything not listed
// before chrome.debugger is called. The session is attached once per tab and task, pre-attached
// before destructive and payment actions (decision 6), and detached on Stop, on tab loss, when
// the tab leaves the Mono's origins and 2 minutes after the last use (decision 4).
import { ZptahError } from "./errors.js";

export const IDLE_DETACH_MS = 120000;
export const SETTLE_MS = 400;
const CDP_VERSION = "1.3";

/** CDP methods the extension may call, with a check of what they may carry. @type {Map<string, (p: any) => boolean>} */
const ALLOWED = new Map(/** @type {[string, (p: any) => boolean][]} */ ([
  ["Input.dispatchMouseEvent", () => true],
  ["Input.dispatchKeyEvent", () => true],
  ["Input.insertText", (p) => typeof p?.text === "string"],
  ["Input.dispatchDragEvent", () => true],
  ["Page.captureScreenshot", () => true],
  ["Page.enable", () => true],
  ["Page.handleJavaScriptDialog", (p) => typeof p?.accept === "boolean"],
  ["Emulation.setFocusEmulationEnabled", (p) => typeof p?.enabled === "boolean"],
]));

/** @param {string} method */
export const isAllowedCdp = (method) => ALLOWED.has(method);

/**
 * @typedef {object} DebuggerDeps
 * @property {any} api the chrome object (or a fake)
 * @property {() => number} now
 * @property {(ms: number) => Promise<void>} sleep
 * @property {(fn: () => void, ms: number) => unknown} [setTimer]
 * @property {(id: unknown) => void} [clearTimer]
 * @property {(method: string, params: Record<string, unknown>) => void} emit sends an event to the app
 */

/**
 * @typedef {object} TabState
 * @property {boolean} attached
 * @property {boolean} declined the user pressed Cancel on the infobar: no re-attach for the task
 * @property {boolean} sticky once the debugger is used on a tab it is used for the rest of the task
 * @property {number} last last use
 * @property {unknown} timer
 * @property {{ kind: string, message: string } | null} dialog a JavaScript dialog the debugger saw open
 */

/** @param {DebuggerDeps} deps */
export function createDebugger(deps) {
  const { api, now, sleep, emit } = deps;
  const setTimer = deps.setTimer ?? setTimeout;
  const clearTimer = deps.clearTimer ?? /** @type {(id: unknown) => void} */ (clearTimeout);
  /** @type {Map<number, TabState>} */
  const tabs = new Map();

  /** @param {number} tab @returns {TabState} */
  const state = (tab) => {
    let s = tabs.get(tab);
    if (!s) {
      s = { attached: false, declined: false, sticky: false, last: now(), timer: null, dialog: null };
      tabs.set(tab, s);
    }
    return s;
  };

  /** @param {number} tab */
  function armTimer(tab) {
    const s = state(tab);
    if (s.timer !== null) clearTimer(s.timer);
    s.timer = setTimer(() => {
      s.timer = null;
      void sweep();
    }, IDLE_DETACH_MS + 50);
  }

  /** Counts as use of the tab in the task: keeps the session and the declined mark alive. @param {number} tab */
  function touch(tab) {
    const s = state(tab);
    s.last = now();
    if (s.attached || s.declined) armTimer(tab);
  }

  /** @param {number} tab */
  const isAttached = (tab) => tabs.get(tab)?.attached === true;
  /** @param {number} tab */
  const isSticky = (tab) => tabs.get(tab)?.sticky === true;
  /** @param {number} tab */
  const openDialog = (tab) => tabs.get(tab)?.dialog ?? null;
  const attachedTabs = () => [...tabs].filter(([, s]) => s.attached).map(([t]) => t).sort((a, b) => a - b);

  /** @returns {Promise<boolean>} */
  async function permitted() {
    try {
      return (await api.permissions.contains({ permissions: ["debugger"] })) === true;
    } catch {
      return false;
    }
  }

  /**
   * Attaches once per tab; later calls only count as use. Waits for the infobar to settle before
   * returning, so geometry is measured on the final layout.
   * @param {number} tab
   */
  async function attach(tab) {
    const s = state(tab);
    if (s.declined) throw new ZptahError("E_DEBUGGER_DENIED", "You cancelled the debugging notice on this tab, so zptah Browser will not attach again in this task.");
    if (s.attached) {
      touch(tab);
      return false;
    }
    if (!(await permitted())) {
      throw new ZptahError("E_DEBUGGER_DENIED", "This needs the optional debugger permission. Allow it in the zptah Browser options page.");
    }
    try {
      await api.debugger.attach({ tabId: tab }, CDP_VERSION);
    } catch (err) {
      const m = String(err instanceof Error ? err.message : err);
      if (/another debugger|already attached|devtools/i.test(m)) throw new ZptahError("E_DEBUGGER_BUSY", "Another debugger (such as DevTools) is attached to this tab.");
      if (/no tab|cannot attach|cannot access/i.test(m)) throw new ZptahError("E_TAB_GONE", "The tab is gone or cannot be debugged.");
      throw new ZptahError("E_DEBUGGER_DENIED", m.slice(0, 200));
    }
    s.attached = true;
    s.sticky = true;
    s.last = now();
    armTimer(tab);
    await sendRaw(tab, "Page.enable", {});
    await sleep(SETTLE_MS);
    return true;
  }

  /**
   * @param {number} tab @param {string} method @param {Record<string, unknown>} params
   */
  async function sendRaw(tab, method, params) {
    const check = ALLOWED.get(method);
    if (!check || !check(params)) throw new ZptahError("E_DEBUGGER_DENIED", `The debugger command ${method.slice(0, 60)} is not allowed.`);
    return api.debugger.sendCommand({ tabId: tab }, method, params);
  }

  /**
   * The only way to call chrome.debugger.sendCommand. Refuses everything off the allowlist
   * before the call.
   * @param {number} tab @param {string} method @param {Record<string, unknown>} [params]
   */
  async function send(tab, method, params = {}) {
    if (!isAttached(tab)) throw new ZptahError("E_DEBUGGER_DENIED", "The debugger is not attached to this tab.");
    touch(tab);
    try {
      return await sendRaw(tab, method, params);
    } catch (err) {
      if (err instanceof ZptahError) throw err;
      const m = String(err instanceof Error ? err.message : err);
      if (/detached|no target|not attached/i.test(m)) {
        markDetached(tab, "detached");
        throw new ZptahError("E_DEBUGGER_DENIED", "The debugger was detached from this tab.");
      }
      throw new ZptahError("E_INVALID", m.slice(0, 200));
    }
  }

  /** @param {number} tab @param {string} reason */
  function markDetached(tab, reason) {
    const s = tabs.get(tab);
    if (!s?.attached) return;
    s.attached = false;
    s.sticky = false;
    s.dialog = null;
    if (s.timer !== null) clearTimer(s.timer);
    s.timer = null;
    emit("debugger.detached", { tab, reason: reason.slice(0, 128) });
  }

  /**
   * @param {number} tab @param {string} reason
   */
  async function detach(tab, reason) {
    const s = tabs.get(tab);
    if (!s?.attached) return;
    markDetached(tab, reason);
    try {
      await api.debugger.detach({ tabId: tab });
    } catch {
      // already gone
    }
  }

  /** @param {string} reason */
  async function detachAll(reason) {
    for (const tab of attachedTabs()) await detach(tab, reason);
    for (const s of tabs.values()) {
      s.sticky = false;
      s.declined = false;
    }
  }

  /** Detaches sessions idle for 2 minutes and forgets old marks (the task is over). */
  async function sweep() {
    const t = now();
    for (const [tab, s] of [...tabs]) {
      if (t - s.last < IDLE_DETACH_MS) continue;
      if (s.attached) await detach(tab, "task_ended");
      if (s.timer !== null) clearTimer(s.timer);
      tabs.delete(tab);
    }
  }

  /** @param {number} tab */
  function forget(tab) {
    const s = tabs.get(tab);
    if (s?.timer != null) clearTimer(s.timer);
    tabs.delete(tab);
  }

  /** chrome.debugger.onDetach. @param {{ tabId?: number }} source @param {string} reason */
  function onDetach(source, reason) {
    if (typeof source?.tabId !== "number") return;
    const s = tabs.get(source.tabId);
    if (!s?.attached) return;
    markDetachedWith(source.tabId, reason);
  }

  /** @param {number} tab @param {string} reason */
  function markDetachedWith(tab, reason) {
    const s = state(tab);
    if (reason === "canceled_by_user") s.declined = true;
    markDetached(tab, reason);
    if (s.declined) armTimer(tab);
  }

  /** chrome.debugger.onEvent. @param {{ tabId?: number }} source @param {string} method @param {any} params */
  function onEvent(source, method, params) {
    if (typeof source?.tabId !== "number") return;
    const s = tabs.get(source.tabId);
    if (!s?.attached) return;
    if (method === "Page.javascriptDialogOpening") {
      const kind = String(params?.type ?? "alert").slice(0, 32);
      const message = String(params?.message ?? "").slice(0, 65536);
      s.dialog = { kind, message };
      emit("dialog.opened", { tab: source.tabId, kind, message });
    } else if (method === "Page.javascriptDialogClosed") {
      s.dialog = null;
    }
  }

  return { attach, send, detach, detachAll, sweep, touch, forget, isAttached, isSticky, openDialog, attachedTabs, permitted, onDetach, onEvent, tabs };
}
