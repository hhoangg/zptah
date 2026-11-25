// Tab ownership (spec 9.2, 9.3): which Mono owns which tab. The extension holds the authoritative
// table because only it sees the user drag a tab. It is kept in chrome.storage.session (survives a
// worker restart, not a browser restart); a restored group comes back as a user's group and its
// tabs start as user tabs.
//
// `ownerOf` is the hook the read and act paths use. The handlers here implement `tab.open`,
// `tab.close`, `tab.adopt` and `tab.release`, and the Chrome events keep the table in step with
// the user's drags, with popups an owned tab opens and with closed tabs and groups.
import { DEFAULT_DEADLINE_MS, errorReply, okReply, withDeadline, ZptahError } from "./errors.js";
import { focusGuard } from "./focus-guard.js";
import { createGroups } from "./groups.js";
import { currentRoster, requireReadable } from "./origins.js";
import { getReadableTab, tabInfo } from "./tabs.js";

const KEY = "zptah.owned";
const HEX32 = /^[0-9a-f]{32}$/;
/**
 * A group change the worker made itself is not the user's drag when Chrome reports it within this
 * time. Chrome reports every such change, which consumes the mark; the time only bounds a mark
 * that no event ever consumed.
 */
const EXPECT_MS = 1500;

/** @typedef {{ tab: number, mono: string, since_ms: number, how: "created" | "adopted" | "user_drag" | "popup" }} Owned */

/** At least 2 words or 8 characters (spec 9.3), the same bar the gate applies. @param {string} quote */
function longEnough(quote) {
  const words = quote.match(/[\p{L}\p{N}]+/gu) ?? [];
  return words.length >= 2 || words.join("").length >= 8;
}

/**
 * @param {import("./link.js").BrowserLink | { handlers: Map<string, any>, roster: unknown, stopped?: boolean, sendEvent?: (m: string, p: any) => boolean }} link
 * @param {object} deps
 * @param {ReturnType<import("./readpath.js").registerReadPath>} deps.read
 * @param {any} [deps.api]
 * @param {() => number} [deps.now]
 * @param {{ opening(): void }} [deps.front] the front guard, told when an owned tab opens something
 */
export function registerOwnership(link, deps) {
  const api = deps.api ?? /** @type {any} */ (globalThis).chrome;
  const now = deps.now ?? Date.now;
  const { read } = deps;
  const guard = focusGuard(api);
  const area = api.storage?.session;

  /** @type {Map<number, Owned>} */
  const owned = new Map();
  /** Tabs the user took back, with the time (ms). @type {Map<number, number>} */
  const takenBack = new Map();
  /** The group each owned tab was put in. @type {Map<number, number>} */
  const groupOfTab = new Map();
  /** Tabs the worker is about to move itself. @type {Map<number, number>} */
  const expected = new Map();

  const emit = (/** @type {string} */ method, /** @type {Record<string, unknown>} */ params) => {
    try {
      /** @type {any} */ (link).sendEvent?.(method, params);
    } catch {
      // The link is down; the app reads the owners from the next tabs.list.
    }
  };

  const roster = () => currentRoster(/** @type {any} */ (link), read.slot);
  const groups = createGroups({
    api,
    roster,
    expectChange: (tab) => void expected.set(tab, now()),
  });

  // ---- the table ----

  async function persist() {
    if (!area) return;
    try {
      await area.set({
        [KEY]: {
          owned: [...owned.values()],
          takenBack: [...takenBack],
          groupOfTab: [...groupOfTab],
        },
      });
    } catch {
      // Storage full or unavailable: the table still works in memory.
    }
  }

  const ready = (async () => {
    if (!area) return;
    try {
      const stored = (await area.get(KEY))?.[KEY];
      if (!stored || typeof stored !== "object") return;
      for (const o of stored.owned ?? []) if (o && Number.isSafeInteger(o.tab) && HEX32.test(o.mono)) owned.set(o.tab, o);
      for (const [t, ms] of stored.takenBack ?? []) takenBack.set(t, ms);
      for (const [t, g] of stored.groupOfTab ?? []) groupOfTab.set(t, g);
    } catch {
      // Nothing restored: every tab is the user's, which fails closed.
    }
  })();

  /** The Mono that owns the tab, or null for the user's tab. @param {number} tab */
  const ownerOf = (tab) => owned.get(tab)?.mono ?? null;

  /** @param {number} tab @param {string} mono @param {Owned["how"]} how */
  function claim(tab, mono, how) {
    owned.set(tab, { tab, mono, since_ms: now(), how });
    takenBack.delete(tab);
    void persist();
  }

  /** The tab becomes the user's. @param {number} tab */
  function release(tab) {
    const was = owned.get(tab) ?? null;
    owned.delete(tab);
    groupOfTab.delete(tab);
    void persist();
    return was;
  }

  /** @param {number} tab */
  function takeBack(tab) {
    const was = release(tab);
    if (was) takenBack.set(tab, now());
    void persist();
    return was;
  }

  /** Every tab of a Mono becomes the user's and leaves its group. @param {string} mono */
  async function releaseMono(mono) {
    const tabs = [...owned.values()].filter((o) => o.mono === mono).map((o) => o.tab);
    for (const tab of tabs) {
      release(tab);
      await groups.ungroup(tab);
    }
    groups.forgetMono(mono);
    return tabs;
  }

  // ---- the roster ----

  /**
   * Called when the roster changed or a link came up: groups take the current title and color,
   * and the tabs of a Mono that left the roster go back to the user.
   */
  async function syncRoster() {
    await ready;
    const ids = new Set(roster().map((m) => m.id));
    for (const mono of new Set([...owned.values()].map((o) => o.mono))) {
      if (!ids.has(mono)) await releaseMono(mono);
    }
    await groups.refresh();
  }

  // ---- helpers for the handlers ----

  /** @param {Record<string, any>} msg */
  function entryFor(msg) {
    if (/** @type {any} */ (link).stopped) throw new ZptahError("E_STOPPED", "zptah Browser is stopped.");
    if (typeof msg.mono !== "string" || !HEX32.test(msg.mono)) throw new ZptahError("E_INVALID", "mono is required");
    const entry = roster().find((m) => m.id === msg.mono);
    if (!entry) throw new ZptahError("E_UNKNOWN_MONO", "This Mono is not known to the browser.");
    return entry;
  }

  /**
   * @param {string} method
   * @param {(entry: any, params: any) => Promise<Record<string, unknown>>} fn
   */
  function route(method, fn) {
    link.handlers.set(method, async (/** @type {Record<string, any>} */ msg) => {
      const id = String(msg.id);
      try {
        const entry = entryFor(msg);
        const params = /** @type {any} */ (msg.params ?? {});
        const deadline = Number.isInteger(params.deadline_ms) ? params.deadline_ms : DEFAULT_DEADLINE_MS;
        await ready;
        return okReply(id, await withDeadline(deadline, () => fn(entry, params)));
      } catch (err) {
        return errorReply(id, err);
      }
    });
  }

  /** @param {any} entry @param {number} tabId the tab's info, with its owner and group title */
  async function infoOf(entry, tabId) {
    const tab = await api.tabs.get(tabId);
    const owner = ownerOf(tabId);
    const info = tabInfo(tab, owner);
    const mono = owner ? roster().find((m) => m.id === owner) : null;
    return mono ? { ...info, group_title: groups.titleOf(mono) } : info;
  }

  /** Writing to the Mono's own tabs needs act level. @param {any} entry @param {string} what */
  function needAct(entry, what) {
    if (entry.level !== "act") throw new ZptahError("E_UNSUPPORTED", `This Mono can only read the browser, so it cannot ${what}.`);
  }

  /** @param {any} entry @param {unknown} id */
  function ownTab(entry, id) {
    if (!Number.isSafeInteger(id) || /** @type {number} */ (id) < 0) throw new ZptahError("E_INVALID", "tab must be a tab id");
    const tab = /** @type {number} */ (id);
    if (ownerOf(tab) !== entry.id) throw new ZptahError("E_NOT_OWNER", "That tab is not this Mono's. Only a Mono's own tabs can be closed or released.");
    return tab;
  }

  /**
   * The window a new tab goes to: the one holding `after`, else the user's last focused normal
   * window. The extension never creates a window.
   * @param {any} after
   */
  async function windowFor(after) {
    if (after) {
      const win = await api.windows.get(after.windowId).catch(() => null);
      if (win && !win.incognito) return win;
    }
    const win = await api.windows.getLastFocused({ windowTypes: ["normal"] }).catch(() => null);
    if (!win || typeof win.id !== "number") throw new ZptahError("E_UNSUPPORTED", "There is no open browser window to put the tab in.");
    if (win.incognito) throw new ZptahError("E_INCOGNITO", "zptah Browser does not work in incognito windows.");
    return win;
  }

  // ---- tab.open / close / adopt / release ----

  route("tab.open", async (entry, p) => {
    if (typeof p.url !== "string" || p.url === "") throw new ZptahError("E_INVALID", "url is required");
    let url;
    try {
      url = new URL(p.url);
    } catch {
      throw new ZptahError("E_INVALID", "url is not an address");
    }
    if (url.protocol !== "http:" && url.protocol !== "https:") throw new ZptahError("E_INVALID", "Only http and https addresses can be opened.");
    requireReadable(entry, url.href);
    let after = null;
    if (Number.isSafeInteger(p.after_tab) && ownerOf(p.after_tab) === entry.id) {
      after = await api.tabs.get(p.after_tab).catch(() => null);
    }
    const win = await windowFor(after);
    /** @type {Record<string, unknown>} */
    const props = { url: url.href, windowId: win.id };
    if (after && after.windowId === win.id) props.index = after.index + 1;
    // The focus guard opens it in the background.
    const created = await guard.tabs.create(props);
    claim(created.id, entry.id, "created");
    const groupId = await groups.place(created.id, entry);
    if (groupId !== null) {
      groupOfTab.set(created.id, groupId);
      void persist();
    }
    return infoOf(entry, created.id);
  });

  route("tab.close", async (entry, p) => {
    needAct(entry, "close tabs");
    const tab = ownTab(entry, p.tab);
    release(tab);
    try {
      await api.tabs.remove(tab);
    } catch {
      throw new ZptahError("E_TAB_GONE", "The tab is gone.");
    }
    return {};
  });

  route("tab.release", async (entry, p) => {
    needAct(entry, "release tabs");
    const tab = ownTab(entry, p.tab);
    release(tab);
    await groups.ungroup(tab);
    return {};
  });

  route("tab.adopt", async (entry, p) => {
    needAct(entry, "adopt tabs");
    const tab = await getReadableTab(api, entry, p.tab);
    const grant = p.grant;
    const owner = ownerOf(tab.id);
    if (owner === entry.id) return infoOf(entry, tab.id);
    if (owner === null) {
      // A tab of the user's: only words the user wrote, and after the stop when they took it back.
      if (!grant || grant.kind !== "user_reference" || typeof grant.quote !== "string" || !Number.isSafeInteger(grant.message_ms)) {
        throw new ZptahError("E_NOT_OWNER", "Adopting a tab of the user's needs the words the user wrote that name it.");
      }
      if (!longEnough(grant.quote)) throw new ZptahError("E_NOT_OWNER", "The user's reference is too short.");
      const stop = takenBack.get(tab.id);
      if (stop !== undefined && grant.message_ms <= stop) {
        throw new ZptahError("E_NOT_OWNER", "The user took this tab back. Only words they wrote after that count, or they can drag it back.");
      }
    } else if (!grant || grant.kind !== "user_approved" || typeof grant.approval_id !== "string" || grant.approval_id === "") {
      // A tab another Mono owns moves only with the user's yes.
      throw new ZptahError("E_NOT_OWNER", "That tab belongs to another Mono. Taking it over needs the user's approval.");
    }
    claim(tab.id, entry.id, "adopted");
    const groupId = await groups.place(tab.id, entry);
    if (groupId !== null) {
      groupOfTab.set(tab.id, groupId);
      void persist();
    }
    return infoOf(entry, tab.id);
  });

  // ---- Chrome events ----

  /** @param {number} tabId */
  function isExpected(tabId) {
    const at = expected.get(tabId);
    if (at === undefined) return false;
    expected.delete(tabId);
    return now() - at < EXPECT_MS;
  }

  /** A tab's group changed: only the user's own drags get here. @param {number} tabId @param {number} groupId */
  async function groupChanged(tabId, groupId) {
    await ready;
    if (isExpected(tabId)) return;
    const owner = ownerOf(tabId);
    const mono = groupId >= 0 ? groups.monoOf(groupId) : null;
    if (mono !== null) {
      if (owner === mono) return;
      // Dragged into a Mono's group: adopted by the user, no prompt (9.3).
      claim(tabId, mono, "user_drag");
      groupOfTab.set(tabId, groupId);
      emit("tab.ownership", { tab: tabId, owner: mono, by: "user_drag" });
      return;
    }
    if (owner !== null) {
      // Dragged out, or ungrouped: the user took it back.
      takeBack(tabId);
      emit("tab.ownership", { tab: tabId, owner: null, by: groupId === -1 ? "user_ungroup" : "user_drag" });
    }
  }

  /** A tab an owned tab opened is the same Mono's (9.3). @param {number} tabId @param {number | undefined} sourceTab */
  async function opened(tabId, sourceTab) {
    if (typeof sourceTab !== "number") return;
    await ready;
    const mono = ownerOf(sourceTab);
    if (mono === null || ownerOf(tabId) === mono) return;
    deps.front?.opening();
    claim(tabId, mono, "popup");
    const entry = roster().find((m) => m.id === mono);
    if (entry) {
      const groupId = await groups.place(tabId, entry).catch(() => null);
      if (groupId !== null) groupOfTab.set(tabId, groupId);
      void persist();
    }
    emit("tab.ownership", { tab: tabId, owner: mono, by: "popup" });
  }

  api.tabs?.onUpdated?.addListener((/** @type {number} */ tabId, /** @type {any} */ change) => {
    if (change && typeof change.groupId === "number") void groupChanged(tabId, change.groupId);
  });
  api.tabs?.onCreated?.addListener((/** @type {any} */ tab) => void opened(tab.id, tab.openerTabId));
  api.webNavigation?.onCreatedNavigationTarget?.addListener((/** @type {any} */ d) => void opened(d.tabId, d.sourceTabId));
  api.tabs?.onRemoved?.addListener((/** @type {number} */ tabId) => {
    release(tabId);
    takenBack.delete(tabId);
    expected.delete(tabId);
  });
  api.tabGroups?.onRemoved?.addListener(
    (/** @type {any} */ group) =>
      void (async () => {
        await ready;
        const mono = groups.forget(group.id);
        if (mono === null) return;
        for (const [tab, g] of [...groupOfTab]) {
          if (g !== group.id || ownerOf(tab) !== mono) continue;
          release(tab);
          emit("tab.ownership", { tab, owner: null, by: "group_closed" });
        }
      })(),
  );

  /** @param {string} mono */
  const titleOfMono = (mono) => {
    const entry = roster().find((m) => m.id === mono);
    return entry ? groups.titleOf(entry) : undefined;
  };

  return { ownerOf, titleOfMono, claim, release, takeBack, releaseMono, syncRoster, groups, ready, snapshot: () => [...owned.values()], takenBackAt: (/** @type {number} */ tab) => takenBack.get(tab) ?? null };
}
