// The link to the zptah app over the native messaging port: handshake, pairing, sealed frames
// and reconnect with backoff (spec 5.3, 12.3). The dispatcher for app requests is a registry
// that later tickets fill; until then every request is answered E_UNSUPPORTED.
//
// Wire notes shared with the Rust side (crates/monocode-core/src/mono/browser/link):
// - hello, auth and pair.begin and their responses carry no seq or mac, except that the auth
//   response is the first frame of the session: it is sealed with the app's seq 1.
// - While pairing, the hello result has no mac. pair.begin returns K once; the extension stores
//   it, closes the port and reconnects for a normal handshake.
// - seq must be exactly the previous seq plus one. Any failure closes the port.
import { backoffDelay } from "./backoff.js";
import { b64decodeFixed, b64encode } from "./bytes.js";
import { HANDSHAKE_TIMEOUT_MS, PROTOCOL } from "./config.js";
import { AuthError, ExtHandshake, KEY_LEN, NONCE_LEN } from "./handshake.js";

/**
 * @typedef {"idle" | "connecting" | "ready" | "pairing" | "protocol_mismatch" | "not_paired"
 *   | "auth_failed" | "pair_failed" | "host_missing" | "disconnected"} LinkStatus
 */

/**
 * @typedef {object} LinkDeps
 * @property {string} hostName
 * @property {(name: string) => chrome.runtime.Port} connectNative
 * @property {{ load(): Promise<{ instanceId: string, key: string | null }>, saveKey(key: string | null): Promise<void> }} store
 * @property {string} extVersion
 * @property {{ brand: string, version: string }} browser
 * @property {() => number} [now]
 * @property {(fn: () => void, ms: number) => unknown} [setTimer]
 * @property {(id: unknown) => void} [clearTimer]
 * @property {(status: LinkStatus, detail?: unknown) => void} [onStatus]
 */

/** @typedef {Record<string, unknown>} Frame */

const APP_METHODS = new Set([
  "roster.set", "status.get", "stop.resume", "tabs.list", "tab.open", "tab.close", "tab.adopt",
  "tab.release", "page.snapshot", "page.read", "page.find", "page.screenshot", "page.wait", "nav.go",
  "nav.history", "act.prepare", "act.commit", "act.extend", "act.cancel", "dialog.respond",
  "upload.put", "task.end", "cursor.prefs",
]);

/** @param {unknown} v @returns {v is Frame} */
const isObject = (v) => typeof v === "object" && v !== null && !Array.isArray(v);

export class BrowserLink {
  #deps;
  /** @type {chrome.runtime.Port | null} */
  #port = null;
  /** @type {LinkStatus} */
  #status = "idle";
  #instanceId = "";
  /** @type {string | null} */
  #keyB64 = null;
  /** @type {string | null} */
  #pairNonce = null;
  /** @type {"hello" | "auth" | "pair" | "ready"} */
  #stage = "hello";
  /** @type {ExtHandshake | null} */
  #hs = null;
  /** @type {import("./handshake.js").Session | null} */
  #session = null;
  #counter = 0;
  /** @type {string} */
  #pendingId = "";
  #attempts = 0;
  #nextAttemptAt = 0;
  #deadline = 0;
  #loaded = false;
  #inbound = Promise.resolve();
  #outbound = Promise.resolve();
  /** @type {unknown} */
  #retryTimer = null;
  /** @type {Map<string, (frame: Frame) => Promise<Frame> | Frame>} */
  handlers = new Map();
  /** @type {unknown[] | null} */
  roster = null;
  stopped = false;
  /** The app's version from the last hello, for the options page. */
  appVersion = "";

  /** @param {LinkDeps} deps */
  constructor(deps) {
    this.#deps = deps;
  }

  get status() {
    return this.#status;
  }
  get instanceId() {
    return this.#instanceId;
  }
  get paired() {
    return this.#keyB64 !== null;
  }
  get nextAttemptAt() {
    return this.#nextAttemptAt;
  }
  get attempts() {
    return this.#attempts;
  }

  #now() {
    return (this.#deps.now ?? Date.now)();
  }

  /** @param {LinkStatus} status @param {unknown} [detail] */
  #setStatus(status, detail) {
    console.debug("zptah-link", status, detail instanceof Error ? detail.message : detail ?? "");
    this.#status = status;
    this.#deps.onStatus?.(status, detail);
  }

  async load() {
    if (this.#loaded) return;
    const s = await this.#deps.store.load();
    this.#instanceId = s.instanceId;
    this.#keyB64 = s.key;
    this.#loaded = true;
  }

  /** Connects when paired. Safe to call often. */
  async start() {
    await this.load();
    await this.tick();
  }

  /** Called by the 20 s timer, the alarm and startup: reconnects when due, enforces timeouts. */
  async tick() {
    await this.load();
    const now = this.#now();
    if (this.#port) {
      if (this.#stage !== "ready" && this.#deadline > 0 && now >= this.#deadline) this.#fail(this.#pairNonce !== null ? "pair_failed" : "auth_failed", "handshake timed out", this.#pairNonce === null);
      return;
    }
    if (this.#keyB64 === null || now < this.#nextAttemptAt) return;
    this.#connect(false);
  }

  /**
   * Starts pairing with the nonce the app showed on the pair page.
   * @param {string} nonce
   */
  async pair(nonce) {
    await this.load();
    try {
      b64decodeFixed(nonce, NONCE_LEN);
    } catch {
      this.#setStatus("pair_failed", "bad nonce");
      throw new Error("bad nonce");
    }
    this.#closePort();
    this.#pairNonce = nonce;
    this.#connect(true);
  }

  /** Forgets the key (the app was told first, or the user chose Disconnect). */
  async unpair() {
    await this.load();
    this.#closePort();
    this.#keyB64 = null;
    await this.#deps.store.saveKey(null);
    this.#setStatus("idle");
  }

  /** @param {boolean} pairing */
  #connect(pairing) {
    this.#clearRetry();
    let port;
    try {
      port = this.#deps.connectNative(this.#deps.hostName);
    } catch (err) {
      this.#onGone(err instanceof Error ? err.message : null, pairing);
      return;
    }
    this.#port = port;
    this.#session = null;
    this.#counter = 0;
    this.#inbound = Promise.resolve();
    this.#outbound = Promise.resolve();
    this.#stage = "hello";
    this.#deadline = this.#now() + HANDSHAKE_TIMEOUT_MS;
    this.#setStatus(pairing ? "pairing" : "connecting");
    port.onMessage.addListener((msg) => {
      this.#inbound = this.#inbound
        .then(() => (port === this.#port ? this.#onMessage(port, msg) : undefined))
        .catch((err) => {
          if (port !== this.#port) return;
          const pairing = this.#pairNonce !== null;
          const authKind = err instanceof AuthError && err.kind !== "invalid";
          this.#fail(pairing ? "pair_failed" : authKind ? "auth_failed" : "disconnected", err, !pairing);
        });
    });
    port.onDisconnect.addListener(() => {
      const message = chrome.runtime.lastError?.message ?? null;
      if (port !== this.#port) return;
      // A ready link is gone at once: queued app requests must not run on the page after the app
      // left, and the reconnect should not wait for them. During the handshake and pairing the
      // answer may still be queued behind the disconnect: the app closes the pairing connection
      // right after its answer, and Chrome delivers the message before the disconnect, but it is
      // handled asynchronously. Handle what is queued first, or a finished pairing is reported as
      // failed and its key is lost.
      if (this.#stage === "ready") {
        this.#onGone(message, pairing);
        return;
      }
      void this.#inbound.then(() => {
        if (port === this.#port) this.#onGone(message, pairing);
      });
    });
    const key = pairing ? null : this.#keyB64 === null ? null : b64decodeFixed(this.#keyB64, KEY_LEN);
    this.#hs = new ExtHandshake({ key, instanceId: this.#instanceId });
    this.#send({
      v: PROTOCOL,
      type: "req",
      id: this.#nextId(),
      method: "hello",
      params: this.#hs.hello({ extVersion: this.#deps.extVersion, browser: this.#deps.browser }),
    }, false);
  }

  #nextId() {
    this.#counter += 1;
    this.#pendingId = `x-${this.#counter}`;
    return this.#pendingId;
  }

  /**
   * @param {Frame} frame
   * @param {boolean} sealed
   */
  #send(frame, sealed) {
    const port = this.#port;
    if (!port) return;
    const session = this.#session;
    // Sealing is async but seq is taken synchronously, so the chain keeps wire order.
    this.#outbound = this.#outbound
      .then(async () => {
        const out = sealed && session ? await session.seal(frame) : frame;
        if (port === this.#port) port.postMessage(out);
      })
      .catch((err) => {
        if (port === this.#port) this.#fail("disconnected", err);
      });
  }

  /**
   * @param {chrome.runtime.Port} port
   * @param {unknown} msg
   */
  async #onMessage(port, msg) {
    if (!isObject(msg) || msg.v !== PROTOCOL || typeof msg.id !== "string" || typeof msg.type !== "string") {
      throw new AuthError("invalid", "malformed frame");
    }
    if (this.#stage === "ready") return this.#onSealed(msg);
    // Before the session exists only one response is acceptable: the answer to our request.
    if (msg.type !== "res" || msg.id !== this.#pendingId) throw new AuthError("out_of_order", "unexpected frame");
    if (msg.ok === false) return this.#onErrorResponse(msg);
    if (msg.ok !== true || !isObject(msg.result)) throw new AuthError("invalid", "malformed response");
    const hs = this.#hs;
    if (!hs) throw new AuthError("out_of_order", "no handshake");
    if (this.#stage === "hello") {
      if (msg.result.protocol !== PROTOCOL) throw new AuthError("invalid", "unsupported protocol");
      this.appVersion = typeof msg.result.app_version === "string" ? msg.result.app_version.slice(0, 64) : "";
      if (this.#pairNonce !== null) {
        hs.checkUnpairedHello(msg.result);
        this.#stage = "pair";
        this.#send({
          v: PROTOCOL, type: "req", id: this.#nextId(), method: "pair.begin",
          params: { nonce: this.#pairNonce, instance_id: this.#instanceId },
        }, false);
      } else {
        const auth = await hs.onHelloResult(msg.result);
        this.#stage = "auth";
        this.#send({ v: PROTOCOL, type: "req", id: this.#nextId(), method: "auth", params: auth }, false);
      }
      return;
    }
    if (this.#stage === "pair") return this.#onPaired(port, msg.result);
    // stage "auth": the response is sealed with the app's seq 1.
    const session = await hs.session();
    await session.open(msg);
    const { roster, stopped } = msg.result;
    if (!Array.isArray(roster) || typeof stopped !== "boolean") throw new AuthError("invalid", "malformed auth result");
    this.#session = session;
    this.roster = roster;
    this.stopped = stopped;
    this.#stage = "ready";
    this.#deadline = 0;
    this.#attempts = 0;
    this.#nextAttemptAt = 0;
    this.#setStatus("ready");
  }

  /** @param {chrome.runtime.Port} port @param {Frame} result */
  async #onPaired(port, result) {
    let key;
    try {
      key = b64decodeFixed(result.key, KEY_LEN);
    } catch {
      throw new AuthError("invalid", "pair.begin returned a bad key");
    }
    const text = b64encode(key);
    await this.#deps.store.saveKey(text);
    this.#keyB64 = text;
    this.#pairNonce = null;
    // The pairing connection is finished; a normal handshake follows at once.
    this.#closePort(port);
    this.#attempts = 0;
    this.#nextAttemptAt = 0;
    this.#connect(false);
  }

  /** @param {Frame} msg */
  #onErrorResponse(msg) {
    const err = isObject(msg.error) ? msg.error : {};
    const code = typeof err.code === "string" ? err.code : "";
    const pairing = this.#stage === "pair" || this.#pairNonce !== null;
    this.#pairNonce = null;
    if (code === "E_PROTOCOL") this.#fail("protocol_mismatch", err.detail, false);
    else if (code === "E_NOT_PAIRED") this.#fail(pairing ? "pair_failed" : "not_paired", code, !pairing);
    else if (pairing) this.#fail("pair_failed", code, false);
    else this.#fail("auth_failed", code);
  }

  /** @param {Frame} msg */
  async #onSealed(msg) {
    const session = this.#session;
    if (!session) throw new AuthError("out_of_order", "no session");
    await session.open(msg);
    if (msg.type !== "req" || typeof msg.method !== "string" || typeof msg.id !== "string" || !msg.id.startsWith("a-") || !APP_METHODS.has(msg.method)) {
      throw new AuthError("invalid", "unexpected frame from the app");
    }
    const handler = this.handlers.get(msg.method);
    /** @type {Frame} */
    let reply;
    try {
      reply = handler
        ? await handler(msg)
        : { v: PROTOCOL, type: "res", id: msg.id, ok: false, error: { code: "E_UNSUPPORTED", message: "not implemented" } };
    } catch (err) {
      reply = { v: PROTOCOL, type: "res", id: msg.id, ok: false, error: { code: "E_INVALID", message: String(err instanceof Error ? err.message : err).slice(0, 200) } };
    }
    this.#send(reply, true);
  }

  /**
   * Sends an event once the link is ready. Returns false when it is not.
   * @param {string} method
   * @param {Frame} params
   */
  sendEvent(method, params) {
    if (this.#stage !== "ready" || !this.#session) return false;
    this.#send({ v: PROTOCOL, type: "evt", id: this.#nextId(), method, params }, true);
    return true;
  }

  /**
   * @param {LinkStatus} status
   * @param {unknown} detail
   * @param {boolean} [retry] schedule a reconnect with backoff
   */
  #fail(status, detail, retry = true) {
    this.#pairNonce = null;
    this.#closePort();
    this.#setStatus(status, detail);
    if (retry) this.#scheduleRetry();
  }

  /** @param {string | null} message @param {boolean} pairing */
  #onGone(message, pairing) {
    this.#port = null;
    this.#session = null;
    this.#hs = null;
    this.#stage = "hello";
    if (pairing || this.#pairNonce !== null) {
      this.#pairNonce = null;
      this.#setStatus("pair_failed", message);
      return;
    }
    const missing = message !== null && /not found|forbidden|not allowed/i.test(message);
    this.#setStatus(missing ? "host_missing" : "disconnected", message);
    this.#scheduleRetry();
  }

  #scheduleRetry() {
    if (this.#keyB64 === null) return;
    const delay = backoffDelay(this.#attempts);
    this.#attempts += 1;
    this.#nextAttemptAt = this.#now() + delay;
    this.#clearRetry();
    // The 20 s tick and the alarm are the backstop if the worker is stopped before this fires.
    this.#retryTimer = (this.#deps.setTimer ?? setTimeout)(() => {
      this.#retryTimer = null;
      void this.tick();
    }, delay);
  }

  #clearRetry() {
    if (this.#retryTimer !== null) (this.#deps.clearTimer ?? /** @type {(id: unknown) => void} */ (clearTimeout))(this.#retryTimer);
    this.#retryTimer = null;
  }

  /** @param {chrome.runtime.Port | null} [only] */
  #closePort(only) {
    const port = this.#port;
    if (!port || (only && port !== only)) return;
    this.#port = null;
    this.#session = null;
    this.#hs = null;
    this.#stage = "hello";
    this.#deadline = 0;
    try {
      port.disconnect();
    } catch {
      // already closed
    }
  }
}
