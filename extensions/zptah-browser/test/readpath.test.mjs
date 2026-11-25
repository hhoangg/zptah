// @ts-nocheck - chrome API fakes and a fake link are duck-typed test doubles.
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import test from "node:test";
import { pageText } from "../src/worker/paging.js";
import { fitPng, fromBase64, pngSize, toBase64 } from "../src/worker/screenshot.js";
import { checkUrl, isChallengeFrame, isRestricted, normalizeOrigin } from "../src/worker/origins.js";
import { registerReadPath } from "../src/worker/readpath.js";

const MONO = "a".repeat(32);
const OTHER = "b".repeat(32);

function png(width, height, total) {
  const b = new Uint8Array(Math.max(total, 24));
  [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a].forEach((v, i) => (b[i] = v));
  const dv = new DataView(b.buffer);
  dv.setUint32(16, width);
  dv.setUint32(20, height);
  return b;
}
const dataUrl = (b) => `data:image/png;base64,${toBase64(b)}`;

/** Fake imaging: output size is proportional to the area. */
const imaging = {
  calls: [],
  async render(bytes, { crop, scalePct }) {
    const size = pngSize(bytes);
    const w = Math.max(1, Math.round(((crop?.width ?? size.width) * scalePct) / 100));
    const h = Math.max(1, Math.round(((crop?.height ?? size.height) * scalePct) / 100));
    imaging.calls.push({ crop, scalePct });
    return { bytes: png(w, h, Math.round((bytes.length * scalePct * scalePct) / 10000)), width: w, height: h };
  },
};

function setup(opts = {}) {
  const calls = [];
  let clock = 1_000_000;
  const tabs = new Map(
    Object.entries(
      opts.tabs ?? {
        1: { id: 1, windowId: 10, url: "https://shop.test/a", title: "Shop", active: true, status: "complete" },
      },
    ).map(([k, v]) => [Number(k), v]),
  );
  const frames = opts.frames ?? { 1: [{ frameId: 0, parentFrameId: -1, url: "https://shop.test/a" }] };
  /** content per tab and frame: ops answered by the fake page */
  const content = opts.content ?? {};
  const injected = new Set();
  const api = {
    tabs: {
      get: async (id) => {
        if (!tabs.has(id)) throw new Error(`No tab with id: ${id}.`);
        return tabs.get(id);
      },
      query: async (q) => [...tabs.values()].filter((t) => q.windowId === undefined || t.windowId === q.windowId),
      captureVisibleTab: async (...a) => {
        calls.push(["captureVisibleTab", ...a]);
        return opts.capture ? opts.capture() : dataUrl(png(1000, 800, 5000));
      },
      update: async () => assert.fail("must never update a tab"),
    },
    windows: { get: async () => ({ state: opts.windowState ?? "normal" }), update: async () => assert.fail("must never update a window") },
    webNavigation: { getAllFrames: async ({ tabId }) => frames[tabId] ?? [{ frameId: 0, parentFrameId: -1, url: tabs.get(tabId)?.url ?? "" }] },
    scripting: {
      executeScript: async ({ target, func, files, args }) => {
        const key = `${target.tabId}:${target.frameIds[0]}`;
        calls.push(["executeScript", key, files ? "inject" : args?.[0] ?? "probe"]);
        if (opts.hang) return new Promise(() => {});
        if (files) {
          injected.add(key);
          return [{ frameId: target.frameIds[0], result: undefined }];
        }
        if (!args) return [{ frameId: target.frameIds[0], result: injected.has(key) ? 1 : 0 }];
        const fn = content[key];
        if (!fn) throw new Error("Cannot access contents of the page.");
        return [{ frameId: target.frameIds[0], result: await fn(args[0], args[1]) }];
      },
    },
    permissions: { contains: async () => opts.debuggerPermission ?? false },
    debugger: {
      getTargets: async () => (opts.attached ? [{ tabId: 1, attached: true }] : [{ tabId: 1, attached: false }]),
      sendCommand: async (target, method, params) => {
        calls.push(["cdp", method, params]);
        if (method === "Page.getLayoutMetrics") return { cssContentSize: { width: 1280, height: 5000 } };
        return { data: toBase64(opts.cdpPng ?? png(800, 600, 4000)) };
      },
    },
  };
  const link = {
    handlers: new Map(),
    stopped: opts.stopped ?? false,
    roster: opts.roster ?? [
      { id: MONO, name: "Mira", allowed_origins: [], blocked_origins: ["https://bank.test"], level: "read" },
      { id: OTHER, name: "Kai", allowed_origins: ["https://shop.test"], blocked_origins: [], level: "read" },
    ],
  };
  const pages = registerReadPath(link, {
    api,
    now: () => clock,
    sleep: async (ms) => {
      clock += ms;
    },
    imaging,
    ownerOf: opts.ownerOf,
    captureTimeoutMs: 30,
  });
  const call = async (method, params, mono = MONO) => {
    const res = await link.handlers.get(method)({ v: 1, type: "req", id: "a-1", method, mono, params });
    return res;
  };
  return { api, link, call, calls, tabs, pages, advance: (ms) => (clock += ms) };
}

const err = (res, code) => {
  assert.equal(res.ok, false, JSON.stringify(res));
  assert.equal(res.error.code, code, JSON.stringify(res));
  return res.error.message;
};

// ---- origins ----

test("origin helpers follow the app's rules", () => {
  assert.equal(normalizeOrigin("Example.com"), "https://example.com");
  assert.equal(normalizeOrigin("http://a.test:80/x"), "http://a.test");
  assert.equal(normalizeOrigin("*.a.test"), null);
  assert.equal(normalizeOrigin("ftp://a.test"), null);
  for (const u of ["chrome://settings", "chrome-extension://x/y", "file:///etc/passwd", "view-source:https://a.test", "about:config", "https://chromewebstore.google.com/detail/x", "https://chrome.google.com/webstore/detail/x", "https://microsoftedge.microsoft.com/addons/x", "data:text/html,x"]) {
    assert.equal(isRestricted(u), true, u);
  }
  for (const u of ["about:blank", "", "https://chrome.google.com/other", "https://a.test/"]) assert.equal(isRestricted(u), false, u);
  assert.equal(checkUrl({ allowed_origins: [], blocked_origins: ["bank.test"] }, "https://bank.test/x"), "blocked");
  assert.equal(checkUrl({ allowed_origins: ["https://a.test"], blocked_origins: [] }, "https://b.test/"), "not_allowed");
  assert.equal(checkUrl({ allowed_origins: ["https://a.test"], blocked_origins: [] }, "about:blank"), "ok");
  for (const u of ["https://challenges.cloudflare.com/x", "https://newassets.hcaptcha.com/", "https://www.google.com/recaptcha/api2/anchor", "https://x.arkoselabs.com/", "https://geo.captcha-delivery.com/"]) assert.equal(isChallengeFrame(u), true, u);
  assert.equal(isChallengeFrame("https://www.google.com/search"), false);
});

test("pageText cuts on line boundaries within the byte budget and resumes", () => {
  const text = ["- one", "- two", "- thrée", "- four"].join("\n");
  const first = pageText(text, 0, 14);
  assert.equal(first.text, "- one\n- two\n");
  assert.equal(first.truncated, true);
  assert.equal(first.total_bytes, new TextEncoder().encode(text).length);
  const second = pageText(text, first.next_from, 14);
  assert.equal(second.text, "- thrée\n");
  const third = pageText(text, second.next_from, 100);
  assert.equal(third.text, "- four");
  assert.equal(third.truncated, false);
  assert.equal(third.next_from, undefined);
  assert.equal(pageText("x".repeat(10), 0, 4).text, "xxxx");
  assert.equal(pageText("ééé", 1, 10).text, "éé");
});

// ---- scoping ----

test("requests need a known Mono and a running browser", async () => {
  const s = setup();
  err(await s.call("tabs.list", { all: true }, "c".repeat(32)), "E_UNKNOWN_MONO");
  err(await s.call("tabs.list", { all: true }, "nothex"), "E_INVALID");
  s.link.stopped = true;
  err(await s.call("tabs.list", { all: true }), "E_STOPPED");
});

test("roster.set replaces the roster until a new connection brings its own", async () => {
  const s = setup();
  const newcomer = "d".repeat(32);
  err(await s.call("tabs.list", { all: true }, newcomer), "E_UNKNOWN_MONO");
  const set = await s.link.handlers.get("roster.set")({ id: "a-2", method: "roster.set", params: { monos: [{ id: newcomer, allowed_origins: [], blocked_origins: [] }] } });
  assert.equal(set.ok, true);
  assert.equal((await s.call("tabs.list", { all: true }, newcomer)).ok, true);
  err(await s.call("tabs.list", { all: true }), "E_UNKNOWN_MONO");
  s.link.roster = [{ id: MONO, allowed_origins: [], blocked_origins: [] }];
  assert.equal((await s.call("tabs.list", { all: true })).ok, true);
  const bad = await s.link.handlers.get("roster.set")({ id: "a-3", method: "roster.set", params: {} });
  err(bad, "E_INVALID");
});

test("the roster may arrive as an object with monos", async () => {
  const s = setup({ roster: { monos: [{ id: MONO, allowed_origins: [], blocked_origins: [] }] } });
  assert.equal((await s.call("tabs.list", { all: true })).ok, true);
});

// ---- tabs.list ----

test("tabs.list: all tabs, browser pages masked, origin rules applied, own tabs by owner", async () => {
  const mk = (id, url, extra = {}) => ({ id, windowId: 10, url, title: `T${id}`, active: false, status: "complete", ...extra });
  const s = setup({
    tabs: {
      1: mk(1, "https://shop.test/a", { active: true, audible: true }),
      2: mk(2, "chrome://settings/passwords", { title: "Passwords" }),
      3: mk(3, "https://bank.test/login"),
      4: mk(4, "https://x.test/", { incognito: true }),
      5: mk(5, "https://chromewebstore.google.com/detail/foo"),
      6: mk(6, "", { pendingUrl: "https://shop.test/loading", status: "loading" }),
    },
    ownerOf: (id) => (id === 1 ? MONO : null),
  });
  const all = await s.call("tabs.list", { all: true, deadline_ms: 5000 });
  assert.equal(all.ok, true);
  assert.deepEqual(all.result.tabs.map((t) => t.tab), [1, 2, 5, 6]);
  assert.deepEqual(all.result.tabs[1], { tab: 2, window: 10, url: "chrome:", title: "(browser page)", owner: null, active_in_window: false, audible: false, loading: false });
  assert.equal(all.result.tabs[0].owner, MONO);
  assert.equal(all.result.tabs[0].audible, true);
  assert.equal(all.result.tabs[3].loading, true);
  assert.equal(all.result.tabs[3].url, "https://shop.test/loading");
  const own = await s.call("tabs.list", { all: false });
  assert.deepEqual(own.result.tabs.map((t) => t.tab), [1]);
  // the other Mono only sees its allowed origin, plus browser pages
  const kai = await s.call("tabs.list", { all: true }, OTHER);
  assert.deepEqual(kai.result.tabs.map((t) => t.tab), [1, 2, 5, 6]);
  assert.equal(s.calls.filter((c) => c[0] === "executeScript").length, 0, "listing never injects");
});

// ---- page.snapshot ----

const snapAnswer = (extra = {}) => ({
  text: '- heading "Shop" [level=1] [ref=e1]\n- button "Buy" [ref=e2]',
  url: "https://shop.test/a", title: "Shop", hidden_dropped: 1, controls: 1, iframes: [], visibility: "visible", ...extra,
});

test("snapshot: injects on demand, answers the contract fields, never activates anything", async () => {
  const s = setup({ content: { "1:0": (op, a) => (op === "snapshot" ? snapAnswer() : { error: "?" }) } });
  const r = await s.call("page.snapshot", { tab: 1, mode: "full", max_bytes: 60000, deadline_ms: 5000 });
  assert.equal(r.ok, true, JSON.stringify(r));
  assert.equal(r.result.text, snapAnswer().text);
  assert.equal(r.result.url, "https://shop.test/a");
  assert.equal(r.result.title, "Shop");
  assert.equal(r.result.truncated, false);
  assert.equal(r.result.hidden_dropped, 1);
  assert.equal(r.result.visibility, "visible");
  assert.equal(r.result.unrendered, undefined);
  assert.ok(Number.isInteger(r.result.generation));
  assert.ok(s.calls.some((c) => c[0] === "executeScript" && c[2] === "inject"));
  const again = await s.call("page.snapshot", { tab: 1, mode: "full" });
  assert.equal(s.calls.filter((c) => c[2] === "inject").length, 1, "injected once");
  assert.ok(again.result.generation > r.result.generation);
});

test("snapshot: paging with from and generation, stale generation or page", async () => {
  const lines = Array.from({ length: 30 }, (_, i) => `- paragraph: line number ${i}`);
  const s = setup({ content: { "1:0": () => snapAnswer({ text: lines.join("\n") }) } });
  const first = (await s.call("page.snapshot", { tab: 1, mode: "full", max_bytes: 200 })).result;
  assert.equal(first.truncated, true);
  assert.ok(new TextEncoder().encode(first.text).length <= 200);
  assert.ok(first.text.endsWith("\n"));
  let text = first.text;
  let next = first;
  while (next.truncated) {
    const res = await s.call("page.snapshot", { tab: 1, mode: "full", max_bytes: 200, from: next.next_from, generation: first.generation });
    assert.equal(res.ok, true, JSON.stringify(res));
    assert.equal(res.result.generation, first.generation);
    text += res.result.text;
    next = res.result;
  }
  assert.equal(text, lines.join("\n"));
  err(await s.call("page.snapshot", { tab: 1, mode: "full", from: 10, generation: first.generation + 1 }), "E_STALE_REF");
  err(await s.call("page.snapshot", { tab: 1, mode: "full", from: 10 }), "E_STALE_REF");
  s.tabs.get(1).url = "https://shop.test/b";
  err(await s.call("page.snapshot", { tab: 1, mode: "full", from: 10, generation: first.generation }), "E_STALE_REF");
});

test("snapshot: child frames are stitched under their iframe and refs get the frame prefix", async () => {
  const s = setup({
    frames: {
      1: [
        { frameId: 0, parentFrameId: -1, url: "https://shop.test/a" },
        { frameId: 5, parentFrameId: 0, url: "https://pay.test/frame" },
        { frameId: 6, parentFrameId: 0, url: "https://challenges.cloudflare.com/c" },
        { frameId: 7, parentFrameId: 0, url: "https://bank.test/evil" },
      ],
    },
    content: {
      "1:0": () => snapAnswer({ text: '- main:\n  - iframe [ref=e3]:\n    \u0000slot:0\u0000\n  - paragraph: after', iframes: [{ src: "https://pay.test/frame" }] }),
      "1:5": () => ({ text: '- textbox "Card" [ref=e1]: <filled 16 chars>', url: "https://pay.test/frame", title: "", hidden_dropped: 2, controls: 1, iframes: [], visibility: "visible" }),
      "1:6": () => assert.fail("a challenge frame must not be injected"),
      "1:7": () => assert.fail("a blocked frame must not be injected"),
    },
  });
  const r = await s.call("page.snapshot", { tab: 1, mode: "full" });
  assert.equal(r.ok, true, JSON.stringify(r));
  assert.equal(r.result.text, '- main:\n  - iframe [ref=e3]:\n    - textbox "Card" [ref=f5e1]: <filled 16 chars>\n  - paragraph: after');
  assert.equal(r.result.hidden_dropped, 3);
  assert.ok(!r.result.text.includes("\u0000"));
});

test("snapshot: scope_ref goes to its frame, stale refs are E_STALE_REF", async () => {
  const seen = [];
  const s = setup({
    frames: { 1: [{ frameId: 0, parentFrameId: -1, url: "https://shop.test/a" }, { frameId: 5, parentFrameId: 0, url: "https://pay.test/f" }] },
    content: {
      "1:0": () => snapAnswer(),
      "1:5": (op, a) => {
        seen.push(a);
        return a.scope === "e1" ? { text: '- textbox "Card" [ref=e1]', url: "https://pay.test/f", title: "", hidden_dropped: 0, controls: 1, iframes: [], visibility: "visible" } : { stale: true };
      },
    },
  });
  err(await s.call("page.snapshot", { tab: 1, mode: "full", scope_ref: "e1" }), "E_STALE_REF");
  const full = await s.call("page.snapshot", { tab: 1, mode: "full" });
  const scoped = await s.call("page.snapshot", { tab: 1, mode: "interactive", scope_ref: "f5e1", depth: 3 });
  assert.equal(scoped.result.text, '- textbox "Card" [ref=f5e1]');
  assert.equal(scoped.result.generation, full.result.generation);
  const scopedCall = seen.find((a) => a.scope === "e1");
  assert.equal(scopedCall.depth, 3);
  assert.equal(scopedCall.mode, "interactive");
  err(await s.call("page.snapshot", { tab: 1, mode: "full", scope_ref: "f5e9" }), "E_STALE_REF");
  err(await s.call("page.snapshot", { tab: 1, mode: "full", scope_ref: "junk" }), "E_STALE_REF");
});

test("snapshot: a hidden tab with almost no controls reports unrendered (F7)", async () => {
  const s = setup({ content: { "1:0": () => snapAnswer({ visibility: "hidden", controls: 0, text: "" }) } });
  const r = (await s.call("page.snapshot", { tab: 1, mode: "full" })).result;
  assert.equal(r.visibility, "hidden");
  assert.equal(r.unrendered, true);
  const s2 = setup({ content: { "1:0": () => snapAnswer({ visibility: "hidden", controls: 70 }) } });
  const r2 = (await s2.call("page.snapshot", { tab: 1, mode: "full" })).result;
  assert.equal(r2.visibility, "hidden");
  assert.equal(r2.unrendered, undefined);
});

test("page methods refuse restricted, blocked, closed and incognito tabs without injecting", async () => {
  const mk = (id, url, extra = {}) => ({ id, windowId: 10, url, title: "t", active: false, status: "complete", ...extra });
  const s = setup({
    tabs: { 1: mk(1, "chrome://settings"), 2: mk(2, "https://bank.test/x"), 3: mk(3, "https://x.test/", { incognito: true }), 4: mk(4, "https://chromewebstore.google.com/detail/x"), 5: mk(5, "https://other.test/") },
    content: {},
  });
  for (const method of ["page.snapshot", "page.read", "page.find", "page.screenshot", "page.wait"]) {
    const base = { query: "x" };
    err(await s.call(method, { tab: 1, mode: "full", ...base }), "E_RESTRICTED_URL");
    err(await s.call(method, { tab: 2, mode: "full", ...base }), "E_ORIGIN_BLOCKED");
    err(await s.call(method, { tab: 3, mode: "full", ...base }), "E_INCOGNITO");
    err(await s.call(method, { tab: 4, mode: "full", ...base }), "E_RESTRICTED_URL");
    err(await s.call(method, { tab: 99, mode: "full", ...base }), "E_TAB_GONE");
    err(await s.call(method, { tab: 5, mode: "full", ...base }, OTHER), "E_ORIGIN_BLOCKED");
    err(await s.call(method, { tab: "x", mode: "full", ...base }), "E_INVALID");
  }
  assert.equal(s.calls.length, 0);
});

test("a request that outlives its deadline answers E_TIMEOUT", async () => {
  const s = setup({ hang: true });
  err(await s.call("page.snapshot", { tab: 1, mode: "full", deadline_ms: 30 }), "E_TIMEOUT");
});

test("a page that cannot be scripted maps to a protocol error", async () => {
  const s = setup({ content: {} });
  err(await s.call("page.read", { tab: 1 }), "E_RESTRICTED_URL");
});

// ---- page.read ----

test("read: markdown paged by bytes from a fresh computation", async () => {
  const md = "# Title\n\nparagraph one\n\nparagraph two\n";
  const s = setup({ content: { "1:0": () => ({ markdown: md, url: "https://shop.test/a", title: "Shop", controls: 5, visibility: "visible" }) } });
  const r = await s.call("page.read", { tab: 1, max_bytes: 20 });
  assert.equal(r.ok, true, JSON.stringify(r));
  assert.equal(r.result.markdown, "# Title\n\n");
  assert.equal(r.result.truncated, true);
  assert.equal(r.result.total_bytes, md.length);
  assert.equal(r.result.url, "https://shop.test/a");
  assert.equal(r.result.visibility, "visible");
  assert.equal("text" in r.result, false);
  const rest = await s.call("page.read", { tab: 1, from: r.result.next_from, max_bytes: 1000 });
  assert.equal(r.result.markdown + rest.result.markdown, md);
  assert.equal(rest.result.truncated, false);
});

// ---- page.find ----

test("find: matches from every frame with prefixed refs, generation and page header", async () => {
  const s = setup({
    frames: { 1: [{ frameId: 0, parentFrameId: -1, url: "https://shop.test/a" }, { frameId: 5, parentFrameId: 0, url: "https://pay.test/f" }] },
    content: {
      "1:0": (op, a) => ({ matches: [{ ref: "e4", role: "button", name: "Buy", context: "Cart" }], url: "https://shop.test/a", title: "Shop" }),
      "1:5": () => ({ matches: [{ ref: "e1", role: "textbox", name: "Buy code", context: "" }], url: "u", title: "t" }),
    },
  });
  const r = await s.call("page.find", { tab: 1, query: "buy" });
  assert.equal(r.ok, true, JSON.stringify(r));
  assert.deepEqual(r.result.matches.map((m) => m.ref), ["e4", "f5e1"]);
  assert.equal(r.result.url, "https://shop.test/a");
  assert.equal(r.result.title, "Shop");
  assert.ok(Number.isInteger(r.result.generation));
  const snap = await s.call("page.snapshot", { tab: 1, mode: "full" });
  const again = await s.call("page.find", { tab: 1, query: "buy", max: 1 });
  assert.equal(again.result.generation, snap.result.generation, "find reuses the snapshot's generation");
  assert.equal(again.result.matches.length, 1);
});

// ---- page.wait ----

test("wait: text appears after a few polls; deadline gives met false, not an error", async () => {
  let polls = 0;
  const s = setup({ content: { "1:0": (op, a) => (op === "state" ? { has: ++polls >= 3, gone: true, visible: true } : {}) } });
  const r = await s.call("page.wait", { tab: 1, text: "Done", deadline_ms: 30000 });
  assert.equal(r.ok, true, JSON.stringify(r));
  assert.deepEqual(r.result, { met: true, url: "https://shop.test/a", title: "Shop" });
  assert.equal(polls, 3);
  const s2 = setup({ content: { "1:0": () => ({ has: false, gone: false, visible: true }) } });
  const t0 = s2.advance(0);
  const miss = await s2.call("page.wait", { tab: 1, text: "Never", text_gone: "x", time_ms: 2000, deadline_ms: 32000 });
  assert.equal(miss.ok, true);
  assert.equal(miss.result.met, false);
  assert.ok(s2.advance(0) - t0 >= 2000 && s2.advance(0) - t0 <= 32000 - 1000, "slept time_ms and stopped before the deadline");
});

test("wait: text_gone, plain sleep and load", async () => {
  let polls = 0;
  const s = setup({ content: { "1:0": () => ({ has: true, gone: ++polls >= 2, visible: true }) } });
  const r = await s.call("page.wait", { tab: 1, text_gone: "Spinner", deadline_ms: 30000 });
  assert.equal(r.result.met, true);
  const s2 = setup({});
  const t0 = s2.advance(0);
  const sleepOnly = await s2.call("page.wait", { tab: 1, time_ms: 1500, deadline_ms: 31500 });
  assert.equal(sleepOnly.result.met, true);
  assert.equal(s2.advance(0) - t0, 1500);
  assert.equal(s2.calls.length, 0, "a plain sleep touches no page");
  const s3 = setup({ tabs: { 1: { id: 1, windowId: 10, url: "https://shop.test/a", title: "S", active: true, status: "loading" } } });
  const loading = await s3.call("page.wait", { tab: 1, load: true, deadline_ms: 3000 });
  assert.equal(loading.result.met, false);
});

test("wait: a tab that navigates to a blocked origin stops the wait", async () => {
  const s = setup({ content: { "1:0": () => ({ has: false, gone: true, visible: true }) } });
  let n = 0;
  const get = s.api.tabs.get;
  s.api.tabs.get = async (id) => {
    const t = await get(id);
    if (++n > 2) t.url = "https://bank.test/";
    return t;
  };
  err(await s.call("page.wait", { tab: 1, text: "x", deadline_ms: 30000 }), "E_ORIGIN_BLOCKED");
});

// ---- page.screenshot ----

test("screenshot: visible active tab, one capture, url and title reported", async () => {
  const s = setup({ content: { "1:0": (op) => (op === "state" ? { visible: true } : {}) } });
  const r = await s.call("page.screenshot", { tab: 1, max_bytes: 700000 });
  assert.equal(r.ok, true, JSON.stringify(r));
  assert.equal(r.result.method, "visible");
  assert.equal(r.result.width, 1000);
  assert.equal(r.result.height, 800);
  assert.equal(r.result.scale_pct, undefined);
  assert.equal(r.result.url, "https://shop.test/a");
  assert.equal(r.result.title, "Shop");
  assert.deepEqual(pngSize(fromBase64(r.result.png_b64)), { width: 1000, height: 800 });
  assert.deepEqual(s.calls.find((c) => c[0] === "captureVisibleTab"), ["captureVisibleTab", 10, { format: "png" }]);
});

test("screenshot: a picture over max_bytes is scaled down until it fits", async () => {
  imaging.calls.length = 0;
  const s = setup({ content: { "1:0": () => ({ visible: true }) }, capture: () => dataUrl(png(2000, 1000, 400000)) });
  const r = await s.call("page.screenshot", { tab: 1, max_bytes: 100000 });
  assert.equal(r.ok, true, JSON.stringify(r));
  const bytes = fromBase64(r.result.png_b64);
  assert.ok(bytes.length <= 100000);
  assert.ok(r.result.scale_pct >= 1 && r.result.scale_pct < 100);
  assert.deepEqual(pngSize(bytes), { width: r.result.width, height: r.result.height });
  assert.ok(r.result.width < 2000);
  const hopeless = setup({ content: { "1:0": () => ({ visible: true }) }, capture: () => dataUrl(png(2000, 1000, 400000)) });
  err(await hopeless.call("page.screenshot", { tab: 1, max_bytes: 10 }), "E_TOO_LARGE");
});

test("screenshot: fitPng keeps a small picture untouched and rejects non-PNG data", async () => {
  const small = png(10, 10, 100);
  const out = await fitPng({ render: async () => assert.fail("no re-encode needed") }, small, 1000);
  assert.equal(out.bytes, small);
  await assert.rejects(fitPng(imaging, new Uint8Array(40), 1000), (e) => e.code === "E_INVALID");
});

test("screenshot: an element ref crops to its box in device pixels", async () => {
  imaging.calls.length = 0;
  const s = setup({
    content: {
      "1:0": (op, a) => {
        if (op === "state") return { visible: true };
        if (op === "snapshot") return snapAnswer();
        if (op === "rect") return a.ref === "e2" ? { x: 10, y: 20, width: 100, height: 50, viewport: { width: 1280, height: 800 }, dpr: 2 } : { stale: true };
      },
    },
  });
  err(await s.call("page.screenshot", { tab: 1, ref: "e2" }), "E_STALE_REF");
  await s.call("page.snapshot", { tab: 1, mode: "full" });
  const r = await s.call("page.screenshot", { tab: 1, ref: "e2" });
  assert.equal(r.ok, true, JSON.stringify(r));
  assert.deepEqual(imaging.calls[0].crop, { x: 20, y: 40, width: 200, height: 100 });
  err(await s.call("page.screenshot", { tab: 1, ref: "e9" }), "E_STALE_REF");
  err(await s.call("page.screenshot", { tab: 1, ref: "f3e1" }), "E_UNSUPPORTED");
});

test("screenshot: captureVisibleTab hangs on hidden pages, so a timeout answers E_UNSUPPORTED and a late result is dropped", async () => {
  let settle;
  const s = setup({ content: { "1:0": () => ({ visible: true }) }, capture: () => new Promise((r) => (settle = r)) });
  const msg = err(await s.call("page.screenshot", { tab: 1 }), "E_UNSUPPORTED");
  assert.match(msg, /background/);
  settle(dataUrl(png(10, 10, 100)));
});

test("screenshot: the active tab changing during the capture discards the picture", async () => {
  const mk = (id, active) => ({ id, windowId: 10, url: "https://shop.test/a", title: "t", active, status: "complete" });
  const s = setup({ tabs: { 1: mk(1, true), 2: mk(2, false) }, content: { "1:0": () => ({ visible: true }) } });
  s.api.tabs.captureVisibleTab = async () => {
    s.tabs.get(1).active = false;
    s.tabs.get(2).active = true;
    return dataUrl(png(10, 10, 100));
  };
  err(await s.call("page.screenshot", { tab: 1 }), "E_UNSUPPORTED");
});

test("screenshot: background tab, minimized window or hidden page without the debugger is E_UNSUPPORTED", async () => {
  const inactive = setup({ tabs: { 1: { id: 1, windowId: 10, url: "https://shop.test/a", title: "t", active: false } }, content: { "1:0": () => ({ visible: true }) } });
  assert.match(err(await inactive.call("page.screenshot", { tab: 1 }), "E_UNSUPPORTED"), /use browser_snapshot, or ask the user to show the tab/);
  const minimized = setup({ windowState: "minimized", content: { "1:0": () => ({ visible: true }) } });
  err(await minimized.call("page.screenshot", { tab: 1 }), "E_UNSUPPORTED");
  const hidden = setup({ content: { "1:0": () => ({ visible: false }) } });
  err(await hidden.call("page.screenshot", { tab: 1 }), "E_UNSUPPORTED");
  for (const s of [inactive, minimized, hidden]) assert.ok(!s.calls.some((c) => c[0] === "captureVisibleTab" || c[0] === "cdp"));
});

test("screenshot: the debugger is used only when permitted and already attached", async () => {
  const hiddenPage = { content: { "1:0": () => ({ visible: false }) } };
  const granted = setup({ ...hiddenPage, debuggerPermission: true, attached: true });
  const r = await granted.call("page.screenshot", { tab: 1 });
  assert.equal(r.ok, true, JSON.stringify(r));
  assert.equal(r.result.method, "debugger");
  assert.deepEqual(r.result.url, "https://shop.test/a");
  assert.ok(granted.calls.some((c) => c[0] === "cdp" && c[1] === "Page.captureScreenshot"));
  const noPermission = setup({ ...hiddenPage, debuggerPermission: false, attached: true });
  err(await noPermission.call("page.screenshot", { tab: 1 }), "E_UNSUPPORTED");
  const notAttached = setup({ ...hiddenPage, debuggerPermission: true, attached: false });
  err(await notAttached.call("page.screenshot", { tab: 1 }), "E_UNSUPPORTED");
  for (const s of [noPermission, notAttached]) assert.ok(!s.calls.some((c) => c[0] === "cdp"), "never attaches or sends commands on its own");
});

test("screenshot: full_page needs the attached debugger and captures beyond the viewport", async () => {
  const visible = { content: { "1:0": () => ({ visible: true }) } };
  err(await setup(visible).call("page.screenshot", { tab: 1, full_page: true }), "E_UNSUPPORTED");
  const s = setup({ ...visible, debuggerPermission: true, attached: true });
  const r = await s.call("page.screenshot", { tab: 1, full_page: true });
  assert.equal(r.result.method, "debugger");
  const cdp = s.calls.find((c) => c[0] === "cdp" && c[1] === "Page.captureScreenshot");
  assert.equal(cdp[2].captureBeyondViewport, true);
  assert.deepEqual(cdp[2].clip, { x: 0, y: 0, width: 1280, height: 5000, scale: 1 });
});

test("screenshot: captures are spaced to stay under two per second", async () => {
  const s = setup({ content: { "1:0": () => ({ visible: true }) } });
  const slept = [];
  const t0 = s.advance(0);
  await s.call("page.screenshot", { tab: 1 });
  await s.call("page.screenshot", { tab: 1 });
  assert.ok(s.advance(0) - t0 >= 500, "the second call waited for the gap");
  void slept;
});

// ---- the extension must never take the user's focus ----

test("no worker source activates a tab or focuses a window", () => {
  const dir = new URL("../src/worker/", import.meta.url);
  for (const name of readdirSync(dir)) {
    const text = readFileSync(new URL(name, dir), "utf8");
    assert.ok(!/active\s*:\s*true|focused\s*:\s*true|highlighted\s*:|drawAttention|bringToFront|activateTarget/.test(text), name);
    assert.ok(!/tabs\s*\.\s*update|windows\s*\.\s*update/.test(text.replace(/\/\/.*$/gm, "")) || name === "main.js", name);
  }
});
