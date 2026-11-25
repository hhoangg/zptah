// Pairing state in chrome.storage.local: the install's instance_id (created once) and the
// pairing key K (set by pair.begin, deleted on disconnect).
import { toHex } from "./bytes.js";
import { randomBytes } from "./crypto.js";

const KEY = "zptah.link";

/**
 * @param {{ get(key: string): Promise<Record<string, unknown>>, set(items: Record<string, unknown>): Promise<void> }} area
 */
export function createStore(area) {
  /** @returns {Promise<{ instanceId: string, key: string | null }>} */
  async function load() {
    const stored = (await area.get(KEY))[KEY];
    const rec = typeof stored === "object" && stored !== null ? /** @type {Record<string, unknown>} */ (stored) : {};
    let instanceId = typeof rec.instance_id === "string" && /^[0-9a-f]{32}$/.test(rec.instance_id) ? rec.instance_id : "";
    const key = typeof rec.key === "string" ? rec.key : null;
    if (!instanceId) {
      instanceId = toHex(randomBytes(16));
      await area.set({ [KEY]: { instance_id: instanceId, key } });
    }
    return { instanceId, key };
  }
  /** @param {string | null} key */
  async function saveKey(key) {
    const { instanceId } = await load();
    await area.set({ [KEY]: { instance_id: instanceId, key } });
  }
  return { load, saveKey };
}

/**
 * Restricts chrome.storage.local to trusted contexts (no content scripts) and returns an area
 * whose every read and write waits for that call, so K is never readable from a page context.
 * @param {{ setAccessLevel(o: { accessLevel: string }): Promise<void>, get(key: string): Promise<Record<string, unknown>>, set(items: Record<string, unknown>): Promise<void> }} local
 */
export function trustedArea(local) {
  const ready = local.setAccessLevel({ accessLevel: "TRUSTED_CONTEXTS" });
  return {
    /** @param {string} key */
    get: async (key) => (await ready, local.get(key)),
    /** @param {Record<string, unknown>} items */
    set: async (items) => (await ready, local.set(items)),
  };
}
