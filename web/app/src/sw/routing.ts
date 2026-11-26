// What the service worker may cache and answer (architecture.md 3.5, security.md). Pure data and
// predicates, no service worker globals, so a unit test can read it and check-dist can compare it with
// the built sw.js. sw.ts is the only consumer that wires it into Workbox.

/** The shell document every navigation falls back to when offline. */
export const NAVIGATION_FALLBACK = "/index.html";

/**
 * Navigations that must reach the network and never get the shell: the API and the relay (a response of either is
 * private, cookies and ciphertext frames, and is never stored in Cache Storage), the landing page and the privacy
 * policy page (`/privacy`, a static page of the Worker, not an SPA route). `/` is the landing page when the request has no session cookie and `/home` always is, and only the Worker can tell: a cached
 * shell must not hide the landing from a signed-out visitor. The installed app starts at `/app` (the manifest), which
 * is cached like every other route. `/landing/` holds the landing's own files.
 */
export const NAVIGATION_DENYLIST: readonly RegExp[] = [
  /^\/api\//,
  /^\/relay\//,
  // Workbox matches a denylist entry against pathname + search, so each landing entry allows a query string (`/?utm_source=x`).
  /^\/(\?.*)?$/,
  /^\/home\/?(\?.*)?$/,
  /^\/landing(\/|\?|$)/,
  /^\/privacy(\/|\?|$)/,
];

/**
 * Runtime caching routes. None, on purpose: the only things in Cache Storage are the precached build
 * assets. Adding a route here needs a security review; a test fails if one matches /api or /relay.
 */
export const RUNTIME_CACHE_ROUTES: readonly { readonly pattern: RegExp; readonly handler: string }[] = [];

/** True when a navigation to `pathname` (plus `search`, as Workbox matches it) may be answered with the cached shell. */
export function isShellNavigation(pathname: string, search = ""): boolean {
  const target = pathname + search;
  return !NAVIGATION_DENYLIST.some((pattern) => pattern.test(target));
}

/** True for a navigation to exactly `/` (any query). The Worker decides what it is; see rootNavigation.ts. */
export function isRootNavigation(pathname: string): boolean {
  return pathname === "/";
}

/** True for paths that must never be served from or written to a cache by the service worker. */
export function isNeverCached(pathname: string): boolean {
  return /^\/(api|relay)(\/|$)/.test(pathname);
}
