// @ts-nocheck - the fake permissions API is a duck-typed test double.
// The pairing page states and the options page logic: connection card, active Monos and the
// debugger permission flow. All pure, so no browser is needed.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { countdownText, CLOSE_SECONDS, desktopName, noCodeView, pairView, TIMEOUT_MS } from "../src/pair/model.js";
import {
  activeMonosText,
  connectionFacts,
  debuggerView,
  describeStatus,
} from "../src/options/model.js";
import { createPrefsWriter, debuggerAllowed, defaultPrefs, loadPrefs, parsePrefs, PREFS_KEY } from "../src/worker/prefs.js";

// ---- pairing page ----

test("pairing: waiting, verifying and connected follow the link status", () => {
  assert.equal(pairView({ status: "pairing" }, 0).state, "waiting");
  const verifying = pairView({ status: "connecting" }, 3000);
  assert.equal(verifying.state, "verifying");
  assert.equal(verifying.step, 2);
  const ok = pairView({ status: "ready", app_version: "0.9.1" }, 4000);
  assert.equal(ok.state, "connected");
  assert.equal(ok.tone, "ok");
  assert.equal(ok.step, 3);
  assert.match(ok.body, /zptah 0\.9\.1/);
});

test("pairing: the desktop is named by the app version, the only name the protocol carries", () => {
  assert.equal(desktopName({ app_version: " 2.1.0 " }), "zptah 2.1.0 on this computer");
  assert.equal(desktopName({}), "zptah on this computer");
  assert.equal(desktopName(null), "zptah on this computer");
});

test("pairing: failures carry a reason; Retry only where the same code can still work", () => {
  const f = pairView({ status: "pair_failed", detail: "E_NOT_PAIRED" }, 2000);
  assert.equal(f.state, "failed");
  assert.match(f.body, /E_NOT_PAIRED/);
  assert.match(f.reason, /Connect extension/);
  // Refused by the app, or a bad code: sending the same code again cannot help.
  assert.equal(f.retry, false);
  assert.equal(pairView({ status: "pair_failed", detail: "bad nonce" }, 0).retry, false);
  assert.match(pairView({ status: "pair_failed", detail: "bad nonce" }, 0).body, /not valid/);
  // Other failures may be a passing problem (the app was not ready): Retry is offered.
  const other = pairView({ status: "auth_failed", detail: "" }, 0);
  assert.match(other.body, /did not accept/);
  assert.equal(other.retry, true);
  assert.equal(pairView({ status: "pair_failed", detail: "Native host has exited." }, 0).retry, true);
});

test("pairing: a dropped connection is a failure with its reason, not a wait until the limit", () => {
  const v = pairView({ status: "disconnected", detail: "Native host has exited." }, 1000);
  assert.equal(v.state, "failed");
  assert.match(v.body, /Native host has exited/);
  assert.equal(v.retry, true);
  assert.equal(pairView({ status: "idle", detail: "" }, 1000).state, "failed");
});

test("pairing: a missing connector is its own failure", () => {
  for (const v of [pairView({ status: "host_missing" }, 0), pairView({ status: "pair_failed", detail: "Specified native messaging host not found." }, 0)]) {
    assert.equal(v.state, "failed");
    assert.match(v.body, /connector/);
    assert.equal(v.retry, true);
  }
});

test("pairing: a wrong or unknown build is told apart from a plain failure", () => {
  const a = pairView({ status: "protocol_mismatch", detail: "" }, 0);
  const b = pairView({ status: "pair_failed", detail: "unsupported protocol" }, 0);
  for (const v of [a, b]) {
    assert.equal(v.state, "wrong_build");
    assert.equal(v.tone, "warn");
    assert.match(v.reason, /Update/);
    assert.equal(v.retry, false);
  }
  // Only the exact status or text counts: a message that merely mentions a protocol does not.
  assert.equal(pairView({ status: "pair_failed", detail: "protocol buffers are fun" }, 0).state, "failed");
  assert.equal(pairView({ status: "pair_failed", detail: "unsupported protocol version 9" }, 0).state, "failed");
});

test("pairing: it times out after the page limit or when the handshake timed out", () => {
  assert.equal(pairView({ status: "pairing" }, TIMEOUT_MS - 1).state, "waiting");
  const t = pairView({ status: "pairing" }, TIMEOUT_MS);
  assert.equal(t.state, "timeout");
  assert.equal(t.retry, true);
  assert.equal(pairView({ status: "pair_failed", detail: "handshake timed out" }, 100).state, "timeout");
  // Success still wins at the limit.
  assert.equal(pairView({ status: "ready" }, TIMEOUT_MS * 2).state, "connected");
});

test("pairing: a page opened without a code says so and offers no Retry", () => {
  const v = noCodeView();
  assert.equal(v.state, "no_code");
  assert.equal(v.retry, false);
  assert.match(v.reason, /Settings > Browser/);
});

test("pairing: the close countdown text and length", () => {
  assert.equal(CLOSE_SECONDS, 5);
  assert.equal(countdownText(5), "Closing this tab in 5 seconds.");
  assert.equal(countdownText(1), "Closing this tab in 1 second.");
});

// ---- options: connection ----

test("options: the connection card tone and label", () => {
  const base = { paired: true, protocol: 1 };
  assert.deepEqual(pick(describeStatus({ ...base, status: "ready" })), ["ok", "Connected"]);
  assert.deepEqual(pick(describeStatus({ ...base, status: "ready", stopped: true })), ["ok", "Connected, stopped"]);
  assert.deepEqual(pick(describeStatus({ ...base, status: "protocol_mismatch" })), ["warn", "Version mismatch"]);
  assert.deepEqual(pick(describeStatus({ ...base, status: "auth_failed" })), ["bad", "Not accepted"]);
  assert.deepEqual(pick(describeStatus({ ...base, status: "host_missing" })), ["bad", "Connector missing"]);
  assert.deepEqual(pick(describeStatus({ ...base, status: "disconnected" })), ["warn", "Offline"]);
  assert.deepEqual(pick(describeStatus({ paired: false, status: "idle", protocol: 1 })), ["idle", "Not paired"]);
});
const pick = (d) => [d.tone, d.label];

test("options: connection facts list only what is known", () => {
  const rows = connectionFacts({ status: "ready", paired: true, protocol: 1, app_version: "0.9.1", browser: { brand: "Chrome", version: "158" }, instance_id: "0123456789abcdef" });
  assert.deepEqual(rows, [["Desktop", "zptah 0.9.1"], ["Browser", "Chrome 158"], ["Profile id", "01234567"], ["Protocol", "1"]]);
  assert.deepEqual(connectionFacts({ status: "idle", paired: false, protocol: 1 }), []);
  assert.deepEqual(connectionFacts({ status: "disconnected", paired: true, protocol: 1 }).map((r) => r[0]), ["Browser", "Protocol"]);
});

test("options: the active Monos line", () => {
  assert.match(activeMonosText(0), /No Mono/);
  assert.match(activeMonosText(undefined), /No Mono/);
  assert.equal(activeMonosText(1), "1 Mono is working in this browser.");
  assert.equal(activeMonosText(3), "3 Monos are working in this browser.");
});

// ---- options: the debugger fallback switch ----

const area = (prefs) => ({ get: async (k) => ({ [k]: prefs }) });

test("debugger: it is a required permission with no optional list, and the switch is off by default", () => {
  const manifest = JSON.parse(readFileSync(new URL("../manifest.json", import.meta.url), "utf8"));
  const dev = { ...manifest, ...JSON.parse(readFileSync(new URL("../manifest.dev.json", import.meta.url), "utf8")) };
  for (const m of [manifest, dev]) {
    assert.ok(m.permissions.includes("debugger"));
    assert.equal("optional_permissions" in m, false);
  }
  assert.equal(defaultPrefs().use_debugger, false);
  assert.equal(parsePrefs({ cursor: false }).use_debugger, false);
  assert.equal(parsePrefs({ use_debugger: true }).use_debugger, true);
  assert.equal(parsePrefs({ use_debugger: "yes" }).use_debugger, false);
});

test("debugger: the fallback is usable only when chrome.debugger exists and the switch is on", async () => {
  const on = { debugger: {}, storage: { local: area({ use_debugger: true }) } };
  assert.equal(await debuggerAllowed(on), true);
  assert.equal(await debuggerAllowed({ ...on, storage: { local: area({ use_debugger: false }) } }), false);
  assert.equal(await debuggerAllowed({ ...on, storage: { local: area(undefined) } }), false);
  assert.equal(await debuggerAllowed({ storage: on.storage }), false, "no chrome.debugger");
  assert.equal(await debuggerAllowed({ debugger: {}, storage: { local: { get: async () => { throw new Error("x"); } } } }), false);
  assert.equal(await debuggerAllowed({ debugger: {} }), false, "no storage");
});

test("debugger: turning the switch keeps the other settings", async () => {
  const stored = { [PREFS_KEY]: { cursor: false, off_sites: ["a.example"], use_debugger: false } };
  const prefs = await loadPrefs({ get: async () => stored });
  const next = parsePrefs({ ...prefs, use_debugger: true });
  assert.deepEqual(next, { cursor: false, off_sites: ["a.example"], use_debugger: true });
});

test("debugger: the card state for on, off and a build Chrome did not give the permission", () => {
  assert.deepEqual(debuggerView({ available: true, enabled: true }), { label: "On", tone: "ok", note: "", canToggle: true });
  assert.deepEqual(debuggerView({ available: true, enabled: false }), { label: "Off", tone: "idle", note: "", canToggle: true });
  const gone = debuggerView({ available: false, enabled: true });
  assert.equal(gone.canToggle, false);
  assert.match(gone.note, /did not give/);
});

test("debugger: the options page has no permission request or revoke any more", () => {
  const src = readFileSync(new URL("../src/options/options.js", import.meta.url), "utf8");
  assert.doesNotMatch(src, /permissions\.(request|remove)/);
});

test("the shipped pages carry no wording about a few seconds", () => {
  for (const f of ["../src/options/options.html", "../src/pair/pair.html", "../src/options/model.js", "../src/pair/model.js"]) {
    assert.doesNotMatch(readFileSync(new URL(f, import.meta.url), "utf8"), /few seconds/i, f);
  }
});

test("settings writes are serialized and merged, so quick changes do not overwrite each other", async () => {
  let stored = { cursor: true, off_sites: [], use_debugger: false };
  const area = {
    get: async () => {
      const snapshot = structuredClone(stored);
      await new Promise((r) => setTimeout(r, 5));
      return { [PREFS_KEY]: snapshot };
    },
    set: async (o) => {
      await new Promise((r) => setTimeout(r, 2));
      stored = structuredClone(o[PREFS_KEY]);
    },
  };
  const write = createPrefsWriter(area);
  await Promise.all([
    write((c) => ({ ...c, cursor: false })),
    write((c) => ({ ...c, use_debugger: true })),
    write((c) => ({ ...c, off_sites: [...c.off_sites, "a.example"] })),
    write((c) => ({ ...c, off_sites: [...c.off_sites, "b.example"] })),
  ]);
  assert.deepEqual(stored, { cursor: false, off_sites: ["a.example", "b.example"], use_debugger: true });
});

test("a failed settings write rejects its own call and does not block the next", async () => {
  let fail = true;
  let stored = null;
  const area = {
    get: async () => ({ [PREFS_KEY]: stored }),
    set: async (o) => {
      if (fail) throw new Error("quota");
      stored = o[PREFS_KEY];
    },
  };
  const write = createPrefsWriter(area);
  await assert.rejects(write((c) => ({ ...c, cursor: false })), /quota/);
  fail = false;
  assert.equal((await write((c) => ({ ...c, cursor: false }))).cursor, false);
  assert.equal(stored.cursor, false);
});

test("options page: status text is written only when it changes, and unpair and reconnect never throw", () => {
  const src = readFileSync(new URL("../src/options/options.js", import.meta.url), "utf8");
  assert.match(src, /const setText = /);
  assert.doesNotMatch(src, /\$\("status"\)\.textContent =/);
  for (const t of ["zptah.unpair", "zptah.reconnect"]) {
    assert.match(src, new RegExp(`sendMessage\\(\\{ type: "${t.replace(".", "\\.")}" \\}\\)\\.catch`), t);
  }
});

test("pair page: the countdown is outside the live region", () => {
  const html = readFileSync(new URL("../src/pair/pair.html", import.meta.url), "utf8");
  assert.match(html, /<p id="reason"[^>]*><\/p>\s*<\/div>\s*<p id="countdown"/);
});
