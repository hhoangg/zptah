// The user's in-page effects settings (spec 10, 11.3), kept in chrome.storage.local: the global
// cursor switch and the list of sites without in-page effects. Shared by the worker and the
// options page.
export const PREFS_KEY = "zptah.prefs";

/** @typedef {{ cursor: boolean, off_sites: string[] }} Prefs */

/** @returns {Prefs} */
export const defaultPrefs = () => ({ cursor: true, off_sites: [] });

/**
 * `example.com` or `*.example.com`, lower case, no scheme, port or path; null when it is neither.
 * @param {unknown} input
 * @returns {string | null}
 */
export function normalizeSite(input) {
  if (typeof input !== "string") return null;
  const text = input.trim().toLowerCase();
  const m = /^(\*\.)?((?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)$/.exec(text);
  return m ? `${m[1] ?? ""}${m[2]}` : null;
}

/** @param {unknown} raw @returns {Prefs} */
export function parsePrefs(raw) {
  const out = defaultPrefs();
  if (typeof raw !== "object" || raw === null) return out;
  const r = /** @type {Record<string, unknown>} */ (raw);
  if (typeof r.cursor === "boolean") out.cursor = r.cursor;
  if (Array.isArray(r.off_sites)) {
    for (const s of r.off_sites) {
      const site = normalizeSite(s);
      if (site && !out.off_sites.includes(site)) out.off_sites.push(site);
    }
  }
  return out;
}

/**
 * Whether the page's host is on the off list. `example.com` matches that host only,
 * `*.example.com` its subdomains.
 * @param {string[]} list @param {string} url
 */
export function siteIsOff(list, url) {
  let host;
  try {
    host = new URL(url).hostname.toLowerCase();
  } catch {
    return true;
  }
  return list.some((entry) => (entry.startsWith("*.") ? host.endsWith(entry.slice(1)) && host.length > entry.length - 1 : host === entry));
}

/**
 * @param {{ get(key: string): Promise<Record<string, unknown>> }} area
 * @returns {Promise<Prefs>}
 */
export async function loadPrefs(area) {
  try {
    return parsePrefs((await area.get(PREFS_KEY))[PREFS_KEY]);
  } catch {
    return defaultPrefs();
  }
}
