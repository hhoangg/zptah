// @ts-nocheck - rejects() predicates take untyped errors.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { b64decodeFixed, b64encode, utf8 } from "../src/worker/bytes.js";
import {
  appMac, AuthError, DIR_APP_TO_EXT, DIR_EXT_TO_APP, ExtHandshake, extMac, frameMacInput, Session, sessionKey,
} from "../src/worker/handshake.js";
import { hmac } from "../src/worker/crypto.js";

const V = JSON.parse(readFileSync(new URL("../shared/handshake-vectors.json", import.meta.url), "utf8"));
const key = b64decodeFixed(V.key, 32);
const wrongKey = b64decodeFixed(V.wrong_key, 32);
const extChal = b64decodeFixed(V.ext_challenge, 32);
const appChal = b64decodeFixed(V.app_challenge, 32);
const id = V.instance_id;
const info = { extVersion: "1.0.0", browser: { brand: "Chrome", version: "120" } };

test("app mac, ext mac and session key match the vectors", async () => {
  assert.equal(b64encode(await appMac(key, extChal, appChal, id)), V.app_mac);
  assert.equal(b64encode(await extMac(key, extChal, appChal, id)), V.ext_mac);
  assert.equal(b64encode(await sessionKey(key, extChal, appChal)), V.session_key);
  assert.equal(b64encode(await appMac(wrongKey, extChal, appChal, id)), V.wrong_key_app_mac);
  assert.notEqual(V.wrong_key_app_mac, V.app_mac);
});

test("ExtHandshake reproduces the vector auth mac and accepts the vector app mac", async () => {
  const hs = new ExtHandshake({ key, instanceId: id, challenge: extChal });
  const hello = hs.hello(info);
  assert.equal(hello.paired, true);
  assert.equal(hello.challenge, V.ext_challenge);
  assert.deepEqual(hello.protocol, [1, 1]);
  const auth = await hs.onHelloResult({ challenge: V.app_challenge, mac: V.app_mac });
  assert.equal(auth.mac, V.ext_mac);
  const session = await hs.session();
  assert.ok(session instanceof Session);
});

test("a wrong key fails on both sides", async () => {
  // We hold the real key but the app proved with another one.
  const a = new ExtHandshake({ key, instanceId: id, challenge: extChal });
  a.hello(info);
  await assert.rejects(a.onHelloResult({ challenge: V.app_challenge, mac: V.wrong_key_app_mac }), (e) => e instanceof AuthError && e.kind === "bad_mac");
  // We hold another key but the app proved with the real one.
  const b = new ExtHandshake({ key: wrongKey, instanceId: id, challenge: extChal });
  b.hello(info);
  await assert.rejects(b.onHelloResult({ challenge: V.app_challenge, mac: V.app_mac }), (e) => e instanceof AuthError && e.kind === "bad_mac");
});

test("a missing, truncated or tampered app mac is refused while paired", async () => {
  const tampered = b64encode(Uint8Array.from(b64decodeFixed(V.app_mac, 32), (b, i) => (i === 0 ? b ^ 1 : b)));
  for (const mac of [undefined, "", "AAAA", tampered, 5]) {
    const hs = new ExtHandshake({ key, instanceId: id, challenge: extChal });
    hs.hello(info);
    await assert.rejects(hs.onHelloResult({ challenge: V.app_challenge, mac }), AuthError, String(mac));
  }
});

test("the instance id is bound into both macs", async () => {
  const other = "0".repeat(32);
  const hs = new ExtHandshake({ key, instanceId: other, challenge: extChal });
  hs.hello(info);
  await assert.rejects(hs.onHelloResult({ challenge: V.app_challenge, mac: V.app_mac }), AuthError);
});

test("bad challenge, out-of-order steps and reuse are refused", async () => {
  const hs = new ExtHandshake({ key, instanceId: id, challenge: extChal });
  await assert.rejects(hs.onHelloResult({ challenge: V.app_challenge, mac: V.app_mac }), (e) => e.kind === "out_of_order");
  const hs2 = new ExtHandshake({ key, instanceId: id, challenge: extChal });
  hs2.hello(info);
  await assert.rejects(hs2.onHelloResult({ challenge: "AAAA", mac: V.app_mac }), (e) => e.kind === "invalid");
  const hs3 = new ExtHandshake({ key, instanceId: id, challenge: extChal });
  hs3.hello(info);
  await hs3.onHelloResult({ challenge: V.app_challenge, mac: V.app_mac });
  await assert.rejects(hs3.onHelloResult({ challenge: V.app_challenge, mac: V.app_mac }), (e) => e.kind === "out_of_order");
  await assert.rejects(new ExtHandshake({ key, instanceId: id }).session(), (e) => e.kind === "out_of_order");
});

test("pairing: hello says paired false and the result must carry no mac", async () => {
  const hs = new ExtHandshake({ key: null, instanceId: id, challenge: extChal });
  assert.equal(hs.hello(info).paired, false);
  hs.checkUnpairedHello({ challenge: V.app_challenge });
  const hs2 = new ExtHandshake({ key: null, instanceId: id, challenge: extChal });
  hs2.hello(info);
  assert.throws(() => hs2.checkUnpairedHello({ challenge: V.app_challenge, mac: V.app_mac }), AuthError);
  const hs3 = new ExtHandshake({ key: null, instanceId: id, challenge: extChal });
  hs3.hello(info);
  await assert.rejects(hs3.onHelloResult({ challenge: V.app_challenge }), AuthError);
});

test("every frame vector: MAC input, seal and open in order", async () => {
  const sk = b64decodeFixed(V.session_key, 32);
  const ext = new Session(sk, DIR_EXT_TO_APP);
  const app = new Session(sk, DIR_APP_TO_EXT);
  for (const f of V.frames) {
    const { mac: _m, ...unsigned } = { ...f.envelope };
    // The canonical text the vector committed to.
    const input = frameMacInput(f.direction, unsigned);
    assert.equal(new TextDecoder().decode(input.slice(9)), f.canonical);
    assert.equal(input[0], f.direction);
    assert.equal(new DataView(input.buffer).getBigUint64(1, false), BigInt(f.envelope.seq));
    assert.equal(b64encode(await hmac(sk, input)), f.mac);

    const sender = f.direction === 1 ? ext : app;
    const receiver = f.direction === 1 ? app : ext;
    const sealed = await sender.seal(f.envelope);
    assert.equal(sealed.mac, f.mac);
    assert.equal(sealed.seq, f.envelope.seq);
    await receiver.open({ ...f.envelope, mac: f.mac });
  }
});

/** A fresh pair of sessions at sequence zero. */
async function sessions() {
  const sk = b64decodeFixed(V.session_key, 32);
  return { ext: new Session(sk, DIR_EXT_TO_APP), app: new Session(sk, DIR_APP_TO_EXT) };
}
const req = (n) => ({ v: 1, type: "req", id: `a-${n}`, method: "status.get", mono: "0123456789abcdef0123456789abcdef", params: {} });

test("frames: tampering, wrong direction and bad macs fail", async () => {
  const { ext, app } = await sessions();
  const good = await app.seal(req(1));
  await assert.rejects(ext.open({ ...good, params: { x: 1 } }), (e) => e.kind === "bad_frame_mac");
  await assert.rejects(ext.open({ ...good, mac: "AAAA" }), (e) => e.kind === "bad_frame_mac");
  await assert.rejects(ext.open({ ...good, id: "a-2" }), (e) => e.kind === "bad_frame_mac");
  // Our own frame reflected back is rejected: wrong direction byte.
  const mine = await ext.seal({ v: 1, type: "evt", id: "x-1", method: "stop", params: {} });
  await assert.rejects(ext.open(mine), (e) => e.kind === "bad_frame_mac");
  // The untouched frame still opens, because failures do not change state.
  await ext.open(good);
});

test("frames: seq must be exactly previous plus one", async () => {
  const { ext, app } = await sessions();
  const f1 = await app.seal(req(1));
  const f2 = await app.seal(req(2));
  const f3 = await app.seal(req(3));
  await assert.rejects(ext.open(f2), (e) => e.kind === "replay", "skipping ahead");
  await ext.open(f1);
  await assert.rejects(ext.open(f1), (e) => e.kind === "replay", "replay");
  await assert.rejects(ext.open(f3), (e) => e.kind === "replay", "gap");
  await ext.open(f2);
  await ext.open(f3);
});

test("frames: missing seq or mac, or a float, is refused", async () => {
  const { ext, app } = await sessions();
  const good = await app.seal(req(1));
  const { mac: _m, ...noMac } = good;
  const { seq: _s, ...noSeq } = good;
  await assert.rejects(ext.open(noMac), (e) => e.kind === "replay");
  await assert.rejects(ext.open(noSeq), (e) => e.kind === "replay");
  await assert.rejects(ext.open({ ...good, params: { n: 1.5 } }));
});

test("seal numbers frames 1, 2, 3 and drops a stale mac", async () => {
  const { ext } = await sessions();
  for (let i = 1; i <= 3; i += 1) {
    const out = await ext.seal({ v: 1, type: "evt", id: `x-${i}`, method: "stop", params: {}, mac: "stale" });
    assert.equal(out.seq, i);
    assert.notEqual(out.mac, "stale");
  }
});

test("utf8 helper encodes the domain strings as bytes", () => {
  assert.deepEqual([...utf8("é")], [0xc3, 0xa9]);
});
