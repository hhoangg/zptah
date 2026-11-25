// The public landing page of the official host (web/landing). It is a static asset under /landing/ (built into the
// assets directory next to the SPA), and the Worker decides which of the two a request for `/` gets:
//   - `/home`: always the landing page.
//   - `/`: the landing page when the request has no session cookie, the SPA (the assets' index.html) when it has one.
// Presence check only: the cookie is not validated and D1 is not touched. A stale or forged cookie gets the SPA, whose
// own auth guard sends it to /login. Both answers vary on the cookie, so no cache may share one with the other.
import type { Hono } from "hono";
import type { AppEnv } from "./http.ts";
import { readSessionToken } from "./modules/sessions/session.ts";

/** Where the landing build puts its page inside the assets directory (web/landing/build.mjs). */
export const LANDING_ASSET_PATH = "/landing/";

/** True when the request carries a non-empty `__Host-mc_session` cookie (exact name, no validation). */
export function hasSessionCookie(request: Request): boolean {
  return readSessionToken(request) !== null;
}

function withCookieVariance(res: Response): Response {
  const headers = new Headers(res.headers);
  const vary = headers.get("Vary");
  headers.set("Vary", vary ? (/\bCookie\b/i.test(vary) ? vary : `${vary}, Cookie`) : "Cookie");
  headers.set("Cache-Control", "no-store");
  return new Response(res.body, { status: res.status, statusText: res.statusText, headers });
}

async function fetchLanding(env: AppEnv["Bindings"], request: Request): Promise<Response> {
  const url = new URL(LANDING_ASSET_PATH, request.url);
  // A fresh GET without the conditional headers of the browser's copy of `/` (that copy may be the SPA).
  return env.ASSETS.fetch(new Request(url, { method: "GET", headers: { Accept: request.headers.get("Accept") ?? "text/html" } }));
}

/** Registers `GET /` and `GET /home` (HEAD is answered from GET by Hono). Other methods fall through to the catch-all. */
export function landingRoutes(app: Hono<AppEnv>): void {
  app.on("GET", ["/home", "/home/"], async (c) => withCookieVariance(await fetchLanding(c.env, c.req.raw)));
  app.on("GET", "/", async (c) => {
    const res = hasSessionCookie(c.req.raw) ? await c.env.ASSETS.fetch(c.req.raw) : await fetchLanding(c.env, c.req.raw);
    return withCookieVariance(res);
  });
}
