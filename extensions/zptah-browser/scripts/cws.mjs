// @ts-nocheck - dependency-free CLI script; covered by its test file.
// Chrome Web Store API v2 client for the extension release (spec 13.6): fetchStatus, upload,
// publish. No dependencies. Dry-run by default: without --execute nothing is sent and no
// secret is read or printed. The workflow (extension-release.yml) runs it only in the
// protected `chrome-web-store` environment, after a human approved the job.
//
//   node scripts/cws.mjs status  [--execute]
//   node scripts/cws.mjs upload  --zip FILE [--execute]
//   node scripts/cws.mjs publish [--publish-type STAGED_PUBLISH|DEFAULT_PUBLISH] [--execute]
//   node scripts/cws.mjs release --zip FILE [--publish-type ...] [--execute]   (upload, then publish)
//
// Environment (names only; values are never logged):
//   CWS_PUBLISHER_ID, CWS_ITEM_ID                          always
//   CWS_SERVICE_ACCOUNT_KEY                                service account JSON key (preferred)
//   or CWS_CLIENT_ID, CWS_CLIENT_SECRET, CWS_REFRESH_TOKEN  OAuth refresh token
import { createHash, createSign } from "node:crypto";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

export const API = "https://chromewebstore.googleapis.com";
export const SCOPE = "https://www.googleapis.com/auth/chromewebstore";
export const TOKEN_URL = "https://oauth2.googleapis.com/token";
export const PUBLISH_TYPES = ["STAGED_PUBLISH", "DEFAULT_PUBLISH"];

const b64url = (buf) => Buffer.from(buf).toString("base64url");

/** @param {Record<string, string | undefined>} env */
export function loadConfig(env) {
  const missing = [];
  for (const name of ["CWS_PUBLISHER_ID", "CWS_ITEM_ID"]) if (!env[name]) missing.push(name);
  let auth;
  if (env.CWS_SERVICE_ACCOUNT_KEY) {
    let key;
    try {
      key = JSON.parse(env.CWS_SERVICE_ACCOUNT_KEY);
    } catch {
      throw new Error("CWS_SERVICE_ACCOUNT_KEY is not valid JSON");
    }
    if (!key.client_email || !key.private_key) throw new Error("CWS_SERVICE_ACCOUNT_KEY needs client_email and private_key");
    auth = { kind: "service_account", email: key.client_email, privateKey: key.private_key, tokenUri: key.token_uri || TOKEN_URL };
  } else if (env.CWS_CLIENT_ID && env.CWS_CLIENT_SECRET && env.CWS_REFRESH_TOKEN) {
    auth = { kind: "oauth", clientId: env.CWS_CLIENT_ID, clientSecret: env.CWS_CLIENT_SECRET, refreshToken: env.CWS_REFRESH_TOKEN };
  } else {
    missing.push("CWS_SERVICE_ACCOUNT_KEY (or CWS_CLIENT_ID, CWS_CLIENT_SECRET and CWS_REFRESH_TOKEN)");
  }
  if (missing.length > 0) throw new Error(`missing environment: ${missing.join(", ")}`);
  return { publisherId: env.CWS_PUBLISHER_ID, itemId: env.CWS_ITEM_ID, auth };
}

/** Signed JWT assertion for the service account flow. */
export function serviceAccountAssertion(auth, nowSec) {
  const head = b64url(JSON.stringify({ alg: "RS256", typ: "JWT" }));
  const claims = b64url(
    JSON.stringify({ iss: auth.email, scope: SCOPE, aud: auth.tokenUri, iat: nowSec, exp: nowSec + 3600 }),
  );
  const sig = createSign("RSA-SHA256").update(`${head}.${claims}`).sign(auth.privateKey);
  return `${head}.${claims}.${b64url(sig)}`;
}

async function readJson(res, what) {
  const text = await res.text();
  if (!res.ok) {
    // The body of an API error holds no secret; the request headers are never printed.
    throw new Error(`${what} failed: HTTP ${res.status} ${text.slice(0, 500)}`);
  }
  try {
    return text ? JSON.parse(text) : {};
  } catch {
    throw new Error(`${what} returned invalid JSON`);
  }
}

/** @returns {Promise<string>} an access token */
export async function accessToken(auth, fetchImpl, nowSec = Math.floor(Date.now() / 1000)) {
  const body = new URLSearchParams(
    auth.kind === "service_account"
      ? { grant_type: "urn:ietf:params:oauth:grant-type:jwt-bearer", assertion: serviceAccountAssertion(auth, nowSec) }
      : { grant_type: "refresh_token", client_id: auth.clientId, client_secret: auth.clientSecret, refresh_token: auth.refreshToken },
  );
  const res = await fetchImpl(auth.kind === "service_account" ? auth.tokenUri : TOKEN_URL, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: body.toString(),
  });
  const json = await readJson(res, "token request");
  if (!json.access_token) throw new Error("token response has no access_token");
  return json.access_token;
}

const itemName = (cfg) => `publishers/${encodeURIComponent(cfg.publisherId)}/items/${encodeURIComponent(cfg.itemId)}`;

/** The request each command would send (also what a dry run prints). `zipBytes` may be a Buffer or a size label. */
export function describeRequest(cmd, cfg, opts = {}) {
  const name = itemName(cfg);
  if (cmd === "status") return { method: "GET", url: `${API}/v2/${name}:fetchStatus` };
  if (cmd === "upload") return { method: "POST", url: `${API}/upload/v2/${name}:upload?uploadType=media`, body: opts.zip ?? "<zip>" };
  if (cmd === "publish") {
    return { method: "POST", url: `${API}/v2/${name}:publish`, body: JSON.stringify({ publishType: opts.publishType ?? "STAGED_PUBLISH" }) };
  }
  throw new Error(`unknown command ${cmd}`);
}

async function call(fetchImpl, token, req, what) {
  const headers = { authorization: `Bearer ${token}` };
  if (req.body !== undefined && req.url.includes(":publish")) headers["content-type"] = "application/json";
  if (req.url.includes(":upload")) headers["content-type"] = "application/zip";
  const res = await fetchImpl(req.url, { method: req.method, headers, body: req.body });
  return readJson(res, what);
}

export const fetchStatus = (cfg, token, fetchImpl) => call(fetchImpl, token, describeRequest("status", cfg), "fetchStatus");

/** Uploads the zip and waits while the store reports IN_PROGRESS. */
export async function upload(cfg, token, fetchImpl, zip, { sleep = (ms) => new Promise((r) => setTimeout(r, ms)), attempts = 30, intervalMs = 5000 } = {}) {
  const first = await call(fetchImpl, token, describeRequest("upload", cfg, { zip }), "upload");
  let state = first.uploadState;
  for (let i = 0; state === "IN_PROGRESS" && i < attempts; i++) {
    await sleep(intervalMs);
    state = (await fetchStatus(cfg, token, fetchImpl)).lastAsyncUploadState;
  }
  if (state !== "SUCCEEDED") throw new Error(`upload did not succeed: state ${state ?? "unknown"}`);
  return first;
}

export function publish(cfg, token, fetchImpl, publishType = "STAGED_PUBLISH") {
  if (!PUBLISH_TYPES.includes(publishType)) throw new Error(`publish type must be one of ${PUBLISH_TYPES.join(", ")}`);
  return call(fetchImpl, token, describeRequest("publish", cfg, { publishType }), "publish");
}

/**
 * @param {string[]} argv
 * @param {Record<string, string | undefined>} env
 * @param {{ fetch?: typeof fetch, log?: (s: string) => void, sleep?: (ms: number) => Promise<void>, now?: () => number }} [deps]
 */
export async function run(argv, env, deps = {}) {
  const log = deps.log ?? console.log;
  const fetchImpl = deps.fetch ?? fetch;
  const [cmd, ...rest] = argv;
  const flags = { zip: undefined, publishType: "STAGED_PUBLISH", execute: false };
  for (let i = 0; i < rest.length; i++) {
    if (rest[i] === "--execute") flags.execute = true;
    else if (rest[i] === "--zip") flags.zip = rest[++i];
    else if (rest[i] === "--publish-type") flags.publishType = rest[++i];
    else throw new Error(`unknown argument ${rest[i]}`);
  }
  if (!["status", "upload", "publish", "release"].includes(cmd)) {
    throw new Error("usage: cws.mjs status|upload|publish|release [--zip FILE] [--publish-type TYPE] [--execute]");
  }
  if (!PUBLISH_TYPES.includes(flags.publishType)) throw new Error(`publish type must be one of ${PUBLISH_TYPES.join(", ")}`);
  const needsZip = cmd === "upload" || cmd === "release";
  if (needsZip && !flags.zip) throw new Error(`${cmd} needs --zip FILE`);

  const steps = cmd === "release" ? ["upload", "publish"] : [cmd];
  const zip = needsZip ? readFileSync(flags.zip) : undefined;

  if (!flags.execute) {
    const cfg = { publisherId: env.CWS_PUBLISHER_ID || "<CWS_PUBLISHER_ID>", itemId: env.CWS_ITEM_ID || "<CWS_ITEM_ID>" };
    log("dry run: nothing is sent (add --execute to send)");
    for (const step of steps) {
      const r = describeRequest(step, cfg, { publishType: flags.publishType });
      const size = step === "upload" && zip ? ` (${zip.length} bytes, sha256 ${createHash("sha256").update(zip).digest("hex")})` : "";
      log(`${r.method} ${r.url}${size}${step === "publish" ? ` ${r.body}` : ""}`);
    }
    return { dryRun: true };
  }

  const cfg = loadConfig(env);
  const token = await accessToken(cfg.auth, fetchImpl, deps.now ? deps.now() : undefined);
  const out = {};
  for (const step of steps) {
    if (step === "status") out.status = await fetchStatus(cfg, token, fetchImpl);
    if (step === "upload") out.upload = await upload(cfg, token, fetchImpl, zip, { sleep: deps.sleep });
    if (step === "publish") out.publish = await publish(cfg, token, fetchImpl, flags.publishType);
    log(`${step}: ${JSON.stringify(out[step])}`);
  }
  return out;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  run(process.argv.slice(2), process.env).catch((err) => {
    console.error(err instanceof Error ? err.message : err);
    process.exit(1);
  });
}
