// @ts-nocheck - the fake port and fake app are duck-typed test doubles.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { backoffDelay, BACKOFF_MAX_MS } from "../src/worker/backoff.js";
import { b64decodeFixed, b64encode, toHex } from "../src/worker/bytes.js";
import { randomBytes } from "../src/worker/crypto.js";
import { appMac, DIR_APP_TO_EXT, extMac, Session, sessionKey } from "../src/worker/handshake.js";
import { BrowserLink } from "../src/worker/link.js";
import { createStore, trustedArea } from "../src/worker/store.js";

const V = JSON.parse(readFileSync(new URL("../shared/handshake-vectors.json", import.meta.url), "utf8"));
globalThis.chrome = /** @type {any} */ ({ runtime: { lastError: undefined } });

const tick = () => new Promise((r) => setImmediate(r));
async function until(cond, what) {
  for (let i = 0; i < 200; i += 1) {
    if (cond()) return;
    await tick();
  }
  assert.fail(`timed out waiting for ${what}`);
}

/** The app side of one connection, written against the same primitives and the wire rules of auth.rs. */
class FakeApp {
  constructor(port, opts) {
    this.port = port;
    this.opts = opts;
    this.session = null;
    this.seen = [];
    this.appChal = randomBytes(32);
  }
  send(frame) {
    this.port.deliver(frame);
  }
  async onFrame(f) {
    this.seen.push(f);
    const o = this.opts;
    if (o.script) return o.script(this, f);
    if (f.method === "hello") {
      const extChal = b64decodeFixed(f.params.challenge, 32);
      if (!f.params.paired) {
        this.send({ v: 1, type: "res", id: f.id, ok: true, result: { protocol: 1, app_version: "2.0.0", challenge: b64encode(this.appChal) } });
        return;
      }
      this.extChal = extChal;
      const mac = await appMac(o.key, extChal, this.appChal, f.params.instance_id);
      this.send({ v: 1, type: "res", id: f.id, ok: true, result: { protocol: 1, app_version: "2.0.0", challenge: b64encode(this.appChal), mac: b64encode(mac) } });
    } else if (f.method === "auth") {
      const hello = this.seen[0];
      const want = b64encode(await extMac(o.key, this.extChal, this.appChal, hello.params.instance_id));
      if (f.params.mac !== want) return this.send({ v: 1, type: "res", id: f.id, ok: false, error: { code: "E_AUTH", message: "no" } });
      this.session = new Session(await sessionKey(o.key, this.extChal, this.appChal), DIR_APP_TO_EXT);
      this.send(await this.session.seal({ v: 1, type: "res", id: f.id, ok: true, result: { roster: { monos: [] }, stopped: false } }));
    } else if (f.method === "pair.begin") {
      this.send({ v: 1, type: "res", id: f.id, ok: true, result: { key: b64encode(o.newKey) } });
    }
  }
}

/** Fake native port plus the app behind it. */
function harness(opts) {
  const ports = [];
  const store = memStore(opts.stored);
  const statuses = [];
  let now = 1_000_000;
  const timers = [];
  const link = new BrowserLink({
    hostName: "h",
    connectNative: (name) => {
      assert.equal(name, "h");
      if (opts.missingHost) {
        throw new Error("Specified native messaging host not found.");
      }
      const msgL = [];
      const discL = [];
      const port = {
        postMessage: (m) => {
          queueMicrotask(() => app.onFrame(JSON.parse(JSON.stringify(m))));
        },
        disconnect: () => {
          port.closed = true;
        },
        onMessage: { addListener: (cb) => msgL.push(cb) },
        onDisconnect: { addListener: (cb) => discL.push(cb) },
        deliver: (m) => msgL.forEach((cb) => cb(JSON.parse(JSON.stringify(m)))),
        drop: (message) => {
          globalThis.chrome.runtime.lastError = message ? { message } : undefined;
          discL.forEach((cb) => cb());
          globalThis.chrome.runtime.lastError = undefined;
        },
        closed: false,
      };
      const app = new FakeApp(port, opts);
      port.app = app;
      ports.push(port);
      return port;
    },
    store,
    extVersion: "1.0.0",
    browser: { brand: "Chrome", version: "120" },
    now: () => now,
    setTimer: (fn, ms) => (timers.push({ fn, ms }), timers.length),
    clearTimer: () => {},
    onStatus: (s) => statuses.push(s),
  });
  return { link, ports, store, statuses, timers, advance: (ms) => (now += ms) };
}

function memStore(initial) {
  const data = {};
  if (initial) data["zptah.link"] = initial;
  const area = {
    get: async (k) => ({ [k]: data[k] }),
    set: async (items) => Object.assign(data, JSON.parse(JSON.stringify(items))),
  };
  return Object.assign(createStore(area), { data });
}

const KEY = b64decodeFixed(V.key, 32);
const IID = V.instance_id;
const paired = { instance_id: IID, key: V.key };

test("paired link: hello, mutual auth, then sealed frames both ways", async () => {
  const h = harness({ key: KEY, stored: paired });
  await h.link.start();
  await until(() => h.link.status === "ready", "ready");
  const app = h.ports[0].app;
  assert.equal(app.seen[0].params.paired, true);
  assert.equal(app.seen[0].params.instance_id, IID);
  assert.equal(app.seen[0].id, "x-1");
  assert.equal(app.seen[1].method, "auth");
  assert.equal(h.link.stopped, false);

  // App request, unknown to the registry: E_UNSUPPORTED, sealed with ext seq 1.
  app.send(await app.session.seal({ v: 1, type: "req", id: "a-1", method: "status.get", params: {} }));
  await until(() => app.seen.length === 3, "reply");
  const reply = app.seen[2];
  assert.equal(reply.error.code, "E_UNSUPPORTED");
  assert.equal(reply.seq, 1);
  assert.equal(typeof reply.mac, "string");

  // A registered handler answers, seq 2.
  h.link.handlers.set("status.get", async (f) => ({ v: 1, type: "res", id: f.id, ok: true, result: { stopped: false } }));
  app.send(await app.session.seal({ v: 1, type: "req", id: "a-2", method: "status.get", params: {} }));
  await until(() => app.seen.length === 4, "second reply");
  assert.equal(app.seen[3].seq, 2);
  assert.equal(app.seen[3].ok, true);

  // Events are sealed too.
  assert.equal(h.link.sendEvent("stop", {}), true);
  await until(() => app.seen.length === 5, "event");
  assert.equal(app.seen[4].seq, 3);
});

test("a wrong app mac closes the link before auth is sent", async () => {
  const h = harness({ key: b64decodeFixed(V.wrong_key, 32), stored: paired });
  await h.link.start();
  await until(() => h.link.status === "auth_failed", "auth_failed");
  assert.equal(h.ports[0].closed, true);
  assert.equal(h.ports[0].app.seen.length, 1, "no auth frame was sent");
});

test("a hello result without a mac is refused while paired", async () => {
  const h = harness({
    key: KEY,
    stored: paired,
    script: (app, f) => app.send({ v: 1, type: "res", id: f.id, ok: true, result: { protocol: 1, app_version: "2", challenge: V.app_challenge } }),
  });
  await h.link.start();
  await until(() => h.link.status === "auth_failed", "auth_failed");
  assert.equal(h.ports[0].app.seen.length, 1);
});

test("a tampered frame closes the link", async () => {
  const h = harness({ key: KEY, stored: paired });
  await h.link.start();
  await until(() => h.link.status === "ready", "ready");
  const app = h.ports[0].app;
  const good = await app.session.seal({ v: 1, type: "req", id: "a-1", method: "status.get", params: {} });
  app.send({ ...good, method: "tab.close" });
  await until(() => h.link.status !== "ready", "closed");
  assert.equal(h.ports[0].closed, true);
  assert.equal(h.link.status, "auth_failed");
});

test("a replayed or skipped seq closes the link", async () => {
  for (const mode of ["replay", "skip"]) {
    const h = harness({ key: KEY, stored: paired });
    await h.link.start();
    await until(() => h.link.status === "ready", "ready");
    const app = h.ports[0].app;
    const f1 = await app.session.seal({ v: 1, type: "req", id: "a-1", method: "status.get", params: {} });
    const f2 = await app.session.seal({ v: 1, type: "req", id: "a-2", method: "status.get", params: {} });
    if (mode === "replay") {
      app.send(f1);
      app.send(f1);
    } else {
      app.send(f2);
    }
    await until(() => h.link.status === "auth_failed", mode);
    assert.equal(h.ports[0].closed, true);
  }
});

test("an unsealed frame after auth closes the link", async () => {
  const h = harness({ key: KEY, stored: paired });
  await h.link.start();
  await until(() => h.link.status === "ready", "ready");
  h.ports[0].app.send({ v: 1, type: "req", id: "a-1", method: "status.get", params: {} });
  await until(() => h.link.status === "auth_failed", "closed");
});

test("the app may not send events or extension methods", async () => {
  for (const frame of [
    { v: 1, type: "evt", id: "a-1", method: "stop", params: {} },
    { v: 1, type: "req", id: "a-1", method: "hello", params: {} },
    { v: 1, type: "req", id: "x-1", method: "status.get", params: {} },
  ]) {
    const h = harness({ key: KEY, stored: paired });
    await h.link.start();
    await until(() => h.link.status === "ready", "ready");
    const app = h.ports[0].app;
    app.send(await app.session.seal(frame));
    await until(() => h.link.status !== "ready", JSON.stringify(frame));
    assert.equal(h.ports[0].closed, true);
  }
});

test("E_PROTOCOL from the app is a protocol mismatch, E_NOT_PAIRED is not_paired", async () => {
  for (const [code, status] of [["E_PROTOCOL", "protocol_mismatch"], ["E_NOT_PAIRED", "not_paired"]]) {
    const h = harness({
      key: KEY,
      stored: paired,
      script: (app, f) => app.send({ v: 1, type: "res", id: f.id, ok: false, error: { code, message: "x", detail: { supported: [2, 3] } } }),
    });
    await h.link.start();
    await until(() => h.link.status === status, status);
    assert.equal(h.ports[0].closed, true);
  }
});

test("pairing: no key means no connection until a nonce arrives", async () => {
  const h = harness({ key: null, newKey: KEY, stored: undefined });
  await h.link.start();
  assert.equal(h.ports.length, 0);
  assert.equal(h.link.paired, false);
  assert.match(h.link.instanceId, /^[0-9a-f]{32}$/);
});

test("pairing: hello without mac, pair.begin once, key stored, reconnect with a normal handshake", async () => {
  const nonce = b64encode(randomBytes(16));
  const newKey = randomBytes(32);
  const h = harness({ key: newKey, newKey, stored: undefined });
  await h.link.start();
  await h.link.pair(nonce);
  await until(() => h.link.status === "ready", "ready after pairing");
  assert.equal(h.ports.length, 2);
  const first = h.ports[0].app.seen;
  assert.equal(first[0].params.paired, false);
  assert.equal(first[1].method, "pair.begin");
  assert.deepEqual(first[1].params, { nonce, instance_id: h.link.instanceId });
  assert.equal(first[1].seq, undefined);
  assert.equal(h.ports[0].closed, true, "the pairing connection is finished");
  assert.equal(h.ports[1].app.seen[0].params.paired, true);
  assert.equal(h.store.data["zptah.link"].key, b64encode(newKey));
  assert.equal(h.link.paired, true);
});

test("pairing: a mac in the unpaired hello result, a bad key or an app error fail pairing and store nothing", async () => {
  const nonce = b64encode(randomBytes(16));
  const cases = {
    "mac while pairing": (app, f) => app.send({ v: 1, type: "res", id: f.id, ok: true, result: { protocol: 1, app_version: "2", challenge: V.app_challenge, mac: V.app_mac } }),
    "bad key": (app, f) => app.send(f.method === "hello"
      ? { v: 1, type: "res", id: f.id, ok: true, result: { protocol: 1, app_version: "2", challenge: V.app_challenge } }
      : { v: 1, type: "res", id: f.id, ok: true, result: { key: "AAAA" } }),
    "E_NOT_PAIRED": (app, f) => app.send({ v: 1, type: "res", id: f.id, ok: false, error: { code: "E_NOT_PAIRED", message: "x" } }),
  };
  for (const [name, script] of Object.entries(cases)) {
    const h = harness({ key: KEY, stored: undefined, script });
    await h.link.start();
    await h.link.pair(nonce);
    await until(() => h.link.status === "pair_failed", name);
    assert.equal(h.link.paired, false, name);
    assert.equal(h.store.data["zptah.link"].key, null, name);
    assert.equal(h.ports.length, 1, `${name}: no retry`);
  }
});

test("pairing refuses a nonce that is not 16 bytes of base64", async () => {
  const h = harness({ key: KEY, stored: undefined });
  await assert.rejects(h.link.pair("not-base64!"));
  assert.equal(h.ports.length, 0);
});

test("missing host: reconnect backs off exponentially instead of connecting on every tick", async () => {
  const h = harness({ key: KEY, stored: paired, missingHost: true });
  await h.link.start();
  assert.equal(h.link.status, "host_missing");
  assert.deepEqual(h.timers.map((t) => t.ms), [1000]);
  assert.equal(h.link.attempts, 1);
  // Ticks (the 20 s timer and the alarm) before the due time do nothing.
  await h.link.tick();
  await h.link.tick();
  assert.equal(h.link.attempts, 1);
  for (const want of [2000, 4000, 8000]) {
    h.advance(10 * 60 * 1000);
    await h.link.tick();
    assert.equal(h.timers.at(-1).ms, want);
  }
  assert.equal(h.link.attempts, 4);
});

test("backoff resets after a successful handshake", async () => {
  const h = harness({ key: KEY, stored: paired });
  await h.link.start();
  await until(() => h.link.status === "ready", "ready");
  assert.equal(h.link.attempts, 0);
  h.ports[0].drop(null);
  assert.equal(h.link.status, "disconnected");
  assert.deepEqual(h.timers.map((t) => t.ms), [1000]);
  h.advance(1000);
  await h.link.tick();
  await until(() => h.link.status === "ready", "ready again");
  assert.equal(h.ports.length, 2);
});

test("backoff delays double and cap at five minutes", () => {
  assert.deepEqual([0, 1, 2, 3, 4].map(backoffDelay), [1000, 2000, 4000, 8000, 16000]);
  assert.equal(backoffDelay(9), BACKOFF_MAX_MS);
  assert.equal(backoffDelay(500), BACKOFF_MAX_MS);
});

test("store creates a stable instance id once and keeps the key", async () => {
  const s = memStore(undefined);
  const a = await s.load();
  const b = await s.load();
  assert.equal(a.instanceId, b.instanceId);
  assert.equal(a.key, null);
  await s.saveKey(V.key);
  assert.deepEqual(await s.load(), { instanceId: a.instanceId, key: V.key });
  assert.equal(toHex(randomBytes(16)).length, 32);
});

test("storage access is restricted to trusted contexts before any read or write", async () => {
  const calls = [];
  const local = {
    setAccessLevel: async (o) => calls.push(`access:${o.accessLevel}`),
    get: async (k) => (calls.push("get"), {}),
    set: async () => void calls.push("set"),
  };
  const store = createStore(trustedArea(local));
  await store.load();
  assert.deepEqual(calls.slice(0, 2), ["access:TRUSTED_CONTEXTS", "get"]);
  assert.equal(calls.indexOf("set") > 0, true);
});
