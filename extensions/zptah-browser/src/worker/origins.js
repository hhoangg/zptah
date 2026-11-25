// Per-Mono origin rules (spec 6.5 rule 2) and restricted pages (spec 6.1). Mirrors
// `Origins::check` in crates/monocode-core/src/mono/browser/gate/extension.rs.
import { ZptahError } from "./errors.js";

const STORE_HOSTS = ["chromewebstore.google.com", "microsoftedge.microsoft.com"];
const CHALLENGE = [/(^|\.)challenges\.cloudflare\.com$/, /(^|\.)hcaptcha\.com$/, /(^|\.)arkoselabs\.com$/, /^geo\.captcha-delivery\.com$/];

/**
 * Same as the app's `normalize_origin`; null when the entry is not an origin.
 * @param {unknown} input
 * @returns {string | null}
 */
export function normalizeOrigin(input) {
  if (typeof input !== "string") return null;
  const text = input.trim();
  if (text === "" || /[*; \n]/.test(text)) return null;
  try {
    const url = new URL(text.includes("://") ? text : `https://${text}`);
    if ((url.protocol !== "http:" && url.protocol !== "https:") || url.hostname === "") return null;
    return url.origin;
  } catch {
    return null;
  }
}

/** @param {string} url @returns {URL | null} */
function parse(url) {
  try {
    return new URL(url.trim());
  } catch {
    return null;
  }
}

/**
 * The web origin of an http(s) address, else null.
 * @param {string} url
 */
export function originOf(url) {
  const u = parse(url);
  if (!u || (u.protocol !== "http:" && u.protocol !== "https:") || u.hostname === "") return null;
  return u.origin;
}

/**
 * Browser pages, files and web stores are never read. An empty address (a tab that is still
 * opening) and about:blank are harmless.
 * @param {string} url
 */
export function isRestricted(url) {
  if (url === "" || url === "about:blank") return false;
  const u = parse(url);
  if (!u || (u.protocol !== "http:" && u.protocol !== "https:") || u.hostname === "") return true;
  const host = u.hostname.toLowerCase();
  if (host === "chromewebstore.google.com") return true;
  if (host === "chrome.google.com" && u.pathname.startsWith("/webstore")) return true;
  if (host === "microsoftedge.microsoft.com" && u.pathname.startsWith("/addons")) return true;
  return false;
}

/** Scheme text for a restricted page in a tab list, e.g. "chrome:". @param {string} url */
export function schemeOf(url) {
  const u = parse(url);
  return u ? u.protocol : "about:";
}

/** Anti-bot challenge frames are never injected into (spec 6.4). @param {string} url */
export function isChallengeFrame(url) {
  const u = parse(url);
  if (!u) return false;
  const host = u.hostname.toLowerCase();
  if (host === "www.google.com" && u.pathname.startsWith("/recaptcha/")) return true;
  return CHALLENGE.some((re) => re.test(host));
}

/** @typedef {{ id: string, allowed_origins: string[], blocked_origins: string[], level?: string, name?: string, emoji?: string, color?: string, group_color?: string, cursor?: boolean }} RosterEntry */

/**
 * "ok" when the page may be read, else why not.
 * @param {RosterEntry | null} entry
 * @param {string} url
 * @returns {"ok" | "restricted" | "blocked" | "not_allowed"}
 */
export function checkUrl(entry, url) {
  if (isRestricted(url)) return "restricted";
  const origin = originOf(url);
  if (origin === null) return "ok";
  const blocked = (entry?.blocked_origins ?? []).map(normalizeOrigin);
  if (blocked.includes(origin)) return "blocked";
  const allowed = (entry?.allowed_origins ?? []).map(normalizeOrigin).filter((o) => o !== null);
  if (allowed.length > 0 && !allowed.includes(origin)) return "not_allowed";
  return "ok";
}

/**
 * Throws the protocol error for a page that may not be read.
 * @param {RosterEntry | null} entry @param {string} url
 */
export function requireReadable(entry, url) {
  const verdict = checkUrl(entry, url);
  if (verdict === "restricted") throw new ZptahError("E_RESTRICTED_URL", "This is a browser page (or a file): zptah Browser never reads it.");
  if (verdict === "blocked") throw new ZptahError("E_ORIGIN_BLOCKED", "This site is on this Mono's blocked list, so zptah Browser does not read it.");
  if (verdict === "not_allowed") throw new ZptahError("E_ORIGIN_BLOCKED", "This site is not on this Mono's allowed list, so zptah Browser does not read it.");
}

/**
 * The roster as the link holds it: an array (protocol) or `{monos}`; `roster.set` replaces it.
 * @param {{ roster: unknown }} link
 * @param {{ override: unknown, seen: unknown }} slot
 * @returns {RosterEntry[]}
 */
export function currentRoster(link, slot) {
  // A new connection brings a new roster object; it wins over an older roster.set.
  if (slot.seen !== link.roster) {
    slot.seen = link.roster;
    slot.override = null;
  }
  const raw = slot.override ?? link.roster;
  const list = Array.isArray(raw) ? raw : /** @type {any} */ (raw)?.monos;
  return Array.isArray(list) ? list : [];
}
