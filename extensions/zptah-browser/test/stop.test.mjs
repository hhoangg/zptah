// @ts-nocheck - chrome API fakes are duck-typed test doubles.
import assert from "node:assert/strict";
import test from "node:test";
import { AMBER, badgeFor, createBadge, GREEN, RED, tooltip } from "../src/worker/badge.js";
import { createEffects } from "../src/worker/effects.js";
import { normalizeSite, parsePrefs, PREFS_KEY, siteIsOff } from "../src/worker/prefs.js";
import { createStop, overlayStopSender } from "../src/worker/stop.js";
import { describeStatus } from "../src/options/model.js";

const MIRA = "a".repeat(32);

// ---- the stop state machine ----

function stopRig(owner = MIRA) {
  const calls = [];
  const owners = new Map([[7, owner]]);
  const stop = createStop({
    ownerOf: (tab) => owners.get(tab) ?? null,
    stopTab: async (tab) => void calls.push(["stopTab", tab]),
    clearEffects: async (tab) => void calls.push(["clear", tab]),
    takeBack: async (tab) => {
      calls.push(["takeBack", tab]);
      owners.delete(tab);
    },
    emit: (m, p) => void calls.push(["emit", m, p]),
    onStopped: (tab) => calls.push(["stopped", tab]),
  });
  return { stop, calls, owners };
}

test("Stop cancels, clears, gives the tab back and tells the app, in that order", async () => {
  const { stop, calls } = stopRig();
  const r = await stop.stopTab(7);
  assert.deepEqual(r, { stopped: true });
  assert.deepEqual(calls, [
    ["stopTab", 7],
    ["clear", 7],
    ["takeBack", 7],
    ["emit", "tab.stopped", { tab: 7, mono: MIRA, by: "overlay" }],
    ["stopped", 7],
  ]);
});

test("a second click joins the stop in flight and a later one finds nobody to stop", async () => {
  const { stop, calls } = stopRig();
  const [a, b] = await Promise.all([stop.stopTab(7), stop.stopTab(7)]);
  assert.deepEqual(a, { stopped: true });
  assert.deepEqual(b, { stopped: true });
  assert.equal(calls.filter((c) => c[0] === "emit").length, 1);
  assert.deepEqual(await stop.stopTab(7), { stopped: false });
  assert.equal(calls.filter((c) => c[0] === "emit").length, 1);
  assert.equal(stop.isStopping(7), false);
});

test("a tab nobody owns cannot be stopped", async () => {
  const { stop, calls } = stopRig();
  assert.deepEqual(await stop.stopTab(99), { stopped: false });
  assert.equal(calls.length, 0);
});

test("a failing detach or page does not keep the tab from going back to the user", async () => {
  const calls = [];
  const stop = createStop({
    ownerOf: () => MIRA,
    stopTab: async () => {
      throw new Error("debugger gone");
    },
    clearEffects: async () => {
      throw new Error("no page");
    },
    takeBack: async (tab) => void calls.push(["takeBack", tab]),
    emit: (m) => void calls.push(["emit", m]),
  });
  assert.deepEqual(await stop.stopTab(3), { stopped: true });
  assert.deepEqual(calls, [["takeBack", 3], ["emit", "tab.stopped"]]);
});

test("only our own content script in the top frame of a tab can ask for Stop", () => {
  assert.equal(overlayStopSender({ id: "ext", frameId: 0, tab: { id: 5 } }, "ext"), 5);
  assert.equal(overlayStopSender({ id: "other", frameId: 0, tab: { id: 5 } }, "ext"), null);
  assert.equal(overlayStopSender({ id: "ext", frameId: 3, tab: { id: 5 } }, "ext"), null);
  assert.equal(overlayStopSender({ id: "ext", frameId: 0 }, "ext"), null);
  assert.equal(overlayStopSender({ id: "ext", frameId: 0, tab: { id: -1 } }, "ext"), null);
});

test("a stop the app could not receive is kept in session storage and sent when the link is ready", async () => {
  const store = {};
  const area = { get: async (k) => ({ [k]: store[k] }), set: async (o) => void Object.assign(store, o) };
  const sent = [];
  let up = false;
  const stop = createStop({
    ownerOf: () => MIRA,
    stopTab: async () => {},
    clearEffects: async () => {},
    takeBack: async () => {},
    emit: (m, p) => (up ? (sent.push([m, p]), true) : false),
    area,
  });
  await stop.stopTab(4);
  assert.deepEqual(sent, []);
  assert.equal(store["zptah.pending_stops"].length, 1);
  await stop.flush();
  assert.deepEqual(sent, [], "still down: kept");
  assert.equal(store["zptah.pending_stops"].length, 1);
  up = true;
  await stop.flush();
  assert.deepEqual(sent, [["tab.stopped", { tab: 4, mono: MIRA, by: "overlay" }]]);
  assert.deepEqual(store["zptah.pending_stops"], []);
  await stop.flush();
  assert.equal(sent.length, 1, "sent once");
});

// ---- the badge (spec 11.2) ----

test("badge states", () => {
  const base = { status: "ready", paired: true, stopped: false, active: 0 };
  assert.deepEqual(badgeFor(base), { text: "", color: GREEN });
  assert.deepEqual(badgeFor({ ...base, active: 2 }), { text: "2", color: GREEN });
  assert.deepEqual(badgeFor({ ...base, stopped: true, active: 2 }), { text: "■", color: RED });
  assert.deepEqual(badgeFor({ ...base, status: "disconnected" }), { text: "…", color: AMBER });
  assert.deepEqual(badgeFor({ ...base, status: "connecting" }), { text: "…", color: AMBER });
  assert.deepEqual(badgeFor({ ...base, status: "host_missing" }), { text: "…", color: AMBER });
  for (const status of ["protocol_mismatch", "auth_failed", "not_paired", "pair_failed"]) {
    assert.deepEqual(badgeFor({ ...base, status }), { text: "!", color: RED }, status);
  }
  assert.deepEqual(badgeFor({ ...base, status: "idle", paired: false }), { text: "!", color: RED });
  // Stop shows without the app too: the extension enforces it.
  assert.deepEqual(badgeFor({ ...base, status: "disconnected", stopped: true }), { text: "■", color: RED });
});

test("tooltips in both languages and the badge is written only when it changes", () => {
  assert.equal(tooltip(false, "en"), "Stop all Monos");
  assert.equal(tooltip(true, "en"), "Stopped. Resume in zptah.");
  assert.equal(tooltip(false, "vi"), "Dừng tất cả Mono");
  assert.equal(tooltip(true, "vi"), "Đã dừng. Tiếp tục trong zptah.");
  const writes = [];
  const link = { status: "ready", paired: true, stopped: false };
  let active = 0;
  const api = { action: { setBadgeText: (a) => writes.push(["text", a.text]), setBadgeBackgroundColor: (a) => writes.push(["color", a.color]), setTitle: (a) => writes.push(["title", a.title]) } };
  const badge = createBadge({ api, link, active: () => active });
  badge.update();
  badge.update();
  assert.equal(writes.length, 3);
  active = 1;
  badge.update();
  assert.deepEqual(writes.slice(3).map((w) => w[1]), ["1", GREEN, "Stop all Monos"]);
  link.stopped = true;
  badge.update();
  assert.equal(writes.at(-3)[1], "■");
});

// ---- prefs ----

test("sites without effects: exact host or wildcard subdomains", () => {
  assert.equal(normalizeSite(" Example.COM "), "example.com");
  assert.equal(normalizeSite("*.example.com"), "*.example.com");
  for (const bad of ["", "localhost", "https://example.com", "example.com/path", "*example.com", "a b.com", "exa_mple.com", 5]) {
    assert.equal(normalizeSite(bad), null, String(bad));
  }
  const list = ["shop.test", "*.bank.test"];
  assert.equal(siteIsOff(list, "https://shop.test/cart"), true);
  assert.equal(siteIsOff(list, "https://www.shop.test/cart"), false);
  assert.equal(siteIsOff(list, "https://a.bank.test/"), true);
  assert.equal(siteIsOff(list, "https://bank.test/"), false);
  assert.equal(siteIsOff(list, "https://notbank.test/"), false);
  assert.equal(siteIsOff(list, "not a url"), true, "an unreadable address gets no effects");
  assert.deepEqual(parsePrefs({ cursor: false, off_sites: ["A.com", "a.com", "bad host"] }), { cursor: false, off_sites: ["a.com"] });
  assert.deepEqual(parsePrefs(null), { cursor: true, off_sites: [] });
});

// ---- the effects of the worker ----

function effectsRig({ prefs, url = "https://shop.test/cart", attached = false, cursor = true } = {}) {
  const calls = [];
  let owner = MIRA;
  let time = 1_000_000;
  const state = { url, attached };
  const api = {
    tabs: { get: async () => ({ id: 5, url: state.url }) },
    scripting: {
      executeScript: async (inj) => {
        if (inj.files) return [{ result: 1 }];
        if (!inj.args) return [{ result: 1 }];
        const [op, args] = inj.args;
        calls.push([op, args, inj.target.frameIds[0]]);
        if (op === "act_point") return [{ result: { x: 40, y: 50, name: "Buy now" } }];
        if (op === "cursor_move") return [{ result: { shown: true } }];
        return [{ result: {} }];
      },
    },
    storage: { onChanged: { addListener() {} } },
  };
  const fx = createEffects({
    api,
    now: () => time,
    sleep: async () => {},
    ownerOf: () => owner,
    area: { get: async () => ({ [PREFS_KEY]: prefs ?? {} }) },
    debuggerAttached: () => state.attached,
    lang: () => "en",
  });
  const entry = { id: MIRA, name: "Mira", emoji: "🐸", color: "#d93025", cursor, level: "act", allowed_origins: [], blocked_origins: ["https://bank.test"] };
  const rec = { tab: 5, frameId: 0, id: "p1", action: { kind: "click", ref: "e3" } };
  return { fx, calls, entry, rec, state, advance: (ms) => (time += ms), setOwner: (o) => (owner = o) };
}

const ops = (calls) => calls.map((c) => c[0]);

test("before an act the overlay is shown and the cursor moves to the target with the element's name", async () => {
  const r = effectsRig();
  await r.fx.beforeAct(r.entry, r.rec, "key");
  assert.deepEqual(ops(r.calls), ["overlay_show", "act_point", "cursor_move"]);
  const move = r.calls[2][1];
  assert.deepEqual({ x: move.x, y: move.y, color: move.color, name: move.name, emoji: move.emoji, kind: move.kind, target: move.target, click: move.click }, { x: 40, y: 50, color: "#d93025", name: "Mira", emoji: "🐸", kind: "click", target: "Buy now", click: true });
  assert.equal(r.fx.activeMonos(), 1);
});

test("typed text is never given to the cursor label", async () => {
  const r = effectsRig();
  await r.fx.beforeAct(r.entry, { ...r.rec, action: { kind: "type", ref: "e1", text: "my secret" } }, "k");
  const move = r.calls.find((c) => c[0] === "cursor_move")[1];
  assert.equal(move.target, undefined);
  assert.equal(JSON.stringify(r.calls).includes("my secret"), false);
});

test("the Mono's own cursor switch, the global switch and the site list each turn the cursor off", async () => {
  const off = async (r) => {
    await r.fx.beforeAct(r.entry, r.rec, "k");
    return ops(r.calls);
  };
  assert.deepEqual(await off(effectsRig({ cursor: false })), ["overlay_show"], "Mono switch: the glow and Stop stay");
  assert.deepEqual(await off(effectsRig({ prefs: { cursor: false } })), ["overlay_show"], "global switch");
  assert.deepEqual(await off(effectsRig({ prefs: { off_sites: ["shop.test"] } })), [], "site off: neither");
});

test("no effects on restricted pages, blocked sites or for an action with no target", async () => {
  assert.deepEqual(ops((await (async () => { const r = effectsRig({ url: "chrome://settings" }); await r.fx.beforeAct(r.entry, r.rec, "k"); return r; })()).calls), []);
  assert.deepEqual(ops((await (async () => { const r = effectsRig({ url: "https://bank.test/" }); await r.fx.beforeAct(r.entry, r.rec, "k"); return r; })()).calls), []);
  const r = effectsRig();
  await r.fx.beforeAct(r.entry, { ...r.rec, action: { kind: "press", key: "Enter" } }, "k");
  assert.deepEqual(ops(r.calls), ["overlay_show"]);
});

test("the run ending removes the overlay, unless the debugger is still attached", async () => {
  const r = effectsRig({ attached: true });
  await r.fx.beforeAct(r.entry, r.rec, "k");
  r.calls.length = 0;
  await r.fx.taskEnd(MIRA);
  assert.deepEqual(r.calls, [], "the debugger keeps it");
  assert.equal(r.fx.has(5), true);
  r.state.attached = false;
  r.advance(10 * 60_000);
  await r.fx.sweep();
  assert.deepEqual(ops(r.calls), ["cursor_hide", "overlay_hide"]);
  assert.equal(r.fx.has(5), false);
});

test("a navigation draws the overlay again; a tab that changed hands does not", async () => {
  const r = effectsRig();
  await r.fx.beforeAct(r.entry, r.rec, "k");
  r.calls.length = 0;
  await r.fx.reapply(5);
  assert.deepEqual(ops(r.calls), ["overlay_show"]);
  r.calls.length = 0;
  r.setOwner(null);
  await r.fx.reapply(5);
  assert.deepEqual(r.calls, []);
});

test("effects are hidden around a screenshot and restored even when it fails", async () => {
  const r = effectsRig();
  await r.fx.beforeAct(r.entry, r.rec, "k");
  r.calls.length = 0;
  await assert.rejects(r.fx.around(5, async () => Promise.reject(new Error("no picture"))), /no picture/);
  assert.deepEqual(r.calls.map((c) => [c[0], c[1].on]), [["effects_suspend", true], ["effects_suspend", false]]);
  r.calls.length = 0;
  assert.equal(await r.fx.around(99, async () => "x"), "x");
  assert.deepEqual(r.calls, [], "a tab without effects is not touched");
});

test("an effect that throws never fails the action", async () => {
  const r = effectsRig();
  // A broken page: the script cannot run at all.
  const fx = createEffects({
    api: { tabs: { get: async () => { throw new Error("gone"); } }, scripting: { executeScript: async () => { throw new Error("no"); } } },
    now: Date.now, sleep: async () => {}, ownerOf: () => MIRA, area: { get: async () => ({}) }, debuggerAttached: () => false,
  });
  await assert.doesNotReject(fx.beforeAct(r.entry, r.rec, "k"));
});

// ---- options page text ----

test("the options page says what is wrong and what to do", () => {
  const ok = describeStatus({ status: "ready", paired: true, protocol: 1, app_version: "0.9.1", browser: { brand: "Chrome", version: "158" }, instance_id: "0123456789abcdef" });
  assert.equal(ok.connected, true);
  assert.match(ok.text, /Connected to zptah \(0\.9\.1\)\. Paired as Chrome, id 01234567\. Protocol 1\./);
  assert.match(describeStatus({ status: "idle", paired: false, protocol: 1 }).advice, /Settings > Browser > Connect extension/);
  assert.match(describeStatus({ status: "protocol_mismatch", paired: true, protocol: 1 }).text, /different versions/);
  assert.match(describeStatus({ status: "disconnected", paired: true, protocol: 1 }).text, /not running/);
  assert.match(describeStatus({ status: "ready", paired: true, protocol: 1, stopped: true }).text, /Stopped/);
});
