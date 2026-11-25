// One Chrome tab group per Mono per window (spec 9.1): titled "<emoji> <name>", colored like the
// Mono. A group is presentation: ownership is keyed by Mono id and tab id (ownership.js), never by
// the title.
import { ZptahError } from "./errors.js";

/** @typedef {import("./origins.js").RosterEntry} RosterEntry */

/** The longest group title (spec 9.1). */
export const MAX_TITLE = 40;

/** The nine colors Chrome offers for a group. */
export const GROUP_COLORS = ["grey", "blue", "red", "yellow", "green", "pink", "purple", "cyan", "orange"];

/** @param {string} text @param {number} max code points, not UTF-16 units */
function clip(text, max) {
  const chars = Array.from(text);
  return chars.length <= max ? text : chars.slice(0, max).join("").trimEnd();
}

/**
 * The title of a Mono's group: `<emoji> <name>`, at most 40 characters. Two Monos with the same
 * title get a ` 2`, ` 3` suffix in the later one (roster order).
 * @param {RosterEntry} entry
 * @param {RosterEntry[]} roster
 */
export function groupTitle(entry, roster) {
  const base = (/** @type {RosterEntry} */ e) => clip(`${e.emoji ?? ""} ${e.name ?? ""}`.trim(), MAX_TITLE);
  const own = base(entry);
  let rank = 1;
  for (const other of roster) {
    if (other.id === entry.id) break;
    // The earlier Mono's own suffixed title counts too, so three of a kind get 2 and 3.
    if (base(other) === own) rank += 1;
  }
  if (rank === 1) return own;
  const suffix = ` ${rank}`;
  return `${clip(`${entry.emoji ?? ""} ${entry.name ?? ""}`.trim(), MAX_TITLE - suffix.length)}${suffix}`;
}

/** @param {unknown} color @returns {string} */
export function chromeColor(color) {
  return typeof color === "string" && GROUP_COLORS.includes(color) ? color : "grey";
}

/**
 * @param {object} deps
 * @param {any} deps.api the chrome object (or a fake)
 * @param {() => RosterEntry[]} deps.roster the current roster
 * @param {(tabId: number) => void} [deps.expectChange] told before the worker moves a tab itself, so its own
 *   moves are not taken for the user's drags
 */
export function createGroups(deps) {
  const { api } = deps;
  /** "<mono>:<window>" to the group id. @type {Map<string, number>} */
  const byMonoWindow = new Map();
  /** group id to Mono id. @type {Map<number, string>} */
  const monoOfGroup = new Map();

  /** @param {string} mono @param {number} windowId */
  const key = (mono, windowId) => `${mono}:${windowId}`;

  /** @param {string} mono @param {number} windowId @param {number} groupId */
  function remember(mono, windowId, groupId) {
    byMonoWindow.set(key(mono, windowId), groupId);
    monoOfGroup.set(groupId, mono);
  }

  /** @param {number} groupId */
  function forget(groupId) {
    const mono = monoOfGroup.get(groupId);
    monoOfGroup.delete(groupId);
    for (const [k, v] of byMonoWindow) if (v === groupId) byMonoWindow.delete(k);
    return mono ?? null;
  }

  /** @param {string} mono */
  function entryOf(mono) {
    return deps.roster().find((m) => m.id === mono) ?? null;
  }

  /** @param {RosterEntry} entry */
  function titleOf(entry) {
    return groupTitle(entry, deps.roster());
  }

  /** The Mono a group belongs to, from what this worker made or was offered back. @param {number} groupId */
  const monoOf = (groupId) => monoOfGroup.get(groupId) ?? null;

  /**
   * The group of a Mono in a window, if it still exists.
   * @param {string} mono @param {number} windowId
   * @returns {Promise<number | null>}
   */
  async function existing(mono, windowId) {
    const id = byMonoWindow.get(key(mono, windowId));
    if (id === undefined) return null;
    try {
      const group = await api.tabGroups.get(id);
      if (group && group.windowId === windowId) return id;
    } catch {
      // The user closed the group.
    }
    forget(id);
    return null;
  }

  /**
   * Puts a tab into the Mono's group in the tab's window (a new group when there is none) and
   * gives the group its title and color. Never creates a window. Returns the group id, or null
   * when Chrome does not group tabs of that window (a popup).
   * @param {number} tabId @param {RosterEntry} entry
   * @returns {Promise<number | null>}
   */
  async function place(tabId, entry) {
    const tab = await api.tabs.get(tabId);
    const windowId = tab.windowId;
    deps.expectChange?.(tabId);
    try {
      const known = await existing(entry.id, windowId);
      /** @type {number} */
      let groupId;
      if (known === null) {
        // Chrome puts the group in the window of the tab; the explicit windowId is what makes a
        // group made from the worker land in that window, not in the last focused one.
        groupId = await api.tabs.group({ tabIds: [tabId], createProperties: { windowId } });
        remember(entry.id, windowId, groupId);
      } else {
        groupId = known;
        await api.tabs.group({ tabIds: [tabId], groupId });
      }
      await api.tabGroups.update(groupId, { title: titleOf(entry), color: chromeColor(entry.group_color) });
      return groupId;
    } catch (err) {
      if (/popup|grouping|window/i.test(String(err instanceof Error ? err.message : err))) return null;
      throw new ZptahError("E_UNSUPPORTED", `The tab could not be put in the group: ${String(err instanceof Error ? err.message : err).slice(0, 200)}`);
    }
  }

  /** The tab leaves its group and stays open. @param {number} tabId */
  async function ungroup(tabId) {
    deps.expectChange?.(tabId);
    try {
      await api.tabs.ungroup(tabId);
    } catch {
      // The tab is gone, or was not grouped.
    }
  }

  /**
   * The roster changed: every group takes the Mono's current title and color, and a restored group
   * whose title and color match a roster entry exactly is offered back to that Mono (its tabs stay
   * the user's, nothing is re-attached).
   */
  async function refresh() {
    const roster = deps.roster();
    /** @type {any[]} */
    let groups = [];
    try {
      groups = (await api.tabGroups?.query?.({})) ?? [];
    } catch {
      groups = [];
    }
    for (const entry of roster) {
      const title = titleOf(entry);
      const color = chromeColor(entry.group_color);
      for (const group of groups) {
        if (monoOfGroup.get(group.id) === entry.id) {
          if (group.title !== title || group.color !== color) {
            try {
              await api.tabGroups.update(group.id, { title, color });
            } catch {
              forget(group.id);
            }
          }
        } else if (!monoOfGroup.has(group.id) && group.title === title && group.color === color && !byMonoWindow.has(key(entry.id, group.windowId))) {
          remember(entry.id, group.windowId, group.id);
        }
      }
    }
  }

  /** @param {string} mono */
  function forgetMono(mono) {
    for (const [id, owner] of [...monoOfGroup]) if (owner === mono) forget(id);
  }

  return { place, ungroup, refresh, monoOf, forget, forgetMono, titleOf, entryOf, existing, remember };
}
