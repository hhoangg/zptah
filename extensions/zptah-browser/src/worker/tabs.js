// tabs.list and the tab checks shared by the page methods (spec 5.5, 6.1, 6.5).
import { ZptahError } from "./errors.js";
import { checkUrl, requireReadable, schemeOf } from "./origins.js";

const MAX_TEXT = 8192;

/** @param {any} tab @returns {string} */
export function tabUrl(tab) {
  return String(tab.url || tab.pendingUrl || "");
}

/**
 * @param {any} tab @param {string | null} owner
 */
export function tabInfo(tab, owner) {
  const url = tabUrl(tab);
  return {
    tab: tab.id,
    window: tab.windowId ?? 0,
    url: url.slice(0, MAX_TEXT),
    title: String(tab.title ?? "").slice(0, MAX_TEXT),
    owner,
    active_in_window: Boolean(tab.active),
    audible: Boolean(tab.audible),
    loading: tab.status === "loading",
  };
}

/**
 * @param {any} api
 * @param {import("./origins.js").RosterEntry} entry
 * @param {string} mono
 * @param {boolean} all
 * @param {(tab: number) => string | null} ownerOf
 * @param {(mono: string) => string | undefined} [groupTitle] the title of a Mono's tab group
 */
export async function listTabs(api, entry, mono, all, ownerOf, groupTitle) {
  /** @type {any[]} */
  const tabs = await api.tabs.query({});
  /** @type {ReturnType<typeof tabInfo>[]} */
  const out = [];
  for (const tab of tabs) {
    if (typeof tab.id !== "number" || tab.incognito) continue;
    const owner = ownerOf(tab.id);
    if (!all && owner !== mono) continue;
    const url = tabUrl(tab);
    const verdict = checkUrl(entry, url);
    if (verdict === "blocked" || verdict === "not_allowed") continue;
    const info = /** @type {ReturnType<typeof tabInfo> & { group_title?: string }} */ (tabInfo(tab, owner));
    const title = owner && groupTitle ? groupTitle(owner) : undefined;
    if (title) info.group_title = title;
    if (verdict === "restricted") {
      info.title = "(browser page)";
      info.url = schemeOf(url);
    }
    out.push(info);
  }
  return { tabs: out };
}

/**
 * The tab a page method works on, or the protocol error why not.
 * @param {any} api @param {import("./origins.js").RosterEntry} entry @param {unknown} id
 */
export async function getReadableTab(api, entry, id) {
  if (!Number.isSafeInteger(id) || /** @type {number} */ (id) < 0) throw new ZptahError("E_INVALID", "tab must be a tab id");
  /** @type {any} */
  let tab;
  try {
    tab = await api.tabs.get(id);
  } catch {
    throw new ZptahError("E_TAB_GONE", "The tab is gone.");
  }
  if (!tab || typeof tab.id !== "number") throw new ZptahError("E_TAB_GONE", "The tab is gone.");
  if (tab.incognito) throw new ZptahError("E_INCOGNITO", "zptah Browser does not work in incognito windows.");
  requireReadable(entry, tabUrl(tab));
  return tab;
}
