// @ts-nocheck - mocks and regex matches are untyped.
import assert from "node:assert/strict";
import { createVerify, generateKeyPairSync } from "node:crypto";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { accessToken, loadConfig, run, serviceAccountAssertion } from "../scripts/cws.mjs";

const { privateKey, publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
const pem = privateKey.export({ type: "pkcs8", format: "pem" });
const SA = JSON.stringify({ client_email: "ci@example.invalid", private_key: pem, token_uri: "https://token.example.invalid/t" });
const ENV = { CWS_PUBLISHER_ID: "pub1", CWS_ITEM_ID: "item1", CWS_SERVICE_ACCOUNT_KEY: SA };

/** A fetch that records calls and answers from a script keyed by "METHOD url-suffix". */
function mockFetch(answers) {
  const calls = [];
  const fn = async (url, init = {}) => {
    calls.push({ url: String(url), method: init.method, headers: init.headers, body: init.body });
    const key = Object.keys(answers).find((k) => `${init.method} ${url}`.includes(k));
    if (!key) return new Response("no answer", { status: 500 });
    const a = answers[key];
    const v = Array.isArray(a) ? a.shift() : a;
    return new Response(JSON.stringify(v.body ?? v), { status: v.status ?? 200 });
  };
  fn.calls = calls;
  return fn;
}

function withZip(fn) {
  const dir = mkdtempSync(join(tmpdir(), "cws-test-"));
  try {
    const zip = join(dir, "x.zip");
    writeFileSync(zip, "PK-not-a-real-zip");
    return fn(zip);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test("dry run is the default: no fetch, no secrets needed or printed", async () => {
  await withZip(async (zip) => {
    const f = mockFetch({});
    const lines = [];
    const res = await run(["release", "--zip", zip], {}, { fetch: f, log: (s) => lines.push(s) });
    assert.equal(res.dryRun, true);
    assert.equal(f.calls.length, 0);
    const text = lines.join("\n");
    assert.match(text, /dry run/);
    assert.match(text, /:upload\?uploadType=media/);
    assert.match(text, /:publish .*STAGED_PUBLISH/);
    assert.match(text, /sha256 [0-9a-f]{64}/);
  });
  const f = mockFetch({});
  await run(["status"], ENV, { fetch: f, log: () => {} });
  assert.equal(f.calls.length, 0);
});

test("config errors name the variable, never the value", () => {
  assert.throws(() => loadConfig({}), /CWS_PUBLISHER_ID, CWS_ITEM_ID, CWS_SERVICE_ACCOUNT_KEY/);
  assert.throws(() => loadConfig({ ...ENV, CWS_SERVICE_ACCOUNT_KEY: "{nope" }), (e) => /not valid JSON/.test(e.message) && !e.message.includes("nope"));
  const oauth = loadConfig({ CWS_PUBLISHER_ID: "p", CWS_ITEM_ID: "i", CWS_CLIENT_ID: "a", CWS_CLIENT_SECRET: "b", CWS_REFRESH_TOKEN: "c" });
  assert.equal(oauth.auth.kind, "oauth");
});

test("service account assertion is a verifiable RS256 JWT", () => {
  const cfg = loadConfig(ENV);
  const jwt = serviceAccountAssertion(cfg.auth, 1000);
  const [h, c, s] = jwt.split(".");
  assert.deepEqual(JSON.parse(Buffer.from(h, "base64url")), { alg: "RS256", typ: "JWT" });
  const claims = JSON.parse(Buffer.from(c, "base64url"));
  assert.deepEqual(claims, { iss: "ci@example.invalid", scope: "https://www.googleapis.com/auth/chromewebstore", aud: "https://token.example.invalid/t", iat: 1000, exp: 4600 });
  assert.ok(createVerify("RSA-SHA256").update(`${h}.${c}`).verify(publicKey, Buffer.from(s, "base64url")));
});

test("oauth refresh token flow posts a refresh_token grant", async () => {
  const f = mockFetch({ "oauth2.googleapis.com/token": { access_token: "tok" } });
  const t = await accessToken({ kind: "oauth", clientId: "a", clientSecret: "b", refreshToken: "c" }, f);
  assert.equal(t, "tok");
  const p = new URLSearchParams(f.calls[0].body);
  assert.equal(p.get("grant_type"), "refresh_token");
  assert.equal(p.get("refresh_token"), "c");
});

test("status calls fetchStatus with a bearer token", async () => {
  const f = mockFetch({
    "token.example.invalid": { access_token: "tok" },
    ":fetchStatus": { lastAsyncUploadState: "SUCCEEDED" },
  });
  const res = await run(["status", "--execute"], ENV, { fetch: f, log: () => {} });
  assert.equal(res.status.lastAsyncUploadState, "SUCCEEDED");
  const c = f.calls[1];
  assert.equal(c.method, "GET");
  assert.equal(c.url, "https://chromewebstore.googleapis.com/v2/publishers/pub1/items/item1:fetchStatus");
  assert.equal(c.headers.authorization, "Bearer tok");
});

test("release uploads, polls while IN_PROGRESS, then publishes staged by default", async () => {
  await withZip(async (zip) => {
    const f = mockFetch({
      "token.example.invalid": { access_token: "tok" },
      ":upload": { uploadState: "IN_PROGRESS" },
      ":fetchStatus": [{ lastAsyncUploadState: "IN_PROGRESS" }, { lastAsyncUploadState: "SUCCEEDED" }],
      ":publish": { state: "PENDING_REVIEW" },
    });
    const res = await run(["release", "--zip", zip, "--execute"], ENV, { fetch: f, log: () => {}, sleep: async () => {} });
    assert.equal(res.publish.state, "PENDING_REVIEW");
    const up = f.calls.find((c) => c.url.includes(":upload"));
    assert.equal(up.headers["content-type"], "application/zip");
    assert.equal(Buffer.from(up.body).toString(), "PK-not-a-real-zip");
    const pub = f.calls.find((c) => c.url.includes(":publish"));
    assert.deepEqual(JSON.parse(pub.body), { publishType: "STAGED_PUBLISH" });
    assert.ok(f.calls.indexOf(up) < f.calls.indexOf(pub));
  });
});

test("a failed upload stops before publish", async () => {
  await withZip(async (zip) => {
    const f = mockFetch({
      "token.example.invalid": { access_token: "tok" },
      ":upload": { uploadState: "FAILED" },
      ":publish": { state: "PENDING_REVIEW" },
    });
    await assert.rejects(run(["release", "--zip", zip, "--execute"], ENV, { fetch: f, log: () => {} }), /upload did not succeed: state FAILED/);
    assert.ok(!f.calls.some((c) => c.url.includes(":publish")));
  });
});

test("api errors and bad arguments are refused", async () => {
  const f = mockFetch({ "token.example.invalid": { access_token: "tok" }, ":fetchStatus": { status: 403, body: { error: "no" } } });
  await assert.rejects(run(["status", "--execute"], ENV, { fetch: f, log: () => {} }), /fetchStatus failed: HTTP 403/);
  await assert.rejects(run(["publish", "--publish-type", "NOW"], ENV, {}), /publish type/);
  await assert.rejects(run(["upload"], ENV, {}), /needs --zip/);
  await assert.rejects(run(["nope"], ENV, {}), /usage/);
});
