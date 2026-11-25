import assert from "node:assert/strict";
import test from "node:test";
import { scanSource, stripComments } from "../scripts/scan.mjs";

/** @type {[string, string][]} */
const BAD_JS = [
  ["eval", "eval('1')"],
  ["new-function", "const f = new Function('return 1')"],
  ["new-function", "Function('return 1')()"],
  ["string-timer", "setTimeout('x()', 10)"],
  ["string-timer", "setInterval(`x()`, 10)"],
  ["importScripts", "importScripts('a.js')"],
  ["dynamic-import", "const m = await import('./a.js')"],
  ["fetch", "fetch('https://example.com')"],
  ["fetch", "await fetch('http://127.0.0.1:1/')"],
  ["XMLHttpRequest", "new XMLHttpRequest()"],
  ["WebSocket", "new WebSocket('ws://x')"],
  ["EventSource", "new EventSource('/e')"],
  ["sendBeacon", "navigator.sendBeacon('/b', 'x')"],
  ["userScripts", "chrome.userScripts.register([])"],
  ["registerContentScripts", "chrome.scripting.registerContentScripts([])"],
  ["requestUpdateCheck", "chrome.runtime.requestUpdateCheck()"],
  ["setUninstallURL", "chrome.runtime.setUninstallURL('https://x')"],
  ["tabs-update-active", "chrome.tabs.update(id, { url, active: true })"],
  ["tabs-update-active", "chrome.tabs.update(id, {\n  active: true,\n})"],
  ["windows-update-focus", "chrome.windows.update(w, { focused: true })"],
];

for (const [id, src] of BAD_JS) {
  test(`rejects ${id}: ${src.slice(0, 30).replace(/\n/g, " ")}`, () => {
    const found = scanSource("a.js", src);
    assert.ok(found.some((m) => m.includes(`forbidden ${id} `)), found.join("\n"));
  });
}

test("rejects bad html", () => {
  assert.ok(scanSource("p.html", "<script>alert(1)</script>").length > 0);
  assert.ok(scanSource("p.html", '<script src="https://cdn.example/x.js"></script>').length > 0);
  assert.ok(scanSource("p.html", '<script src="//cdn.example/x.js"></script>').length > 0);
  assert.ok(scanSource("p.html", '<link rel="stylesheet" href="https://x/y.css">').length > 0);
  assert.ok(scanSource("p.html", '<button onclick="x()">').length > 0);
});

test("store build rejects test hooks, dev build allows them", () => {
  assert.ok(scanSource("a.js", "globalThis.__zptahTest = 1", { store: true }).length > 0);
  assert.equal(scanSource("a.js", "globalThis.__zptahTest = 1", { store: false }).length, 0);
});

test("accepts clean code, comments and look-alikes", () => {
  const ok = [
    "// eval( fetch( is only a comment\nconst a = 1;",
    "/* new Function() */ const b = 2;",
    "const s = 'http://x'; const t = 3;",
    "port.postMessage({ a: 1 }); setTimeout(() => {}, 5);",
    "obj.fetch(1); const prefetch = 2; myeval(1);",
    "chrome.tabs.update(id, { url: 'https://x' })",
    "chrome.windows.update(w, { state: 'normal' })",
  ];
  for (const src of ok) assert.deepEqual(scanSource("a.js", src), [], src);
  assert.deepEqual(
    scanSource("p.html", '<link rel="stylesheet" href="options.css"><script src="options.js" type="module"></script>'),
    [],
  );
});

test("stripComments keeps comment markers inside strings and line numbers", () => {
  assert.equal(stripComments("a('//x'); // c\nb"), "a('//x'); \nb");
  assert.equal(stripComments("a /* x\ny */ b").split("\n").length, 2);
});

test("only the front guard may bring the user's own tab and window back to the front", () => {
  const restore = "api.tabs.update(id, { active: true });\napi.windows.update(w, { focused: true });";
  assert.deepEqual(scanSource("src/worker/front-guard.js", restore), []);
  assert.deepEqual(scanSource("src\\worker\\front-guard.js", restore), []);
  for (const name of ["src/worker/act.js", "src/worker/ownership.js", "front-guard.js", "src/other/front-guard.js"]) {
    const found = scanSource(name, restore);
    assert.ok(found.some((m) => m.includes("forbidden tabs-update-active")), `${name}: ${found.join("\n")}`);
    assert.ok(found.some((m) => m.includes("forbidden windows-update-focus")), name);
  }
  // The exemption covers `active:` and `focused:` only.
  assert.ok(scanSource("src/worker/front-guard.js", "eval('1')").length > 0);
  for (const src of ["api.tabs.update(id, { highlighted: true });", "api.tabs.update(id, { selected: true });", "api.windows.update(w, { drawAttention: true });"]) {
    assert.ok(scanSource("src/worker/front-guard.js", src).length > 0, src);
  }
});
