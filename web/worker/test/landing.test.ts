// Routing of `/` and `/home` (src/landing.ts): the landing page for a request without a session cookie, the SPA with
// one. The Workers pool serves `app/dist/client` through the real ASSETS binding (placeholders when the app is not
// built, the real build otherwise), so the pages are told apart by the `zptah-page` meta that only the landing has.
import { describe, expect, it } from "vitest";
import { buildCsp, COMPANION_HEADERS } from "../src/middleware/security.ts";
import { call, ORIGIN } from "./helpers.ts";

const HOST = new URL(ORIGIN).host;
const LANDING_META = '<meta name="zptah-page" content="landing">';

async function kind(res: Response): Promise<"landing" | "spa"> {
  expect(res.status).toBe(200);
  expect(res.headers.get("Content-Type") ?? "").toContain("text/html");
  const body = await res.text();
  return body.includes(LANDING_META) ? "landing" : "spa";
}

function cookie(name: string, value = "token"): RequestInit {
  return { headers: { Cookie: `${name}=${value}` } };
}

describe("GET / chooses by the presence of the session cookie", () => {
  it("serves the landing page without a cookie", async () => {
    expect(await kind(await call("/"))).toBe("landing");
  });

  it("serves the SPA with the session cookie (presence only, the value is not validated)", async () => {
    expect(await kind(await call("/", cookie("__Host-mc_session", "not-a-real-session")))).toBe("spa");
    expect(await kind(await call("/", { headers: { Cookie: "a=b; __Host-mc_session=x; c=d" } }))).toBe("spa");
  });

  it.each([
    ["a different cookie", "theme=dark"],
    ["a name that only contains the session cookie name", "x__Host-mc_session=1"],
    ["the name without the __Host- prefix", "mc_session=1"],
    ["a name that extends it", "__Host-mc_session2=1"],
    ["an empty session cookie", "__Host-mc_session="],
    ["the name only inside a value", "other=__Host-mc_session=1"],
  ])("serves the landing page with %s", async (_label, header) => {
    expect(await kind(await call("/", { headers: { Cookie: header } }))).toBe("landing");
  });

  it("answers HEAD like GET, without a body", async () => {
    const landing = await call("/", { method: "HEAD" });
    expect(landing.status).toBe(200);
    expect(landing.headers.get("Vary")).toContain("Cookie");
    expect(await landing.text()).toBe("");
    const spa = await call("/", { method: "HEAD", ...cookie("__Host-mc_session") });
    expect(spa.status).toBe(200);
    expect(spa.headers.get("Vary")).toContain("Cookie");
  });

  it.each([
    ["no cookie", {}],
    ["the session cookie", cookie("__Host-mc_session")],
  ])("sets Vary: Cookie and no-store, with the CSP and companion headers, for %s", async (_label, init) => {
    const res = await call("/", init);
    expect(res.headers.get("Vary")).toMatch(/\bCookie\b/);
    expect(res.headers.get("Cache-Control")).toBe("no-store");
    expect(res.headers.get("Content-Security-Policy")).toBe(buildCsp(HOST));
    for (const [name, value] of Object.entries(COMPANION_HEADERS)) expect(res.headers.get(name), name).toBe(value);
  });

  it("does not send a conditional request of an earlier copy through to the assets", async () => {
    // The browser's cached `/` may be the SPA: its ETag must not turn the landing into a 304.
    const res = await call("/", { headers: { "If-None-Match": '"anything"', "If-Modified-Since": "Wed, 01 Jan 2020 00:00:00 GMT" } });
    expect(await kind(res)).toBe("landing");
  });
});

describe("GET /home", () => {
  it.each([{}, cookie("__Host-mc_session")])("is always the landing page (%j)", async (init) => {
    const res = await call("/home", init);
    expect(await kind(res)).toBe("landing");
    expect(res.headers.get("Vary")).toMatch(/\bCookie\b/);
    expect(res.headers.get("Cache-Control")).toBe("no-store");
  });

  it("also answers /home/", async () => {
    expect(await kind(await call("/home/"))).toBe("landing");
  });
});

describe("GET /privacy", () => {
  it.each([{}, cookie("__Host-mc_session")])("is the privacy page for everyone (%j)", async (init) => {
    for (const path of ["/privacy", "/privacy/"]) {
      const res = await call(path, init);
      expect(res.status, path).toBe(200);
      expect(res.headers.get("Content-Type") ?? "").toContain("text/html");
      const body = await res.text();
      // The real build has the page; the placeholder assets of an unbuilt app have no privacy page and answer with the SPA.
      expect(body.includes('<meta name="zptah-page" content="privacy">') || !body.includes(LANDING_META), path).toBe(true);
      expect(res.headers.get("Vary") ?? "").not.toMatch(/\bCookie\b/);
    }
  });
});

describe("everything else is unchanged", () => {
  it.each(["/d/abc", "/d/abc/s/s1", "/settings", "/settings/notifications", "/pair/ABCD-1234", "/app", "/login", "/register"])(
    "%s is the SPA with or without a cookie, and not marked Vary: Cookie",
    async (path) => {
      for (const init of [{}, cookie("__Host-mc_session")]) {
        const res = await call(path, init);
        expect(await kind(res), path).toBe("spa");
        expect(res.headers.get("Cache-Control"), path).not.toBe("no-store");
      }
    },
  );

  it("keeps /api and /relay as JSON 404s for unknown paths", async () => {
    for (const path of ["/api/nope", "/relay/nope"]) {
      const res = await call(path);
      expect(res.status).toBe(404);
      expect(res.headers.get("Content-Type")).toContain("application/json");
      expect(res.headers.get("Cache-Control")).toBe("no-store");
    }
  });

  it("keeps POST / on the old path: the catch-all answers it exactly like any other non-GET path", async () => {
    const res = await call("/", { method: "POST" });
    const other = await call("/d/some/deep/link", { method: "POST" });
    expect(res.status).toBe(other.status);
    expect(await res.text()).toBe(await other.text());
    expect(res.headers.get("Vary") ?? "").not.toMatch(/\bCookie\b/);
    expect(res.headers.get("Cache-Control")).not.toBe("no-store");
  });

  it("serves the landing's own files under /landing/ for everyone", async () => {
    const res = await call("/landing/");
    expect(await kind(res)).toBe("landing");
  });
});
