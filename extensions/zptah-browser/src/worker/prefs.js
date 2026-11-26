// The user's settings (spec 10, 11.3), kept in chrome.storage.local: the global
// cursor switch, the list of sites without in-page effects and the debugger fallback switch. Shared by the worker and the
// options page.
export const PREFS_KEY = "zptah.prefs";

/** @typedef {{ cursor: boolean, off_sites: string[], use_debugger: boolean }} Prefs */

/** @returns {Prefs} */
export const defaultPrefs = () => ({ cursor: true, off_sites: [], use_debugger: false });

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
  if (typeof r.use_debugger === "boolean") out.use_debugger = r.use_debugger;
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

/**
 * Whether the debugger fallback may be used: `debugger` is a required manifest permission (Chrome
 * does not allow it as an optional one), so chrome.debugger exists, and the user's switch in the
 * options page decides. Off by default.
 * @param {any} api the chrome object (or a fake)
 */
export async function debuggerAllowed(api) {
  if (!api?.debugger) return false;
  try {
    return (await loadPrefs(api.storage.local)).use_debugger === true;
  } catch {
    return false;
  }
}

/**
 * Writes the settings one change at a time. Each change reads the stored prefs when its turn
 * comes and writes the merged result, so two quick clicks (or two pages) cannot overwrite each
 * other with a stale copy. A failed write rejects its own call and does not block the next.
 * @param {{ get(key: string): Promise<Record<string, unknown>>, set(items: Record<string, unknown>): Promise<void> }} area
 */
export function createPrefsWriter(area) {
  /** @type {Promise<unknown>} */
  let tail = Promise.resolve();
  /** @param {(cur: Prefs) => Prefs} change @returns {Promise<Prefs>} */
  return (change) => {
    const run = tail.then(async () => {
      const next = parsePrefs(change(await loadPrefs(area)));
      await area.set({ [PREFS_KEY]: next });
      return next;
    });
    tail = run.catch(() => undefined);
    return run;
  };
}
