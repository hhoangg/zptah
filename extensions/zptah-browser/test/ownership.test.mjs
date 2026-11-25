// @ts-nocheck - chrome API fakes and a fake link are duck-typed test doubles.
import assert from "node:assert/strict";
import test from "node:test";
import { createFrontGuard, RESTORE_WINDOW_MS } from "../src/worker/front-guard.js";
import { groupTitle } from "../src/worker/groups.js";
import { registerOwnership } from "../src/worker/ownership.js";
import { registerReadPath } from "../src/worker/readpath.js";

const MIRA = "a".repeat(32);
const KAI = "b".repeat(32);

const entry = (id, name, emoji, extra = {}) => ({
  id,
  name,
  emoji,
  color: "#d93025",
  group_color: "red",
  level: "act",
  allowed_origins: [],
  blocked_origins: ["https://bank.test"],
  cursor: true,
  ...extra,
});

function event() {
  const listeners = [];
  return { addListener: (fn) => listeners.push(fn), fire: (...a) => listeners.forEach((fn) => fn(...a)) };
}

/** A small stateful Chrome: tabs, windows and tab groups, with the events the worker listens to. */
function chrome(opts = {}) {
  const tabs = new Map();
  const groups = new Map();
  const calls = [];
  let nextTab = 100;
  let nextGroup = 500;
  const ev = {
    tabsUpdated: event(),
    tabsCreated: event(),
    tabsRemoved: event(),
    groupsRemoved: event(),
    navTarget: event(),
    activated: event(),
    focusChanged: event(),
  };
  const add = (t) => {
    tabs.set(t.id, { windowId: 1, index: tabs.size, active: false, status: "complete", groupId: -1, ...t });
    return tabs.get(t.id);
  };
  for (const t of opts.tabs ?? []) add(t);
  const storage = new Map();
  const api = {
    tabs: {
      get: async (id) => {
        if (!tabs.has(id)) throw new Error(`No tab with id: ${id}.`);
        return { ...tabs.get(id) };
      },
      query: async (q = {}) => [...tabs.values()].filter((t) => (q.active === undefined || t.active === q.active)).map((t) => ({ ...t })),
      create: async (props) => {
        calls.push(["tabs.create", props]);
        const t = add({ id: nextTab++, url: props.url, title: "New", windowId: props.windowId, index: props.index ?? tabs.size, active: props.active ?? false });
        return { ...t };
      },
      remove: async (id) => {
        calls.push(["tabs.remove", id]);
        tabs.delete(id);
      },
      group: async ({ tabIds, groupId, createProperties }) => {
        calls.push(["tabs.group", { tabIds, groupId, createProperties }]);
        const gid = groupId ?? nextGroup++;
        if (groupId === undefined) groups.set(gid, { id: gid, windowId: createProperties.windowId, title: "", color: "grey" });
        for (const id of tabIds) tabs.get(id).groupId = gid;
        return gid;
      },
      ungroup: async (id) => {
        calls.push(["tabs.ungroup", id]);
        if (tabs.has(id)) tabs.get(id).groupId = -1;
      },
      update: async (id, props) => {
        calls.push(["tabs.update", id, props]);
        if (props.active) for (const t of tabs.values()) if (t.windowId === tabs.get(id).windowId) t.active = t.id === id;
        return { ...tabs.get(id) };
      },
      onUpdated: ev.tabsUpdated,
      onCreated: ev.tabsCreated,
      onRemoved: ev.tabsRemoved,
      onActivated: ev.activated,
    },
    tabGroups: {
      get: async (id) => {
        if (!groups.has(id)) throw new Error("No group");
        return { ...groups.get(id) };
      },
      update: async (id, props) => {
        calls.push(["tabGroups.update", id, props]);
        Object.assign(groups.get(id), props);
      },
      query: async () => [...groups.values()].map((g) => ({ ...g })),
      onRemoved: ev.groupsRemoved,
    },
    windows: {
      get: async (id) => ({ id, incognito: opts.incognito ?? false, type: "normal" }),
      getLastFocused: async () => (opts.noWindow ? null : { id: opts.focusedWindow ?? 1, incognito: opts.incognito ?? false }),
      create: async () => assert.fail("must never create a window"),
      update: async (id, props) => {
        calls.push(["windows.update", id, props]);
      },
      onFocusChanged: ev.focusChanged,
    },
    webNavigation: { onCreatedNavigationTarget: ev.navTarget },
    storage: { session: { get: async (k) => (storage.has(k) ? { [k]: storage.get(k) } : {}), set: async (o) => Object.entries(o).forEach(([k, v]) => storage.set(k, structuredClone(v))) } },
    permissions: { contains: async () => false },
  };
  return { api, tabs, groups, calls, ev, add, storage };
}

function setup(opts = {}) {
  const chrome_ = chrome(opts);
  const events = [];
  const link = {
    handlers: new Map(),
    stopped: false,
    sendEvent: (method, params) => {
      events.push([method, params]);
      return true;
    },
    roster: opts.roster ?? [entry(MIRA, "Mira", "🐸"), entry(KAI, "Kai", "🐱", { group_color: "blue" })],
  };
  let own;
  const read = registerReadPath(link, {
    api: chrome_.api,
    ownerOf: (t) => own?.ownerOf(t) ?? null,
    onRoster: () => void own?.syncRoster(),
    groupTitle: (m) => own?.titleOfMono(m),
  });
  const front = { opened: 0, opening() { this.opened += 1; } };
  own = registerOwnership(link, { read, api: chrome_.api, now: () => opts.clock?.() ?? Date.now(), front });
  const call = async (method, params, mono = MIRA) => link.handlers.get(method)({ v: 1, type: "req", id: "a-1", method, mono, params });
  const wait = () => new Promise((r) => setTimeout(r, 5));
  return { ...chrome_, link, own, read, call, events, front, wait };
}

const USER_TABS = [
  { id: 1, url: "https://mail.test/", title: "Inbox", windowId: 1, active: true },
  { id: 2, url: "https://sheets.test/d/1", title: "Report", windowId: 1 },
  { id: 3, url: "https://bank.test/", title: "Bank", windowId: 1 },
];

const err = (res, code) => {
  assert.equal(res.ok, false, JSON.stringify(res));
  assert.equal(res.error.code, code, JSON.stringify(res));
  return res.error.message;
};

// ---- group titles ----

test("a group is titled with the emoji and the Mono's name, at most 40 characters", () => {
  const roster = [entry(MIRA, "Mira", "🐸"), entry(KAI, "K".repeat(60), "🐱")];
  assert.equal(groupTitle(roster[0], roster), "🐸 Mira");
  const long = groupTitle(roster[1], roster);
  assert.ok(Array.from(long).length <= 40, long);
  assert.ok(long.startsWith("🐱 KKK"));
});

test("two Monos with the same title get a numeric suffix in the later one", () => {
  const roster = [entry(MIRA, "Mira", "🐸"), entry(KAI, "Mira", "🐸"), entry("c".repeat(32), "Mira", "🐸")];
  assert.deepEqual(roster.map((e) => groupTitle(e, roster)), ["🐸 Mira", "🐸 Mira 2", "🐸 Mira 3"]);
  const wide = [entry(MIRA, "M".repeat(60), "🐸"), entry(KAI, "M".repeat(60), "🐸")];
  assert.ok(Array.from(groupTitle(wide[1], wide)).length <= 40);
  assert.ok(groupTitle(wide[1], wide).endsWith(" 2"));
});

// ---- tab.open ----

test("tab.open opens a background tab in the user's window, groups it and makes the Mono its owner", async () => {
  const s = setup({ tabs: USER_TABS });
  const res = await s.call("tab.open", { url: "https://shop.test/cart" });
  assert.equal(res.ok, true, JSON.stringify(res));
  const info = res.result;
  assert.equal(info.owner, MIRA);
  assert.equal(info.window, 1);
  assert.equal(info.group_title, "🐸 Mira");
  const create = s.calls.find((c) => c[0] === "tabs.create")[1];
  assert.equal(create.active, false, "never in front");
  assert.equal(create.windowId, 1);
  const group = s.calls.find((c) => c[0] === "tabs.group")[1];
  assert.deepEqual(group.createProperties, { windowId: 1 }, "an explicit window");
  const update = s.calls.find((c) => c[0] === "tabGroups.update");
  assert.deepEqual(update[2], { title: "🐸 Mira", color: "red" });
  assert.equal(s.own.ownerOf(info.tab), MIRA);
  // The second tab of the same Mono reuses the group.
  await s.call("tab.open", { url: "https://shop.test/b" });
  const groupsMade = s.calls.filter((c) => c[0] === "tabs.group" && c[1].createProperties);
  assert.equal(groupsMade.length, 1);
  assert.ok(s.calls.some((c) => c[0] === "tabs.group" && c[1].groupId !== undefined));
  // tabs.list shows the owner and the group title.
  const list = await s.call("tabs.list", { all: false });
  assert.equal(list.result.tabs.length, 2);
  assert.equal(list.result.tabs[0].group_title, "🐸 Mira");
});

test("tab.open goes to the window of after_tab and right after it, and refuses bad addresses", async () => {
  const s = setup({ tabs: [...USER_TABS, { id: 9, url: "https://x.test/", title: "X", windowId: 7, index: 4 }] });
  const first = await s.call("tab.open", { url: "https://shop.test/a" });
  s.own.claim(9, MIRA, "adopted");
  const second = await s.call("tab.open", { url: "https://shop.test/b", after_tab: 9 });
  assert.equal(second.result.window, 7);
  const create = s.calls.filter((c) => c[0] === "tabs.create")[1][1];
  assert.equal(create.index, 5);
  assert.ok(first.ok);
  err(await s.call("tab.open", { url: "ftp://x.test/" }), "E_INVALID");
  err(await s.call("tab.open", { url: "chrome://settings" }), "E_INVALID");
  err(await s.call("tab.open", { url: "https://bank.test/login" }), "E_ORIGIN_BLOCKED");
  err(await s.call("tab.open", { url: "https://chromewebstore.google.com/detail/x" }), "E_RESTRICTED_URL");
});

test("tab.open never creates a window: no normal window is an error, and incognito is refused", async () => {
  err(await setup({ noWindow: true }).call("tab.open", { url: "https://shop.test/" }), "E_UNSUPPORTED");
  err(await setup({ incognito: true }).call("tab.open", { url: "https://shop.test/" }), "E_INCOGNITO");
});

test("unknown Monos and a stopped browser get their errors", async () => {
  const s = setup({ tabs: USER_TABS });
  err(await s.call("tab.open", { url: "https://shop.test/" }, "c".repeat(32)), "E_UNKNOWN_MONO");
  s.link.stopped = true;
  err(await s.call("tab.open", { url: "https://shop.test/" }), "E_STOPPED");
});

// ---- close and release ----

test("only the owner closes or releases a tab", async () => {
  const s = setup({ tabs: USER_TABS });
  const mine = (await s.call("tab.open", { url: "https://shop.test/a" })).result.tab;
  err(await s.call("tab.close", { tab: 1 }), "E_NOT_OWNER");
  err(await s.call("tab.release", { tab: 1 }), "E_NOT_OWNER");
  err(await s.call("tab.close", { tab: mine }, KAI), "E_NOT_OWNER");
  assert.ok(s.tabs.has(mine));

  assert.equal((await s.call("tab.release", { tab: mine })).ok, true);
  assert.equal(s.own.ownerOf(mine), null);
  assert.equal(s.tabs.get(mine).groupId, -1, "ungrouped, still open");
  err(await s.call("tab.close", { tab: mine }), "E_NOT_OWNER");

  const second = (await s.call("tab.open", { url: "https://shop.test/b" })).result.tab;
  assert.equal((await s.call("tab.close", { tab: second })).ok, true);
  assert.equal(s.tabs.has(second), false);
  assert.equal(s.own.ownerOf(second), null);
});

test("a read only Mono can open a tab but not close, release or adopt", async () => {
  const roster = [entry(MIRA, "Mira", "🐸", { level: "read" })];
  const s = setup({ tabs: USER_TABS, roster });
  assert.equal((await s.call("tab.open", { url: "https://shop.test/a" })).ok, true);
  const mine = s.own.snapshot()[0].tab;
  err(await s.call("tab.close", { tab: mine }), "E_UNSUPPORTED");
  err(await s.call("tab.release", { tab: mine }), "E_UNSUPPORTED");
  err(await s.call("tab.adopt", { tab: 2, grant: { kind: "user_reference", quote: "the report sheet", message_ms: 1 } }), "E_UNSUPPORTED");
});

// ---- adopt ----

const reference = (quote, ms = 10_000) => ({ kind: "user_reference", quote, message_ms: ms });

test("adopting a user tab needs the user's reference and puts the tab in the Mono's group", async () => {
  const s = setup({ tabs: USER_TABS });
  err(await s.call("tab.adopt", { tab: 2 }), "E_NOT_OWNER");
  err(await s.call("tab.adopt", { tab: 2, grant: { kind: "user_approved", approval_id: "x" } }), "E_NOT_OWNER");
  err(await s.call("tab.adopt", { tab: 2, grant: reference("report") }), "E_NOT_OWNER");
  assert.equal(s.own.ownerOf(2), null);
  const res = await s.call("tab.adopt", { tab: 2, grant: reference("the report sheet") });
  assert.equal(res.ok, true, JSON.stringify(res));
  assert.equal(res.result.owner, MIRA);
  assert.equal(s.own.ownerOf(2), MIRA);
  assert.ok(s.tabs.get(2).groupId >= 500, "moved into a group");
  assert.equal(s.groups.get(s.tabs.get(2).groupId).title, "🐸 Mira");
  // Already hers: answers with the tab.
  assert.equal((await s.call("tab.adopt", { tab: 2, grant: reference("the report sheet") })).ok, true);
  // Origin rules still apply.
  err(await s.call("tab.adopt", { tab: 3, grant: reference("the bank tab") }), "E_ORIGIN_BLOCKED");
});

test("a tab of another Mono moves only with the user's approval", async () => {
  const s = setup({ tabs: USER_TABS });
  s.own.claim(2, KAI, "adopted");
  err(await s.call("tab.adopt", { tab: 2, grant: reference("the report sheet") }), "E_NOT_OWNER");
  assert.equal(s.own.ownerOf(2), KAI);
  // The approval alone is not enough: the user's words come with it.
  err(await s.call("tab.adopt", { tab: 2, grant: { kind: "user_approved", approval_id: "0123" } }), "E_NOT_OWNER");
  err(await s.call("tab.adopt", { tab: 2, grant: { kind: "user_approved", approval_id: "0123", quote: "x", message_ms: 5 } }), "E_NOT_OWNER");
  const res = await s.call("tab.adopt", { tab: 2, grant: { kind: "user_approved", approval_id: "0123", quote: "the report sheet", message_ms: 5 } });
  assert.equal(res.ok, true, JSON.stringify(res));
  assert.equal(s.own.ownerOf(2), MIRA);
  // Kai lost it: acting is refused by the owner check the act path uses.
  assert.notEqual(s.read.ownerOf(2), KAI);
});

test("a tab the user took back needs a reference written after the stop", async () => {
  let clock = 1_000_000;
  const s = setup({ tabs: USER_TABS, clock: () => clock });
  await s.call("tab.adopt", { tab: 2, grant: reference("the report sheet", clock - 10) });
  clock += 100;
  s.own.takeBack(2);
  assert.equal(s.own.takenBackAt(2), clock);
  clock += 100;
  err(await s.call("tab.adopt", { tab: 2, grant: reference("the report sheet", clock - 150) }), "E_NOT_OWNER");
  assert.equal((await s.call("tab.adopt", { tab: 2, grant: reference("the report sheet", clock + 1) })).ok, true);
  assert.equal(s.own.takenBackAt(2), null);
});

// ---- the user's drags ----

async function mineWithGroup(s) {
  const info = (await s.call("tab.open", { url: "https://shop.test/a" })).result;
  const tab = s.tabs.get(info.tab);
  // Chrome reports the group change the worker made itself; that consumes the worker's mark.
  s.ev.tabsUpdated.fire(info.tab, { groupId: tab.groupId });
  await s.wait();
  s.events.length = 0;
  return { tab: info.tab, group: tab.groupId };
}

test("the worker's own moves are not the user's drags, even when Chrome reports them early", async () => {
  const s = setup({ tabs: USER_TABS });
  const { tab, group } = await mineWithGroup(s);
  assert.equal(s.own.ownerOf(tab), MIRA);
  assert.deepEqual(s.events, []);
  // The report of a move arrives before the worker has finished it: still not a drag.
  const info = (await s.call("tab.open", { url: "https://shop.test/b" })).result;
  s.ev.tabsUpdated.fire(info.tab, { groupId: group });
  await s.wait();
  assert.equal(s.own.ownerOf(info.tab), MIRA);
  assert.deepEqual(s.events, []);
});

test("a tab dragged out of the group is taken back and the app is told", async () => {
  const s = setup({ tabs: USER_TABS });
  const { tab } = await mineWithGroup(s);
  s.ev.tabsUpdated.fire(tab, { groupId: -1 });
  await s.wait();
  assert.equal(s.own.ownerOf(tab), null);
  assert.ok(s.own.takenBackAt(tab) > 0);
  assert.deepEqual(s.events, [["tab.ownership", { tab, owner: null, by: "user_ungroup" }]]);
  // Dragged into some group of the user's: also taken back, as a drag.
  const second = await mineWithGroup(s);
  s.ev.tabsUpdated.fire(second.tab, { groupId: 9999 });
  await s.wait();
  assert.deepEqual(s.events.at(-1), ["tab.ownership", { tab: second.tab, owner: null, by: "user_drag" }]);
});

test("a tab dragged into a Mono's group is adopted by the user without a prompt, from the user or another Mono", async () => {
  const s = setup({ tabs: USER_TABS });
  const { group } = await mineWithGroup(s);
  s.ev.tabsUpdated.fire(2, { groupId: group });
  await s.wait();
  assert.equal(s.own.ownerOf(2), MIRA);
  assert.deepEqual(s.events.at(-1), ["tab.ownership", { tab: 2, owner: MIRA, by: "user_drag" }]);
  // Kai's tab dragged into Mira's group changes hands: the user decides.
  s.own.claim(3, KAI, "adopted");
  s.ev.tabsUpdated.fire(3, { groupId: group });
  await s.wait();
  assert.equal(s.own.ownerOf(3), MIRA);
  // A user-taken-back tab dragged back is adopted again at once.
  s.own.takeBack(2);
  s.ev.tabsUpdated.fire(2, { groupId: group });
  await s.wait();
  assert.equal(s.own.ownerOf(2), MIRA);
  assert.equal(s.own.takenBackAt(2), null);
});

test("closing the group takes its tabs back and a closed tab is forgotten", async () => {
  const s = setup({ tabs: USER_TABS });
  const { tab, group } = await mineWithGroup(s);
  s.ev.groupsRemoved.fire({ id: group });
  await s.wait();
  assert.equal(s.own.ownerOf(tab), null);
  assert.deepEqual(s.events, [["tab.ownership", { tab, owner: null, by: "group_closed" }]]);
  assert.ok(s.own.takenBackAt(tab) > 0, "a take back: the old reference does not adopt it again");
  const second = await mineWithGroup(s);
  s.ev.tabsRemoved.fire(second.tab);
  assert.equal(s.own.ownerOf(second.tab), null);
});

// ---- popups ----

test("a popup an owned tab opens is owned by the same Mono and put in its group", async () => {
  const s = setup({ tabs: USER_TABS });
  const { tab, group } = await mineWithGroup(s);
  s.add({ id: 300, url: "https://pay.test/", title: "Pay", windowId: 1 });
  s.ev.navTarget.fire({ tabId: 300, sourceTabId: tab });
  await s.wait();
  assert.equal(s.own.ownerOf(300), MIRA);
  assert.equal(s.tabs.get(300).groupId, group);
  assert.deepEqual(s.events.at(-1), ["tab.ownership", { tab: 300, owner: MIRA, by: "popup" }]);
  assert.equal(s.front.opened, 1);
  // A tab the user's own tab opens stays the user's.
  s.add({ id: 301, url: "https://other.test/", title: "O", windowId: 1 });
  s.ev.navTarget.fire({ tabId: 301, sourceTabId: 1 });
  await s.wait();
  assert.equal(s.own.ownerOf(301), null);
});

// ---- the roster ----

test("roster.set renames the groups and returns the tabs of a removed Mono to the user", async () => {
  const s = setup({ tabs: USER_TABS });
  const { tab, group } = await mineWithGroup(s);
  const renamed = [entry(MIRA, "Mira R", "🦄", { group_color: "green" })];
  assert.equal((await s.link.handlers.get("roster.set")({ id: "r1", params: { monos: renamed } })).ok, true);
  await s.wait();
  assert.deepEqual([s.groups.get(group).title, s.groups.get(group).color], ["🦄 Mira R", "green"]);
  assert.equal(s.own.ownerOf(tab), MIRA);
  // The Mono leaves the roster: its tab is the user's again, ungrouped and still open.
  await s.link.handlers.get("roster.set")({ id: "r2", params: { monos: [entry(KAI, "Kai", "🐱")] } });
  await s.wait();
  await s.wait();
  assert.equal(s.own.ownerOf(tab), null);
  assert.equal(s.tabs.get(tab).groupId, -1);
  assert.ok(s.tabs.has(tab));
});

test("a restored group that matches a roster entry is offered back but its tabs stay the user's", async () => {
  const s = setup({ tabs: USER_TABS });
  s.groups.set(700, { id: 700, windowId: 1, title: "🐸 Mira", color: "red" });
  s.tabs.get(2).groupId = 700;
  await s.own.syncRoster();
  assert.equal(s.own.ownerOf(2), null, "nothing is re-attached");
  assert.equal(s.own.groups.monoOf(700), MIRA);
  // The Mono's next tab goes into that group instead of a new one.
  const info = (await s.call("tab.open", { url: "https://shop.test/a" })).result;
  assert.equal(s.tabs.get(info.tab).groupId, 700);
  assert.equal(s.calls.filter((c) => c[0] === "tabs.group" && c[1].createProperties).length, 0);
});

test("the table survives a worker restart in session storage", async () => {
  const s = setup({ tabs: USER_TABS });
  const { tab } = await mineWithGroup(s);
  await s.own.ready;
  await s.wait();
  // A second worker over the same storage.
  const link = { handlers: new Map(), stopped: false, roster: s.link.roster, sendEvent: () => true };
  const read = registerReadPath(link, { api: s.api });
  const again = registerOwnership(link, { read, api: s.api });
  await again.ready;
  assert.equal(again.ownerOf(tab), MIRA);
});

// ---- the focus guard ----

/** A fake browser for the front guard: tabs 1 (window 1, the user's) and 50 (a popup, window 2). */
function fronts(clock, opts = {}) {
  const calls = [];
  const onActivated = event();
  const onFocusChanged = event();
  const api = {
    tabs: {
      query: async (q) => (q.lastFocusedWindow ? [{ id: 1, windowId: 1 }] : [{ id: 1, windowId: 1 }]),
      get: async (id) => ({ id, windowId: id === 50 ? (opts.popupWindow ?? 2) : 1 }),
      update: async (id, props) => void calls.push(["tab", id, props]),
      onActivated,
    },
    windows: { update: async (id, props) => void calls.push(["window", id, props]), onFocusChanged },
  };
  const guard = createFrontGuard({ api, now: () => clock.t, ownerOf: (t) => (opts.owned?.includes(t) ? MIRA : null) });
  return { api, guard, calls, onActivated, onFocusChanged };
}
const tick = () => new Promise((r) => setTimeout(r, 5));

test("the popup took the front before the guard heard of it: the user's tab and window come back", async () => {
  const clock = { t: 0 };
  const f = fronts(clock);
  await tick();
  // The real order: Chrome activates the popup, then the navigation target event arrives.
  clock.t = 100;
  f.onActivated.fire({ tabId: 50, windowId: 2 });
  f.onFocusChanged.fire(2);
  clock.t = 150;
  await f.guard.opening(50);
  await tick();
  assert.deepEqual(f.calls, [["tab", 1, { active: true }], ["window", 1, { focused: true }]]);
  // Once: the restore's own activation changes nothing more.
  f.onActivated.fire({ tabId: 1, windowId: 1 });
  await tick();
  assert.equal(f.calls.length, 2);
});

test("the popup takes the front after the opening event: restored when it does", async () => {
  const clock = { t: 0 };
  const f = fronts(clock);
  await tick();
  await f.guard.opening(50);
  clock.t = 400;
  f.onActivated.fire({ tabId: 50, windowId: 2 });
  await tick();
  assert.deepEqual(f.calls, [["tab", 1, { active: true }], ["window", 1, { focused: true }]]);
});

test("a tab a Mono owns is not a place to go back to", async () => {
  const clock = { t: 0 };
  const f = fronts(clock, { owned: [7] });
  await tick();
  f.onActivated.fire({ tabId: 7, windowId: 1 }); // the Mono's own tab was in front when it opened the popup
  f.onActivated.fire({ tabId: 50, windowId: 2 });
  await f.guard.opening(50);
  await tick();
  assert.deepEqual(f.calls[0], ["tab", 1, { active: true }]);
});

test("a switch the user makes by hand is never undone", async () => {
  const clock = { t: 0 };
  const f = fronts(clock);
  await tick();
  await f.guard.opening(50);
  clock.t = 300;
  // The user goes to another tab and window on purpose, inside the 2 s.
  f.onActivated.fire({ tabId: 8, windowId: 3 });
  f.onFocusChanged.fire(3);
  f.onActivated.fire({ tabId: 9, windowId: 1 });
  await tick();
  assert.deepEqual(f.calls, []);
  assert.deepEqual(f.guard.current(), { windowId: 1, tabId: 9 });
});

test("a window focus change is paired with that window's own tab", async () => {
  const clock = { t: 0 };
  const f = fronts(clock);
  await tick();
  f.onActivated.fire({ tabId: 20, windowId: 5 });
  f.onActivated.fire({ tabId: 1, windowId: 1 });
  f.onFocusChanged.fire(5);
  assert.deepEqual(f.guard.current(), { windowId: 5, tabId: 20 });
});

test("nothing is restored when nothing was opened or after 2 s", async () => {
  for (const scenario of ["never opened", "too late"]) {
    const clock = { t: 0 };
    const f = fronts(clock);
    await tick();
    if (scenario === "too late") await f.guard.opening(50);
    clock.t = scenario === "too late" ? RESTORE_WINDOW_MS + 1 : 100;
    f.onActivated.fire({ tabId: 50, windowId: 2 });
    await tick();
    assert.deepEqual(f.calls, [], scenario);
  }
});

test("the guard exposes no handlers of its own", () => {
  const f = fronts({ t: 0 });
  assert.deepEqual(Object.keys(f.guard).sort(), ["current", "opening"]);
});
