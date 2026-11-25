// @ts-nocheck - chrome API fakes, a fake link and a hand-made DOM are duck-typed test doubles.
// The real content bundle runs in a vm over the fake DOM, so these tests cover the worker and
// the content script together.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { bundleContent } from "../scripts/build.mjs";
import { registerActPath } from "../src/worker/act.js";
import { b64encode } from "../src/worker/bytes.js";
import { createDebugger, isAllowedCdp } from "../src/worker/debugger.js";
import { focusGuard } from "../src/worker/focus-guard.js";
import { Lexicon } from "../src/worker/lexicon.js";
import { registerReadPath } from "../src/worker/readpath.js";
import { createWorld } from "./helpers/fake-dom.mjs";
import { validateAgainst } from "./helpers/schema.mjs";

const MONO = "a".repeat(32);
const OTHER = "b".repeat(32);
const shared = (n) => JSON.parse(readFileSync(new URL(`../shared/${n}`, import.meta.url), "utf8"));
const lexicon = new Lexicon(shared("risk-lexicon.json"), shared("risk-sites.json"));
const SCHEMA = shared("protocol.json");
const bundle = bundleContent(new URL("../src/content", import.meta.url).pathname);

/** Checkout page with a card field, a total and a pay button. */
function checkout(h) {
  return [
    h("h1", { rect: [10, 10, 200, 30] }, "Checkout"),
    h("form", { id: "payform", action: "/pay", method: "post" },
      h("label", { for: "card" }, "Card number"),
      h("input", { id: "card", name: "cardnumber", type: "text", autocomplete: "cc-number", rect: [10, 60, 200, 30] }),
      h("div", { class: "total", rect: [10, 100, 200, 20] }, "Total ₫189,000"),
      h("button", { id: "pay", type: "submit", rect: [10, 130, 120, 36] }, "Pay ₫189,000")),
    h("a", { id: "help", href: "/help", rect: [10, 200, 60, 20] }, "Help"),
  ];
}

function setup(opts = {}) {
  const world = createWorld(opts.url ?? "https://shop.test/checkout");
  const { h } = world;
  for (const k of (opts.build ?? checkout)(h, world)) world.document.body.appendChild(k);
  world.context.globalThis = world.context;
  world.context.top = world.context;
  world.context.parent = world.context;
  world.context.frameElement = null;
  vm.createContext(world.context);
  vm.runInContext(bundle, world.context);
  const calls = [];
  const events = [];
  let clock = 1_000_000;
  const listeners = { removed: [], committed: [], dbgEvent: [], dbgDetach: [], clicked: [], added: [] };
  const tabs = new Map([[1, { id: 1, windowId: 10, url: world.location.href, title: "Checkout", active: true, status: "complete" }]]);
  const hang = new Set(opts.hangOps ?? []);
  const api = {
    tabs: {
      get: async (id) => {
        if (!tabs.has(id)) throw new Error(`No tab with id: ${id}.`);
        return { ...tabs.get(id) };
      },
      query: async () => [...tabs.values()],
      update: async (id, props) => {
        calls.push(["tabs.update", id, props]);
        const t = tabs.get(id);
        if (props.url) {
          t.url = props.url;
          world.location = new URL(props.url);
          world.context.location = world.location;
          world.url = props.url;
        }
        return t;
      },
      goBack: async (id) => {
        calls.push(["goBack", id]);
        tabs.get(id).url = "https://shop.test/previous";
      },
      goForward: async (id) => calls.push(["goForward", id]),
      onRemoved: { addListener: (f) => listeners.removed.push(f) },
    },
    windows: { get: async () => ({ state: "normal" }), update: async () => assert.fail("never update a window") },
    webNavigation: {
      getAllFrames: async () => [{ frameId: 0, parentFrameId: -1, url: world.location.href }],
      onCommitted: { addListener: (f) => listeners.committed.push(f) },
    },
    scripting: {
      executeScript: async ({ target, func, files, args }) => {
        const frameId = target.frameIds[0];
        if (files) return [{ frameId, result: undefined }];
        if (!args) return [{ frameId, result: world.context.zptahContent?.version ?? 0 }];
        if (hang.has(args[0])) return new Promise(() => {});
        // Chrome structured-clones the result out of the page's realm.
        const out = await world.context.zptahContent.run(args[0], args[1]);
        return [{ frameId, result: out === undefined ? undefined : JSON.parse(JSON.stringify(out)) }];
      },
    },
    permissions: { contains: async () => opts.debuggerPermission ?? false, onAdded: { addListener: (f) => listeners.added.push(f) } },
    debugger: {
      attach: async (target, v) => {
        calls.push(["dbg.attach", target.tabId, v]);
        if (opts.attachError) throw new Error(opts.attachError);
      },
      detach: async (target) => calls.push(["dbg.detach", target.tabId]),
      sendCommand: async (target, method, params) => {
        calls.push(["cdp", method, params]);
        if (opts.cdp) return opts.cdp(method, params, world);
        return {};
      },
      onEvent: { addListener: (f) => listeners.dbgEvent.push(f) },
      onDetach: { addListener: (f) => listeners.dbgDetach.push(f) },
    },
    action: { onClicked: { addListener: (f) => listeners.clicked.push(f) } },
  };
  const link = {
    handlers: new Map(),
    stopped: false,
    roster: opts.roster ?? [
      { id: MONO, name: "Mira", allowed_origins: opts.allowed ?? [], blocked_origins: opts.blocked ?? ["https://bank.test"], level: opts.level ?? "act" },
      { id: OTHER, name: "Kai", allowed_origins: [], blocked_origins: [], level: "act" },
    ],
    sendEvent: (method, params) => {
      events.push({ method, params });
      return true;
    },
  };
  // A fake clock; real timers stay short so the vm's own timers keep working.
  const sleep = async (ms) => {
    clock += ms;
    await new Promise((r) => setTimeout(r, ms >= 3000 ? 400 : 1));
  };
  const owner = { value: opts.owner === undefined ? MONO : opts.owner };
  const read = registerReadPath(link, { api, now: () => clock, sleep, ownerOf: () => owner.value, imaging: { render: async () => assert.fail("no imaging") } });
  // Inert timers: the idle sweep is driven by hand so the process can exit.
  const dbg = createDebugger({ api, now: () => clock, sleep, emit: (m, p) => link.sendEvent(m, p), setTimer: () => 0, clearTimer: () => {}, isStopped: () => link.stopped });
  const acts = registerActPath(link, { api, now: () => clock, sleep, read, dbg, lexicon: opts.noLexicon ? null : lexicon });
  const call = async (method, params, mono = MONO) => link.handlers.get(method)({ v: 1, type: "req", id: "a-1", method, mono, params });
  return { world, api, link, call, calls, events, tabs, acts, dbg, read, listeners, owner, advance: (ms) => (clock += ms), now: () => clock, h };
}

const refOf = (text, role, name) => {
  const m = new RegExp(`- ${role} ${JSON.stringify(name).replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}[^\\n]*\\[ref=(e\\d+)\\]`).exec(text);
  assert.ok(m, `no ${role} ${name} in\n${text}`);
  return m[1];
};
const snap = async (s) => (await s.call("page.snapshot", { tab: 1, mode: "full" })).result.text;
const err = (res, code) => {
  assert.equal(res.ok, false, JSON.stringify(res));
  assert.equal(res.error.code, code, JSON.stringify(res));
  return res.error.message;
};
const ok = (res) => {
  assert.equal(res.ok, true, JSON.stringify(res));
  return res.result;
};
const prep = async (s, action, extra = {}) => ok(await s.call("act.prepare", { tab: 1, action, ...extra }));
const finish = (s) => s.world.stop();

// ---- contract ----

test("prepare returns the signals of the Rust types, valid against protocol.json, and matches the golden sample", async () => {
  const s = setup();
  const text = await snap(s);
  const p = await prep(s, { kind: "click", ref: refOf(text, "button", "Pay ₫189,000") });
  validateAgainst(SCHEMA, "Prepared", p);
  assert.match(p.prepare_id, /^[0-9a-f]{32}$/);
  assert.equal(p.expires_ms, s.now() + 30000);
  assert.deepEqual(p.ext_risk, ["payment"]);
  assert.equal(p.digest.length, 64);
  const golden = shared("act-samples.json");
  assert.deepEqual(p.element, golden.element);
  assert.deepEqual(p.form, golden.form);
  finish(s);
});

test("dialog signals: in_dialog, dialog_text, primary buttons and markers match the golden sample", async () => {
  const s = setup({
    url: "https://shop.test/account",
    build: (h) => [
      h("div", { id: "onetrust-banner-sdk", class: "ot-sdk-container", role: "dialog" },
        h("p", {}, "Delete your account permanently?"),
        h("button", { id: "cancel", type: "button", rect: [10, 100, 80, 30] }, "Cancel"),
        h("button", { id: "confirm", type: "button", class: "btn-primary", rect: [100, 100, 80, 30] }, "Delete account")),
      h("nav", {}, h("a", { id: "menu", href: "/settings", rect: [10, 10, 60, 20] }, "Settings")),
    ],
  });
  const text = await snap(s);
  const p = await prep(s, { kind: "click", ref: refOf(text, "button", "Cancel") });
  validateAgainst(SCHEMA, "Prepared", p);
  assert.deepEqual(p.element, shared("act-samples.json").element_in_dialog);
  const link = await prep(s, { kind: "click", ref: refOf(text, "link", "Settings") });
  assert.equal(link.element.in_landmark_menu, true);
  assert.equal(link.element.href, "https://shop.test/settings");
  finish(s);
});

test("recipients, conversation title and visibility are reported and match the golden form sample", async () => {
  const s = setup({
    url: "https://mail.test/compose",
    build: (h) => [
      h("main", {}, h("h1", {}, "New message"),
        h("form", { id: "compose", action: "/send", method: "post" },
          h("input", { id: "to", name: "to", type: "email", value: "an@example.com, binh@example.com", rect: [10, 40, 200, 30] }),
          h("select", { id: "aud", name: "audience", "aria-label": "Visibility", rect: [10, 80, 100, 30] },
            h("option", { value: "p" }, "Public"), h("option", { value: "f", selected: "" }, "Friends")),
          h("button", { id: "send", type: "submit", rect: [10, 120, 80, 30] }, "Send"))),
    ],
  });
  const text = await snap(s);
  const p = await prep(s, { kind: "click", ref: refOf(text, "button", "Send") });
  assert.deepEqual(p.form, shared("act-samples.json").form_with_recipients);
  validateAgainst(SCHEMA, "FormSignals", p.form);
  finish(s);
});

// ---- prepare and commit ----

test("commit refuses the action without approval when ext_risk is not empty, and runs it with a matching one", async () => {
  const s = setup({ debuggerPermission: false });
  let paid = 0;
  s.world.document.querySelector("#pay").addEventListener("click", () => {
    paid += 1;
    s.world.document.body.appendChild(s.world.h("p", {}, "Thanks"));
  });
  const text = await snap(s);
  const p = await prep(s, { kind: "click", ref: refOf(text, "button", "Pay ₫189,000") });
  err(await s.call("act.commit", { prepare_id: p.prepare_id }), "E_NEEDS_APPROVAL");
  err(await s.call("act.commit", { prepare_id: p.prepare_id, approval: { approval_id: "x", categories: ["sharing"], digest: p.digest } }), "E_NEEDS_APPROVAL");
  err(await s.call("act.commit", { prepare_id: p.prepare_id, approval: { approval_id: "x", categories: ["payment"], digest: "0".repeat(64) } }), "E_STALE_PREPARE");
  assert.equal(paid, 0, "nothing ran");
  // A refused commit that did not touch the page keeps nothing alive after E_STALE_PREPARE, so prepare again.
  const p2 = await prep(s, { kind: "click", ref: refOf(text, "button", "Pay ₫189,000") });
  const done = ok(await s.call("act.commit", { prepare_id: p2.prepare_id, approval: { approval_id: "x", categories: ["payment"], digest: p2.digest } }));
  assert.equal(paid, 1);
  assert.equal(done.effects.method, "content");
  assert.ok(done.effects.dom_mutations > 0);
  assert.equal(done.url, "https://shop.test/checkout");
  validateAgainst(SCHEMA, "Effects", done.effects);
  err(await s.call("act.commit", { prepare_id: p2.prepare_id, approval: { approval_id: "x", categories: ["payment"], digest: p2.digest } }), "E_STALE_PREPARE");
  finish(s);
});

test("untrusted click events carry the full sequence and activate through element.click()", async () => {
  const s = setup({ build: (h) => [h("button", { id: "b", type: "button", rect: [10, 10, 80, 30] }, "Next")] });
  const text = await snap(s);
  const p = await prep(s, { kind: "click", ref: refOf(text, "button", "Next") });
  s.world.log.length = 0;
  ok(await s.call("act.commit", { prepare_id: p.prepare_id }));
  const order = s.world.log.filter((e) => e.id === "b").map((e) => e.type);
  assert.deepEqual(order.slice(0, 12), ["pointerover", "pointerenter", "mouseover", "mouseenter", "pointermove", "mousemove", "pointerdown", "mousedown", "pointerup", "mouseup", "click"].slice(0, 12));
  assert.ok(order.includes("click"));
  finish(s);
});

test("a changed amount, name, address, generation or an expired prepare is E_STALE_PREPARE", async () => {
  const mk = async () => {
    const s = setup({ build: (h) => [h("p", { id: "total", rect: [10, 10, 100, 20] }, "Total ₫100"), h("button", { id: "b", type: "button", rect: [10, 40, 80, 30] }, "Next")] });
    const text = await snap(s);
    return { s, p: await prep(s, { kind: "click", ref: refOf(text, "button", "Next") }) };
  };
  // the digest moves when the text next to the button changes
  let { s, p } = await mk();
  s.world.document.querySelector("#total").childNodes[0].data = "Total ₫999";
  err(await s.call("act.commit", { prepare_id: p.prepare_id }), "E_STALE_PREPARE");
  finish(s);
  // the accessible name changes
  ({ s, p } = await mk());
  s.world.document.querySelector("#b").childNodes[0].data = "Delete";
  err(await s.call("act.commit", { prepare_id: p.prepare_id }), "E_STALE_PREPARE");
  finish(s);
  // the element moves a lot
  ({ s, p } = await mk());
  s.world.document.querySelector("#b")._rect = [10, 400, 80, 30];
  err(await s.call("act.commit", { prepare_id: p.prepare_id }), "E_STALE_PREPARE");
  finish(s);
  // the element is removed
  ({ s, p } = await mk());
  const b = s.world.document.querySelector("#b");
  b.parentNode.removeChild(b);
  err(await s.call("act.commit", { prepare_id: p.prepare_id }), "E_STALE_PREPARE");
  finish(s);
  // another address
  ({ s, p } = await mk());
  s.tabs.get(1).url = "https://shop.test/elsewhere";
  err(await s.call("act.commit", { prepare_id: p.prepare_id }), "E_STALE_PREPARE");
  finish(s);
  // a new snapshot (new generation)
  ({ s, p } = await mk());
  await snap(s);
  err(await s.call("act.commit", { prepare_id: p.prepare_id }), "E_STALE_PREPARE");
  finish(s);
  // expired after 30 s
  ({ s, p } = await mk());
  s.advance(31000);
  err(await s.call("act.commit", { prepare_id: p.prepare_id }), "E_STALE_PREPARE");
  err(await s.call("act.commit", { prepare_id: "nope" }), "E_STALE_PREPARE");
  finish(s);
});

test("a small move or scroll of the element is tolerated; extend keeps a prepare alive up to the approval timeout; cancel ends it", async () => {
  const s = setup({ build: (h) => [h("button", { id: "b", type: "button", rect: [10, 40, 80, 30] }, "Next")] });
  const text = await snap(s);
  const p = await prep(s, { kind: "click", ref: refOf(text, "button", "Next") });
  s.world.document.querySelector("#b")._rect = [14, 46, 80, 30];
  s.advance(25000);
  const ext = ok(await s.call("act.extend", { prepare_id: p.prepare_id, until_ms: s.now() + 300000 }));
  assert.equal(ext.expires_ms, s.now() + 300000);
  const capped = ok(await s.call("act.extend", { prepare_id: p.prepare_id, until_ms: s.now() + 9_999_999 }));
  assert.equal(capped.expires_ms, p.expires_ms - 30000 + 600000);
  s.advance(200000);
  ok(await s.call("act.commit", { prepare_id: p.prepare_id }));
  const q = await prep(s, { kind: "click", ref: refOf(text, "button", "Next") });
  ok(await s.call("act.cancel", { prepare_id: q.prepare_id }));
  err(await s.call("act.commit", { prepare_id: q.prepare_id }), "E_STALE_PREPARE");
  ok(await s.call("act.cancel", { prepare_id: "unknown" }));
  finish(s);
});

test("a prepare of another Mono is unknown to this one", async () => {
  const s = setup({ build: (h) => [h("button", { id: "b", type: "button", rect: [10, 40, 80, 30] }, "Next")] });
  const text = await snap(s);
  const p = await prep(s, { kind: "click", ref: refOf(text, "button", "Next") });
  err(await s.call("act.commit", { prepare_id: p.prepare_id }, OTHER), "E_STALE_PREPARE");
  finish(s);
});

// ---- the input paths ----

function formPage(h) {
  return [
    h("form", { id: "f", action: "/search", method: "get" },
      h("label", { for: "q" }, "Search"),
      h("input", { id: "q", name: "q", type: "text", rect: [10, 10, 200, 30] }),
      h("input", { id: "agree", type: "checkbox", "aria-label": "Agree", rect: [10, 50, 20, 20] }),
      h("select", { id: "size", name: "size", "aria-label": "Size", rect: [10, 80, 100, 30] },
        h("option", { value: "s" }, "Small"), h("option", { value: "l" }, "Large")),
      h("button", { id: "go", type: "submit", rect: [10, 120, 80, 30] }, "Search")),
  ];
}

test("type uses the native value setter and the input events, reads the value back, and submits with requestSubmit", async () => {
  const s = setup({ build: formPage, url: "https://shop.test/" });
  const text = await snap(s);
  const p = await prep(s, { kind: "type", ref: refOf(text, "textbox", "Search"), text: "áo dài", submit: true });
  s.world.log.length = 0;
  const done = ok(await s.call("act.commit", { prepare_id: p.prepare_id }));
  const q = s.world.document.querySelector("#q");
  assert.equal(q.value, "áo dài");
  assert.equal(q.nativeSets, 1, "set through the prototype setter");
  const types = s.world.log.filter((e) => e.id === "q").map((e) => e.type);
  assert.deepEqual(types.filter((t) => ["beforeinput", "input", "change"].includes(t)), ["beforeinput", "input", "change"]);
  assert.deepEqual(s.world.document.querySelector("#f").submits, ["implicit"]);
  assert.equal(done.effects.value_matches, true);
  assert.equal(done.effects.submitted, true);
  assert.equal(done.effects.focus_changed, false);
  finish(s);
});

test("type slowly sends one keystroke per character", async () => {
  const s = setup({ build: formPage, url: "https://shop.test/" });
  const text = await snap(s);
  const p = await prep(s, { kind: "type", ref: refOf(text, "textbox", "Search"), text: "abc", slowly: true });
  s.world.log.length = 0;
  ok(await s.call("act.commit", { prepare_id: p.prepare_id }));
  assert.equal(s.world.document.querySelector("#q").value, "abc");
  assert.equal(s.world.log.filter((e) => e.id === "q" && e.type === "keydown").length, 3);
  finish(s);
});

test("a field outside a form: the result says to prepare the site's button as its own click", async () => {
  const s = setup({ build: (h) => [h("input", { id: "q", type: "text", "aria-label": "Find", rect: [10, 10, 200, 30] })], url: "https://shop.test/" });
  const text = await snap(s);
  const p = await prep(s, { kind: "type", ref: refOf(text, "textbox", "Find"), text: "x", submit: true });
  const done = ok(await s.call("act.commit", { prepare_id: p.prepare_id }));
  assert.equal(done.effects.submitted, false);
  assert.match(done.effects.hint, /submit button as its own click/);
  finish(s);
});

test("fill sets text, checkbox and select values; select picks an option by its text", async () => {
  const s = setup({ build: formPage, url: "https://shop.test/" });
  const text = await snap(s);
  const p = await prep(s, { kind: "fill", fields: [
    { ref: refOf(text, "textbox", "Search"), value: "kem", type: "text" },
    { ref: refOf(text, "checkbox", "Agree"), value: "true", type: "checkbox" },
  ] });
  ok(await s.call("act.commit", { prepare_id: p.prepare_id }));
  assert.equal(s.world.document.querySelector("#q").value, "kem");
  assert.equal(s.world.document.querySelector("#agree").checked, true);
  const sel = await prep(s, { kind: "select", ref: refOf(text, "combobox", "Size"), values: ["Large"] });
  ok(await s.call("act.commit", { prepare_id: sel.prepare_id }));
  assert.equal(s.world.document.querySelector("#size").value, "l");
  const bad = await prep(s, { kind: "select", ref: refOf(text, "combobox", "Size"), values: ["Huge"] });
  err(await s.call("act.commit", { prepare_id: bad.prepare_id }), "E_INVALID");
  finish(s);
});

test("press: scroll keys scroll, Enter in a form requestSubmits, Tab moves focus; other keys need the debugger", async () => {
  const s = setup({ build: formPage, url: "https://shop.test/" });
  const text = await snap(s);
  const q = refOf(text, "textbox", "Search");
  let p = await prep(s, { kind: "press", key: "PageDown" });
  ok(await s.call("act.commit", { prepare_id: p.prepare_id }));
  assert.equal(s.world.scrolled.length, 1);
  p = await prep(s, { kind: "press", key: "Enter", ref: q });
  ok(await s.call("act.commit", { prepare_id: p.prepare_id }));
  assert.deepEqual(s.world.document.querySelector("#f").submits, ["implicit"]);
  p = await prep(s, { kind: "press", key: "Escape", ref: q });
  err(await s.call("act.commit", { prepare_id: p.prepare_id }), "E_DEBUGGER_DENIED");
  finish(s);
});

test("a press that needs the debugger uses it when the permission is granted", async () => {
  const s = setup({ build: formPage, url: "https://shop.test/", debuggerPermission: true });
  const text = await snap(s);
  const p = await prep(s, { kind: "press", key: "Control+Shift+K", ref: refOf(text, "textbox", "Search") });
  ok(await s.call("act.commit", { prepare_id: p.prepare_id }));
  const keys = s.calls.filter((c) => c[0] === "cdp" && c[1] === "Input.dispatchKeyEvent");
  assert.equal(keys.length, 2);
  assert.equal(keys[0][2].modifiers, 2 | 8);
  assert.deepEqual(s.dbg.attachedTabs(), [1]);
  finish(s);
});

test("hover fires the hover events and scroll scrolls", async () => {
  const s = setup({ build: (h) => [h("div", { id: "m", role: "button", "aria-label": "Menu", rect: [10, 10, 80, 30] })], url: "https://shop.test/" });
  const text = await snap(s);
  const p = await prep(s, { kind: "hover", ref: refOf(text, "button", "Menu") });
  s.world.log.length = 0;
  ok(await s.call("act.commit", { prepare_id: p.prepare_id }));
  assert.ok(s.world.log.some((e) => e.type === "mouseenter" && e.id === "m"));
  const sc = await prep(s, { kind: "scroll", dx: 0, dy: 300 });
  ok(await s.call("act.commit", { prepare_id: sc.prepare_id }));
  assert.deepEqual(s.world.scrolled.at(-1), [0, 300]);
  finish(s);
});

test("decoy inputs (readonly, hidden) are refused and a covered control is not clicked", async () => {
  const s = setup({
    build: (h) => [h("input", { id: "decoy", type: "text", readonly: "", "aria-label": "Search", rect: [10, 10, 200, 30] }), h("button", { id: "b", type: "button", rect: [10, 60, 80, 30] }, "Next")],
    url: "https://shop.test/",
  });
  const text = await snap(s);
  err(await s.call("act.prepare", { tab: 1, action: { kind: "type", ref: refOf(text, "textbox", "Search"), text: "x" } }), "E_INVALID");
  const p = await prep(s, { kind: "click", ref: refOf(text, "button", "Next") });
  s.world.hitOverride = s.world.h("div", { id: "overlay" }, "Accept cookies");
  const msg = err(await s.call("act.commit", { prepare_id: p.prepare_id }), "E_INVALID");
  assert.match(msg, /covered/);
  finish(s);
});

// ---- passwords and one-time codes ----

test("typing into a password field is refused at prepare, whatever the approval (E_PASSWORD_FIELD)", async () => {
  const s = setup({
    build: (h) => [
      h("input", { id: "pw", type: "password", "aria-label": "Password", rect: [10, 10, 200, 30] }),
      h("input", { id: "pw2", type: "text", "aria-label": "Mật khẩu", rect: [10, 50, 200, 30] }),
      h("input", { id: "pw3", type: "text", autocomplete: "current-password", "aria-label": "Secret", rect: [10, 90, 200, 30] }),
    ],
    url: "https://shop.test/login",
  });
  const text = await snap(s);
  for (const name of ["Password", "Mật khẩu", "Secret"]) {
    err(await s.call("act.prepare", { tab: 1, action: { kind: "type", ref: refOf(text, "textbox", name), text: "hunter2" } }), "E_PASSWORD_FIELD");
  }
  err(await s.call("act.prepare", { tab: 1, action: { kind: "fill", fields: [{ ref: refOf(text, "textbox", "Password"), value: "x", type: "text" }] } }), "E_PASSWORD_FIELD");
  finish(s);
});

test("a one-time code field asks (credential_entry) instead of being blocked", async () => {
  const s = setup({ build: (h) => [h("input", { id: "otp", type: "text", autocomplete: "one-time-code", "aria-label": "Code", rect: [10, 10, 200, 30] })], url: "https://shop.test/login" });
  const text = await snap(s);
  const p = await prep(s, { kind: "type", ref: refOf(text, "textbox", "Code"), text: "123456" });
  assert.deepEqual(p.ext_risk, ["credential_entry"]);
  err(await s.call("act.commit", { prepare_id: p.prepare_id }), "E_NEEDS_APPROVAL");
  finish(s);
});

test("a key that types text while a password field has focus is refused before any key or text is sent", async () => {
  const s = setup({
    build: (h) => [h("input", { id: "pw", type: "password", "aria-label": "Password", rect: [10, 10, 200, 30] }), h("input", { id: "u", type: "text", "aria-label": "User", rect: [10, 50, 200, 30] })],
    url: "https://shop.test/login",
    debuggerPermission: true,
  });
  const text = await snap(s);
  const doc = s.world.document;
  // Text for the user field: the prepared field gets the focus first, so nothing is refused.
  const p = await prep(s, { kind: "type", ref: refOf(text, "textbox", "User"), text: "an" });
  doc.activeElement = doc.querySelector("#pw");
  ok(await s.call("act.commit", { prepare_id: p.prepare_id, input: "trusted" }));
  assert.ok(s.calls.some((c) => c[0] === "cdp" && c[1] === "Input.insertText"));
  // A key press without a ref goes to whatever has focus. A prepare while the password field has focus is refused.
  doc.activeElement = doc.querySelector("#pw");
  err(await s.call("act.prepare", { tab: 1, action: { kind: "press", key: "a" } }), "E_PASSWORD_FIELD");
  // Focus moving into the password field after the checks is caught right before the key is sent.
  doc.activeElement = doc.querySelector("#u");
  const q = await prep(s, { kind: "press", key: "a" });
  const ctx = s.world.context.zptahContent;
  const realRun = ctx.run;
  ctx.run = (op, args) => {
    if (op === "act_active") doc.activeElement = doc.querySelector("#pw");
    return realRun(op, args);
  };
  s.calls.length = 0;
  err(await s.call("act.commit", { prepare_id: q.prepare_id, input: "trusted" }), "E_PASSWORD_FIELD");
  assert.ok(!s.calls.some((c) => c[0] === "cdp" && /Input\.(dispatchKeyEvent|insertText)/.test(c[1])), "no key or text was sent");
  // Enter is not typing: it is allowed with the password field focused.
  ctx.run = realRun;
  doc.activeElement = doc.querySelector("#u");
  const e = await prep(s, { kind: "press", key: "Enter" });
  ok(await s.call("act.commit", { prepare_id: e.prepare_id, input: "trusted" }));
  finish(s);
});

// ---- the debugger ----

test("the CDP allowlist refuses everything else before chrome.debugger is called", async () => {
  const s = setup({ debuggerPermission: true });
  await s.dbg.attach(1);
  s.calls.length = 0;
  for (const m of ["Runtime.evaluate", "Page.navigate", "Page.addScriptToEvaluateOnNewDocument", "Page.bringToFront", "Target.activateTarget", "Network.enable", "Fetch.enable", "Storage.getCookies", "Browser.close", "DOM.getDocument", "Emulation.setUserAgentOverride", "Page.getLayoutMetrics"]) {
    await assert.rejects(() => s.dbg.send(1, m, {}), (e) => e.code === "E_DEBUGGER_DENIED", m);
    assert.equal(isAllowedCdp(m), false, m);
  }
  assert.ok(!s.calls.some((c) => c[0] === "cdp"), "nothing reached sendCommand");
  await assert.rejects(() => s.dbg.send(1, "Emulation.setFocusEmulationEnabled", { enabled: "yes" }), (e) => e.code === "E_DEBUGGER_DENIED");
  for (const m of ["Input.dispatchMouseEvent", "Input.dispatchKeyEvent", "Input.dispatchDragEvent", "Page.captureScreenshot", "Page.enable"]) {
    assert.equal(isAllowedCdp(m), true, m);
    await s.dbg.send(1, m, {});
  }
  await s.dbg.send(1, "Input.insertText", { text: "x" });
  await s.dbg.send(1, "Page.handleJavaScriptDialog", { accept: true });
  await s.dbg.send(1, "Emulation.setFocusEmulationEnabled", { enabled: true });
  finish(s);
});

test("the debugger is attached once per tab, only with the permission, and busy maps to E_DEBUGGER_BUSY", async () => {
  let s = setup({ debuggerPermission: false });
  await assert.rejects(() => s.dbg.attach(1), (e) => e.code === "E_DEBUGGER_DENIED");
  s = setup({ debuggerPermission: true });
  assert.equal(await s.dbg.attach(1), true);
  assert.equal(await s.dbg.attach(1), false);
  assert.equal(s.calls.filter((c) => c[0] === "dbg.attach").length, 1);
  s = setup({ debuggerPermission: true, attachError: "Another debugger is already attached to the tab with id: 1." });
  await assert.rejects(() => s.dbg.attach(1), (e) => e.code === "E_DEBUGGER_BUSY");
  finish(s);
});

test("a click that did nothing keeps its prepare for one trusted retry through the debugger", async () => {
  const s = setup({ build: (h) => [h("button", { id: "b", type: "button", rect: [10, 40, 80, 30] }, "Next")], debuggerPermission: true, url: "https://shop.test/" });
  const text = await snap(s);
  const p = await prep(s, { kind: "click", ref: refOf(text, "button", "Next") });
  const first = ok(await s.call("act.commit", { prepare_id: p.prepare_id }));
  assert.equal(first.effects.dom_mutations, 0);
  assert.match(first.effects.hint, /input: trusted/);
  err(await s.call("act.commit", { prepare_id: p.prepare_id }), "E_STALE_PREPARE");
  const p2 = await prep(s, { kind: "click", ref: refOf(await snap(s), "button", "Next") });
  ok(await s.call("act.commit", { prepare_id: p2.prepare_id }));
  const retry = ok(await s.call("act.commit", { prepare_id: p2.prepare_id, input: "trusted" }));
  assert.equal(retry.effects.method, "debugger");
  const mouse = s.calls.filter((c) => c[0] === "cdp" && c[1] === "Input.dispatchMouseEvent").map((c) => c[2].type);
  assert.deepEqual(mouse, ["mouseMoved", "mousePressed", "mouseReleased"]);
  // centre of [10,40,80,30]
  const pressed = s.calls.find((c) => c[0] === "cdp" && c[2]?.type === "mousePressed")[2];
  assert.deepEqual([pressed.x, pressed.y], [50, 55]);
  err(await s.call("act.commit", { prepare_id: p2.prepare_id, input: "trusted" }), "E_STALE_PREPARE");
  // Once the debugger is used on a tab it is used for the rest of the task.
  const p3 = await prep(s, { kind: "click", ref: refOf(await snap(s), "button", "Next") });
  s.calls.length = 0;
  const third = ok(await s.call("act.commit", { prepare_id: p3.prepare_id }));
  assert.equal(third.effects.method, "debugger");
  finish(s);
});

test("a trusted retry is refused when the page changed after the first try", async () => {
  const s = setup({ build: (h) => [h("button", { id: "b", type: "button", rect: [10, 40, 80, 30] }, "Next")], debuggerPermission: true, url: "https://shop.test/" });
  const text = await snap(s);
  const p = await prep(s, { kind: "click", ref: refOf(text, "button", "Next") });
  ok(await s.call("act.commit", { prepare_id: p.prepare_id }));
  await new Promise((r) => setTimeout(r, 20));
  s.world.document.body.appendChild(s.world.h("p", {}, "late banner"));
  await new Promise((r) => setTimeout(r, 20));
  err(await s.call("act.commit", { prepare_id: p.prepare_id, input: "trusted" }), "E_STALE_PREPARE");
  finish(s);
});

test("trusted input without the permission is E_DEBUGGER_DENIED", async () => {
  const s = setup({ build: (h) => [h("button", { id: "b", type: "button", rect: [10, 40, 80, 30] }, "Next")], url: "https://shop.test/" });
  const p = await prep(s, { kind: "click", ref: refOf(await snap(s), "button", "Next") });
  err(await s.call("act.commit", { prepare_id: p.prepare_id, input: "trusted" }), "E_DEBUGGER_DENIED");
  finish(s);
});

test("drag needs the debugger and moves the mouse from one element to the other", async () => {
  const build = (h) => [h("div", { id: "a", role: "button", "aria-label": "Card A", rect: [10, 10, 40, 40] }), h("div", { id: "b", role: "button", "aria-label": "Column B", rect: [200, 10, 40, 40] })];
  let s = setup({ build, url: "https://shop.test/" });
  let text = await snap(s);
  err(await s.call("act.prepare", { tab: 1, action: { kind: "drag", from_ref: refOf(text, "button", "Card A"), to_ref: refOf(text, "button", "Column B") } }), "E_DEBUGGER_DENIED");
  s = setup({ build, url: "https://shop.test/", debuggerPermission: true });
  text = await snap(s);
  const p = await prep(s, { kind: "drag", from_ref: refOf(text, "button", "Card A"), to_ref: refOf(text, "button", "Column B") });
  ok(await s.call("act.commit", { prepare_id: p.prepare_id }));
  const mouse = s.calls.filter((c) => c[0] === "cdp" && c[1] === "Input.dispatchMouseEvent").map((c) => [c[2].type, c[2].x]);
  assert.equal(mouse[1][0], "mousePressed");
  assert.equal(mouse.at(-1)[0], "mouseReleased");
  assert.equal(mouse.at(-1)[1], 220);
  finish(s);
});

test("destructive and payment prepares pre-attach the debugger and measure after the infobar settled", async () => {
  const build = (h) => [h("button", { id: "d", type: "button", rect: [10, 10, 120, 30] }, "Delete account"), h("button", { id: "n", type: "button", rect: [10, 60, 80, 30] }, "Next")];
  const s = setup({ build, url: "https://shop.test/", debuggerPermission: true });
  const text = await snap(s);
  const safe = await prep(s, { kind: "click", ref: refOf(text, "button", "Next") });
  assert.equal(safe.debugger_attached, false);
  assert.ok(!s.calls.some((c) => c[0] === "dbg.attach"));
  const p = await prep(s, { kind: "click", ref: refOf(text, "button", "Delete account") });
  assert.deepEqual(p.ext_risk, ["destructive"]);
  assert.equal(p.debugger_attached, true);
  assert.deepEqual(s.calls.filter((c) => c[0] === "dbg.attach" || c[1] === "Page.enable").map((c) => c[0]), ["dbg.attach", "cdp"]);
  // Without the permission the prepare still works.
  const t = setup({ build, url: "https://shop.test/", debuggerPermission: false });
  const q = await prep(t, { kind: "click", ref: refOf(await snap(t), "button", "Delete account") });
  assert.equal(q.debugger_attached, false);
  // A busy tab: the prepare continues without the debugger.
  const u = setup({ build, url: "https://shop.test/", debuggerPermission: true, attachError: "Another debugger is already attached" });
  const r = await prep(u, { kind: "click", ref: refOf(await snap(u), "button", "Delete account") });
  assert.equal(r.debugger_attached, false);
  for (const x of [s, t, u]) finish(x);
});

test("the debugger detaches 2 minutes after the last use, and use resets the timer", async () => {
  const s = setup({ debuggerPermission: true });
  await s.dbg.attach(1);
  s.advance(100000);
  await s.dbg.sweep();
  assert.deepEqual(s.dbg.attachedTabs(), [1]);
  s.dbg.touch(1);
  s.advance(100000);
  await s.dbg.sweep();
  assert.deepEqual(s.dbg.attachedTabs(), [1], "use reset the timer");
  s.advance(21000);
  await s.dbg.sweep();
  assert.deepEqual(s.dbg.attachedTabs(), []);
  assert.ok(s.calls.some((c) => c[0] === "dbg.detach"));
  assert.deepEqual(s.events.at(-1), { method: "debugger.detached", params: { tab: 1, reason: "task_ended" } });
  finish(s);
});

test("Stop detaches every session, drops prepares and tells the app", async () => {
  const s = setup({ debuggerPermission: true, build: (h) => [h("button", { id: "b", type: "button", rect: [10, 40, 80, 30] }, "Next")], url: "https://shop.test/" });
  const p = await prep(s, { kind: "click", ref: refOf(await snap(s), "button", "Next") });
  await s.dbg.attach(1);
  s.listeners.clicked[0]();
  await new Promise((r) => setTimeout(r, 30));
  assert.equal(s.link.stopped, true);
  assert.deepEqual(s.dbg.attachedTabs(), []);
  assert.ok(s.events.some((e) => e.method === "stop" && e.params.by === "toolbar"));
  err(await s.call("act.commit", { prepare_id: p.prepare_id }), "E_STOPPED");
  err(await s.call("act.prepare", { tab: 1, action: { kind: "scroll", dx: 0, dy: 1 } }), "E_STOPPED");
  assert.equal(ok(await s.call("status.get", {})).stopped, true);
  ok(await s.call("stop.resume", {}));
  assert.equal(s.link.stopped, false);
  finish(s);
});

test("the user pressing Cancel on the infobar is a refusal for the rest of the task", async () => {
  const s = setup({ debuggerPermission: true });
  await s.dbg.attach(1);
  s.listeners.dbgDetach[0]({ tabId: 1 }, "canceled_by_user");
  assert.deepEqual(s.events.at(-1), { method: "debugger.detached", params: { tab: 1, reason: "canceled_by_user" } });
  await assert.rejects(() => s.dbg.attach(1), (e) => e.code === "E_DEBUGGER_DENIED");
  // The task is over after 2 idle minutes.
  s.advance(121000);
  await s.dbg.sweep();
  assert.equal(await s.dbg.attach(1), true);
  finish(s);
});

test("a tab that leaves the Mono's origins loses the debugger and its prepares", async () => {
  const s = setup({ debuggerPermission: true, allowed: ["https://shop.test"], build: (h) => [h("button", { id: "b", type: "button", rect: [10, 40, 80, 30] }, "Next")], url: "https://shop.test/" });
  const p = await prep(s, { kind: "click", ref: refOf(await snap(s), "button", "Next") });
  await s.dbg.attach(1);
  await s.acts.navigated(1, "https://evil.test/");
  assert.deepEqual(s.dbg.attachedTabs(), []);
  assert.equal(s.acts.records.has(p.prepare_id), false);
  assert.equal(s.events.at(-1).params.reason, "origin_left");
  finish(s);
});

test("closing the tab forgets its prepares", async () => {
  const s = setup({ build: (h) => [h("button", { id: "b", type: "button", rect: [10, 40, 80, 30] }, "Next")], url: "https://shop.test/" });
  const p = await prep(s, { kind: "click", ref: refOf(await snap(s), "button", "Next") });
  s.listeners.removed[0](1);
  assert.equal(s.acts.records.has(p.prepare_id), false);
  finish(s);
});

// ---- dialogs (F1) ----

test("a content script that stops answering after the input is E_DIALOG_SUSPECTED: no retry, reads fail fast, the late result is an event", async () => {
  const s = setup({ build: (h) => [h("button", { id: "b", type: "button", rect: [10, 40, 80, 30] }, "Next")], url: "https://shop.test/" });
  const p = await prep(s, { kind: "click", ref: refOf(await snap(s), "button", "Next") });
  // Make the next perform hang like a page blocked by alert().
  const ctx = s.world.context.zptahContent;
  const realRun = ctx.run;
  let release;
  // The click is dispatched, but the page does not answer until the user closes the dialog.
  ctx.run = (op, args) => (op === "act_perform" ? realRun(op, args).then((r) => new Promise((res) => (release = () => res(r)))) : realRun(op, args));
  const msg = err(await s.call("act.commit", { prepare_id: p.prepare_id }), "E_DIALOG_SUSPECTED");
  assert.match(msg, /outcome is unknown/);
  err(await s.call("act.commit", { prepare_id: p.prepare_id, input: "trusted" }), "E_STALE_PREPARE");
  // While it hangs, reads fail fast instead of waiting 30 s.
  ctx.run = (op, args) => (op === "state" ? new Promise(() => {}) : realRun(op, args));
  err(await s.call("page.snapshot", { tab: 1, mode: "full" }), "E_DIALOG_SUSPECTED");
  err(await s.call("act.prepare", { tab: 1, action: { kind: "scroll", dx: 0, dy: 1 } }), "E_DIALOG_SUSPECTED");
  // The user answers the dialog: the late result arrives as an event and the page answers again.
  ctx.run = realRun;
  release();
  await new Promise((r) => setTimeout(r, 400));
  const late = s.events.find((e) => e.method === "act.late");
  assert.ok(late, JSON.stringify(s.events));
  assert.equal(late.params.ok, true);
  assert.equal(late.params.prepare_id, p.prepare_id);
  validateAgainst(SCHEMA, "x-events/act.late", late.params);
  const again = await s.call("page.snapshot", { tab: 1, mode: "full" });
  assert.equal(again.ok, true, JSON.stringify(again));
  finish(s);
});

test("with the debugger attached a dialog is seen and reported, and dialog.respond answers it", async () => {
  const holder = {};
  const s = setup({ build: (h) => [h("button", { id: "b", type: "button", rect: [10, 40, 80, 30] }, "Delete")], url: "https://shop.test/", debuggerPermission: true,
    cdp: (method, params) => {
      // A click that opens a confirm() holds the input call until the dialog is answered.
      if (method === "Input.dispatchMouseEvent" && params.type === "mouseReleased") {
        holder.s.listeners.dbgEvent[0]({ tabId: 1 }, "Page.javascriptDialogOpening", { type: "confirm", message: "Really delete?" });
        return new Promise(() => {});
      }
      return {};
    } });
  holder.s = s;
  const p = await prep(s, { kind: "click", ref: refOf(await snap(s), "button", "Delete") });
  assert.equal(p.debugger_attached, true);
  const pending = s.call("act.commit", { prepare_id: p.prepare_id, approval: { approval_id: "x", categories: ["destructive"], digest: p.digest } });
  const done = ok(await pending);
  assert.match(done.effects.hint, /confirm dialog opened: "Really delete\?"/);
  const evt = s.events.find((e) => e.method === "dialog.opened");
  assert.deepEqual(evt.params, { tab: 1, kind: "confirm", message: "Really delete?" });
  // Anything else on the tab now says a dialog is open.
  err(await s.call("act.prepare", { tab: 1, action: { kind: "scroll", dx: 0, dy: 1 } }), "E_DIALOG_SUSPECTED");
  ok(await s.call("dialog.respond", { tab: 1, accept: true }));
  assert.ok(s.calls.some((c) => c[1] === "Page.handleJavaScriptDialog" && c[2].accept === true));
  s.listeners.dbgEvent[0]({ tabId: 1 }, "Page.javascriptDialogClosed", {});
  ok(await s.call("act.prepare", { tab: 1, action: { kind: "scroll", dx: 0, dy: 1 } }));
  finish(s);
});

test("dialog.respond without a dialog the debugger can see says so", async () => {
  const s = setup({ debuggerPermission: true, cdp: (m) => { if (m === "Page.handleJavaScriptDialog") throw new Error("No dialog is showing"); return {}; } });
  err(await s.call("dialog.respond", { tab: 1, accept: true }), "E_UNSUPPORTED");
  const t = setup({ debuggerPermission: false });
  err(await t.call("dialog.respond", { tab: 1, accept: true }), "E_DEBUGGER_DENIED");
  finish(s);
  finish(t);
});

// ---- ownership, levels, origins ----

test("only the owner of a tab, at act level, on an allowed origin, can prepare; prepare of a read tool still works", async () => {
  let s = setup({ owner: null });
  err(await s.call("act.prepare", { tab: 1, action: { kind: "scroll", dx: 0, dy: 1 } }), "E_NOT_OWNER");
  s = setup({ level: "read" });
  err(await s.call("act.prepare", { tab: 1, action: { kind: "scroll", dx: 0, dy: 1 } }), "E_UNSUPPORTED");
  s = setup({ blocked: ["https://shop.test"] });
  err(await s.call("act.prepare", { tab: 1, action: { kind: "scroll", dx: 0, dy: 1 } }), "E_ORIGIN_BLOCKED");
  s = setup({});
  err(await s.call("act.prepare", { tab: 1, action: { kind: "scroll", dx: 0, dy: 1 } }), "E_STALE_REF");
  err(await s.call("act.prepare", { tab: 9, action: { kind: "scroll", dx: 0, dy: 1 } }), "E_TAB_GONE");
  err(await s.call("act.prepare", { tab: 1, action: { kind: "explode" } }), "E_INVALID");
  err(await s.call("act.prepare", { tab: 1, action: { kind: "scroll", dx: 0, dy: 1 } }, "c".repeat(32)), "E_UNKNOWN_MONO");
  const t = setup({ noLexicon: true, build: (h) => [h("button", { id: "b", type: "button", rect: [1, 1, 80, 30] }, "Next")] });
  const text = await snap(t);
  err(await t.call("act.prepare", { tab: 1, action: { kind: "click", ref: refOf(text, "button", "Next") } }), "E_UNSUPPORTED");
});

test("a ref from before the last snapshot or a made-up ref is stale", async () => {
  const s = setup({ build: (h) => [h("button", { id: "b", type: "button", rect: [10, 40, 80, 30] }, "Next")], url: "https://shop.test/" });
  await snap(s);
  err(await s.call("act.prepare", { tab: 1, action: { kind: "click", ref: "e99" } }), "E_STALE_REF");
  err(await s.call("act.prepare", { tab: 1, action: { kind: "click", ref: "bogus" } }), "E_STALE_REF");
  finish(s);
});

// ---- navigation and focus ----

test("nav.go opens http and https only, checks the origin lists, and never asks for focus", async () => {
  const s = setup({});
  const r = ok(await s.call("nav.go", { tab: 1, url: "https://shop.test/next" }));
  assert.equal(r.url, "https://shop.test/next");
  const upd = s.calls.filter((c) => c[0] === "tabs.update");
  assert.deepEqual(upd, [["tabs.update", 1, { url: "https://shop.test/next" }]]);
  for (const u of ["javascript:alert(1)", "file:///etc/passwd", "chrome://settings", "data:text/html,x", "ftp://a.test/"]) err(await s.call("nav.go", { tab: 1, url: u }), "E_INVALID");
  err(await s.call("nav.go", { tab: 1, url: "https://bank.test/login" }), "E_ORIGIN_BLOCKED");
  err(await s.call("nav.go", { tab: 1, url: "not a url" }), "E_INVALID");
  assert.equal(s.calls.filter((c) => c[0] === "tabs.update").length, 1);
  const h = ok(await s.call("nav.history", { tab: 1, dir: "back" }));
  assert.equal(h.url, "https://shop.test/previous");
  assert.ok(s.calls.some((c) => c[0] === "goBack"));
  finish(s);
});

test("the focus guard refuses to activate, highlight or focus", () => {
  const calls = [];
  const api = { tabs: { update: (...a) => calls.push(["u", ...a]), create: (p) => calls.push(["c", p]) }, windows: { update: (...a) => calls.push(["w", ...a]) } };
  const g = focusGuard(api);
  for (const props of [{ active: true }, { highlighted: true }, { selected: true }]) assert.throws(() => g.tabs.update(1, props), (e) => e.code === "E_INVALID");
  for (const props of [{ focused: true }, { drawAttention: true }]) assert.throws(() => g.windows.update(1, props), (e) => e.code === "E_INVALID");
  assert.throws(() => g.tabs.create({ active: true }), (e) => e.code === "E_INVALID");
  assert.equal(calls.length, 0);
  g.tabs.update(1, { url: "https://a.test/" });
  g.tabs.create({ url: "https://a.test/" });
  assert.deepEqual(calls, [["u", 1, { url: "https://a.test/" }], ["c", { url: "https://a.test/", active: false }]]);
});

// ---- uploads ----

test("upload.put collects ordered chunks and the upload builds File objects for the input", async () => {
  const s = setup({ build: (h) => [h("input", { id: "f", type: "file", "aria-label": "Photo", rect: [10, 10, 100, 30] })], url: "https://shop.test/upload" });
  const text = await snap(s);
  const file = (bytes) => {
    const len = new Uint8Array(4);
    new DataView(len.buffer).setUint32(0, bytes.length);
    return [...len, ...bytes];
  };
  const stream = new Uint8Array([...file([1, 2, 3]), ...file([9, 9])]);
  const action = { kind: "upload", ref: refOf(text, "button", "Photo"), upload_id: "u1", names: ["a.png", "b.txt"] };
  err(await s.call("act.prepare", { tab: 1, action }), "E_INVALID");
  err(await s.call("upload.put", { upload_id: "u1", seq: 1, bytes_b64: b64encode(stream), last: true }), "E_INVALID");
  ok(await s.call("upload.put", { upload_id: "u1", seq: 0, bytes_b64: b64encode(stream.subarray(0, 6)), last: false }));
  err(await s.call("upload.put", { upload_id: "u1", seq: 0, bytes_b64: b64encode(stream.subarray(6)), last: true }), "E_INVALID");
  ok(await s.call("upload.put", { upload_id: "u1", seq: 1, bytes_b64: b64encode(stream.subarray(6)), last: true }));
  const p = await prep(s, action);
  assert.deepEqual(p.ext_risk, ["upload"]);
  err(await s.call("act.commit", { prepare_id: p.prepare_id }), "E_NEEDS_APPROVAL");
  ok(await s.call("act.commit", { prepare_id: p.prepare_id, approval: { approval_id: "x", categories: ["upload"], digest: p.digest } }));
  const input = s.world.document.querySelector("#f");
  assert.deepEqual(input.files.map((f) => [f.name, f.size, f.type]), [["a.png", 3, "image/png"], ["b.txt", 2, "text/plain"]]);
  err(await s.call("upload.put", { upload_id: "big", seq: 0, bytes_b64: b64encode(new Uint8Array(512 * 1024 + 1)), last: true }), "E_TOO_LARGE");
  finish(s);
});

// ---- status ----

test("status.get reports attached tabs and Monos with pending prepares", async () => {
  const s = setup({ debuggerPermission: true, build: (h) => [h("button", { id: "b", type: "button", rect: [10, 40, 80, 30] }, "Next")], url: "https://shop.test/" });
  await prep(s, { kind: "click", ref: refOf(await snap(s), "button", "Next") });
  await s.dbg.attach(1);
  assert.deepEqual(ok(await s.call("status.get", {})), { stopped: false, debugger_attached: [1], monos_active: 1 });
  finish(s);
});

test("the golden samples are valid against the schema and carry every ElementSignals field of the Rust type", () => {
  const g = shared("act-samples.json");
  validateAgainst(SCHEMA, "ElementSignals", g.element);
  validateAgainst(SCHEMA, "ElementSignals", g.element_in_dialog);
  validateAgainst(SCHEMA, "FormSignals", g.form);
  validateAgainst(SCHEMA, "FormSignals", g.form_with_recipients);
});

// ---- review fixes ----

test("Stop during a commit halts it before anything is dispatched, content or debugger path", async () => {
  const build = (h) => [h("button", { id: "b", type: "button", rect: [10, 40, 80, 30] }, "Next")];
  // Stop while the page is re-checked: no input event reaches the page.
  let s = setup({ build, url: "https://shop.test/" });
  let p = await prep(s, { kind: "click", ref: refOf(await snap(s), "button", "Next") });
  const ctx = s.world.context.zptahContent;
  const realRun = ctx.run;
  ctx.run = (op, args) => {
    const r = realRun(op, args);
    if (op === "act_revalidate") s.listeners.clicked[0]();
    return r;
  };
  s.world.log.length = 0;
  err(await s.call("act.commit", { prepare_id: p.prepare_id }), "E_STOPPED");
  assert.ok(!s.world.log.some((e) => e.type === "click"), "nothing was clicked");
  finish(s);
  // Stop while the debugger is mid-click: the next command is refused, so no mouseReleased goes out.
  const holder = {};
  s = setup({ build, url: "https://shop.test/", debuggerPermission: true, cdp: (m, params) => {
    if (m === "Input.dispatchMouseEvent" && params.type === "mousePressed") holder.s.listeners.clicked[0]();
    return {};
  } });
  holder.s = s;
  p = await prep(s, { kind: "click", ref: refOf(await snap(s), "button", "Next") });
  err(await s.call("act.commit", { prepare_id: p.prepare_id, input: "trusted" }), "E_STOPPED");
  assert.ok(!s.calls.some((c) => c[0] === "cdp" && c[2]?.type === "mouseReleased"));
  await assert.rejects(() => s.dbg.attach(1), (e) => e.code === "E_STOPPED");
  finish(s);
});

test("a text field labelled as a password is refused for type, fill and press, and before each debugger key", async () => {
  const build = (h) => [
    h("input", { id: "pw", type: "text", "aria-label": "Mat khau", rect: [10, 10, 200, 30] }),
    h("input", { id: "pw2", type: "text", "aria-label": "Password", rect: [10, 50, 200, 30] }),
    h("input", { id: "u", type: "text", "aria-label": "User", rect: [10, 90, 200, 30] }),
  ];
  const s = setup({ build, url: "https://shop.test/login", debuggerPermission: true });
  const text = await snap(s);
  const doc = s.world.document;
  for (const name of ["Mat khau", "Password"]) {
    const ref = refOf(text, "textbox", name);
    err(await s.call("act.prepare", { tab: 1, action: { kind: "type", ref, text: "x" } }), "E_PASSWORD_FIELD");
    err(await s.call("act.prepare", { tab: 1, action: { kind: "fill", fields: [{ ref, value: "x", type: "text" }] } }), "E_PASSWORD_FIELD");
    err(await s.call("act.prepare", { tab: 1, action: { kind: "press", key: "a", ref } }), "E_PASSWORD_FIELD");
  }
  doc.activeElement = doc.querySelector("#pw");
  err(await s.call("act.prepare", { tab: 1, action: { kind: "press", key: "a" } }), "E_PASSWORD_FIELD");
  // Focus moving into such a field after the checks is caught before the key goes out.
  doc.activeElement = doc.querySelector("#u");
  const q = await prep(s, { kind: "press", key: "a" });
  const ctx = s.world.context.zptahContent;
  const realRun = ctx.run;
  ctx.run = (op, args) => {
    if (op === "act_active") doc.activeElement = doc.querySelector("#pw2");
    return realRun(op, args);
  };
  s.calls.length = 0;
  err(await s.call("act.commit", { prepare_id: q.prepare_id, input: "trusted" }), "E_PASSWORD_FIELD");
  assert.ok(!s.calls.some((c) => c[0] === "cdp" && /Input\.(dispatchKeyEvent|insertText)/.test(c[1])));
  finish(s);
});

test("a multi-field fill over the debugger focuses each field in turn", async () => {
  const build = (h) => [h("input", { id: "a", type: "text", "aria-label": "First", rect: [10, 10, 200, 30] }), h("input", { id: "b", type: "text", "aria-label": "Second", rect: [10, 50, 200, 30] })];
  const holder = {};
  const typed = [];
  const s = setup({ build, url: "https://shop.test/", debuggerPermission: true, cdp: (m, params) => {
    if (m === "Input.insertText") typed.push([holder.s.world.document.activeElement.id, params.text]);
    return {};
  } });
  holder.s = s;
  const text = await snap(s);
  const p = await prep(s, { kind: "fill", fields: [
    { ref: refOf(text, "textbox", "First"), value: "one", type: "text" },
    { ref: refOf(text, "textbox", "Second"), value: "two", type: "text" },
  ] });
  ok(await s.call("act.commit", { prepare_id: p.prepare_id, input: "trusted" }));
  assert.deepEqual(typed, [["a", "one"], ["b", "two"]]);
  finish(s);
});

test("extend rejects a non-integer until_ms; commit rechecks the Mono is still act level", async () => {
  const s = setup({ build: (h) => [h("button", { id: "b", type: "button", rect: [10, 40, 80, 30] }, "Next")], url: "https://shop.test/" });
  const p = await prep(s, { kind: "click", ref: refOf(await snap(s), "button", "Next") });
  for (const bad of ["x", null, 1.5, Number.MAX_SAFE_INTEGER + 2]) err(await s.call("act.extend", { prepare_id: p.prepare_id, until_ms: bad }), "E_INVALID");
  s.link.roster[0].level = "read";
  err(await s.call("act.commit", { prepare_id: p.prepare_id }), "E_UNSUPPORTED");
  finish(s);
});

test("a commit already running refuses a second one, and a dispatched commit is never run twice", async () => {
  const s = setup({ build: (h) => [h("button", { id: "b", type: "button", rect: [10, 40, 80, 30] }, "Next")], url: "https://shop.test/" });
  let clicks = 0;
  s.world.document.querySelector("#b").addEventListener("click", () => (clicks += 1));
  const p = await prep(s, { kind: "click", ref: refOf(await snap(s), "button", "Next") });
  const ctx = s.world.context.zptahContent;
  const realRun = ctx.run;
  let release;
  ctx.run = (op, args) => (op === "act_perform" ? realRun(op, args).then((r) => new Promise((res) => (release = () => res(r)))) : realRun(op, args));
  const first = s.call("act.commit", { prepare_id: p.prepare_id });
  await new Promise((r) => setTimeout(r, 100));
  err(await s.call("act.commit", { prepare_id: p.prepare_id }), "E_BUSY");
  assert.equal((await first).error?.code, "E_DIALOG_SUSPECTED");
  release();
  await new Promise((r) => setTimeout(r, 50));
  err(await s.call("act.commit", { prepare_id: p.prepare_id }), "E_STALE_PREPARE");
  assert.equal(clicks, 1);
  finish(s);
});

test("paste shortcuts ask like credential entry; a fill checkbox behind a cover is not clicked; an upload is used once", async () => {
  const s = setup({ build: (h) => [
    h("input", { id: "u", type: "text", "aria-label": "User", rect: [10, 10, 200, 30] }),
    h("input", { id: "c", type: "checkbox", "aria-label": "Agree", rect: [10, 50, 20, 20] }),
    h("input", { id: "f", type: "file", "aria-label": "Photo", rect: [10, 90, 100, 30] }),
  ], url: "https://shop.test/" });
  const text = await snap(s);
  for (const key of ["Control+v", "Meta+V", "Shift+Insert"]) {
    const p = await prep(s, { kind: "press", key, ref: refOf(text, "textbox", "User") });
    assert.deepEqual(p.ext_risk, ["credential_entry"], key);
  }
  const p = await prep(s, { kind: "fill", fields: [{ ref: refOf(text, "checkbox", "Agree"), value: "true", type: "checkbox" }] });
  s.world.hitOverride = s.world.h("div", { id: "overlay" }, "banner");
  err(await s.call("act.commit", { prepare_id: p.prepare_id }), "E_INVALID");
  assert.equal(s.world.document.querySelector("#c").checked, false);
  s.world.hitOverride = null;
  const bytes = new Uint8Array([0, 0, 0, 1, 7]);
  ok(await s.call("upload.put", { upload_id: "u9", seq: 0, bytes_b64: b64encode(bytes), last: true }));
  const up = await prep(s, { kind: "upload", ref: refOf(text, "button", "Photo"), upload_id: "u9", names: ["a.txt"] });
  ok(await s.call("act.commit", { prepare_id: up.prepare_id, approval: { approval_id: "x", categories: ["upload"], digest: up.digest } }));
  assert.equal(s.acts.uploads.has("u9"), false);
  finish(s);
});
