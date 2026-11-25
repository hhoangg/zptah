// Service worker (W-12, architecture.md 3.5). Built by vite-plugin-pwa with `injectManifest`: the
// precache list replaces `self.__WB_MANIFEST` at build time, and the result is emitted as /sw.js.
//
// Structure:
//  1. precache and navigation fallback (offline start)
//  2. update protocol: no skipWaiting on install; the page asks with a SKIP_WAITING message
//  3. push handlers: `push`, `notificationclick`, `pushsubscriptionchange` (src/sw/pushHandlers.ts)
//
// Never cached: /api/* and /relay/* (src/sw/routing.ts). There are no runtime caching routes.
import { clientsClaim } from "workbox-core";
import { cleanupOutdatedCaches, createHandlerBoundToURL, matchPrecache, precacheAndRoute } from "workbox-precaching";
import { NavigationRoute, registerRoute } from "workbox-routing";
import { idbPendingStore } from "./sw/pendingTarget";
import { createPushHandlers } from "./sw/pushHandlers";
import { resubscribe } from "./sw/resubscribe";
import { createRootNavigationHandler } from "./sw/rootNavigation";
import { isRootNavigation, NAVIGATION_DENYLIST, NAVIGATION_FALLBACK } from "./sw/routing";

declare const self: ServiceWorkerGlobalScope & { __WB_MANIFEST: Array<string | { url: string; revision: string | null }> };

// --- 1. Precache and navigation fallback ---

cleanupOutdatedCaches();
// An empty directoryIndex (falsy, and the types do not take null), or Workbox answers a navigation to `/` with the precached index.html before the denylist below
// is consulted, and a signed-out visitor would get the shell instead of the landing page.
precacheAndRoute(self.__WB_MANIFEST, { directoryIndex: "" });
// `/` is denylisted below (the Worker decides landing or app), but an app installed with the old start_url `/` must still
// open offline: network first, then the precached shell (sw/rootNavigation.ts).
const rootNavigation = createRootNavigationHandler({
  fetch: (request) => fetch(request),
  matchShell: () => matchPrecache(NAVIGATION_FALLBACK),
});
registerRoute(({ request, url }) => request.mode === "navigate" && isRootNavigation(url.pathname), ({ request }) => rootNavigation(request));
registerRoute(new NavigationRoute(createHandlerBoundToURL(NAVIGATION_FALLBACK), { denylist: [...NAVIGATION_DENYLIST] }));

// --- 2. Update protocol ---

// A new worker waits until the user taps Reload (features/offline/swUpdate.ts): a link that is running or
// a message that is being sent must not be cut by an update the user did not ask for.
self.addEventListener("message", (event) => {
  const data: unknown = event.data;
  if (typeof data === "object" && data !== null && (data as { type?: unknown }).type === "SKIP_WAITING") {
    void self.skipWaiting();
  }
});

// The first install has nothing to wait for; claiming lets that very page load work offline on the next
// navigation without a second reload. After an update the page reloads on `controlling` anyway.
clientsClaim();

// --- 3. Push handlers (P-04) ---
// No fetch handler and no caching here: see src/sw/pushHandlers.ts for the rules of the three listeners.

const pendingStore = idbPendingStore(self.indexedDB);
const pushHandlers = createPushHandlers({
  savePending: (url, at) => pendingStore.put(url, at),
  origin: self.location.origin,
  showNotification: (title, options) => self.registration.showNotification(title, options),
  listWindowClients: () => self.clients.matchAll({ type: "window", includeUncontrolled: true }),
  openWindow: (url) => self.clients.openWindow(url),
  resubscribe: () => resubscribe({ pushManager: self.registration.pushManager, fetch: (input, init) => fetch(input, init) }),
});
self.addEventListener("push", (event) => pushHandlers.push(event));
self.addEventListener("notificationclick", (event) => pushHandlers.notificationclick(event));
self.addEventListener("pushsubscriptionchange", (event) => pushHandlers.pushsubscriptionchange(event));
