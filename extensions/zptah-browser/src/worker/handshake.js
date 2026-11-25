// Extension side of the zptah Browser link handshake (spec 5.3). Mirrors auth.rs; the shared
// vectors in shared/handshake-vectors.json pin every formula.
import { b64decodeFixed, b64encode, concatBytes, utf8 } from "./bytes.js";
import { canonicalJson } from "./canonical.js";
import { hkdf, hmac, hmacVerify, randomBytes } from "./crypto.js";

export const KEY_LEN = 32;
export const CHALLENGE_LEN = 32;
export const NONCE_LEN = 16;
export const DIR_EXT_TO_APP = 1;
export const DIR_APP_TO_EXT = 2;

const APP_DOMAIN = utf8("zptah-browser/1|app|");
const EXT_DOMAIN = utf8("zptah-browser/1|ext|");
const SESSION_INFO = utf8("zptah-browser/1 session");

/** A failed authentication step. `code` is the wire error code it maps to. */
export class AuthError extends Error {
  /**
   * @param {"bad_mac" | "bad_frame_mac" | "replay" | "out_of_order" | "invalid"} kind
   * @param {string} message
   */
  constructor(kind, message) {
    super(message);
    this.kind = kind;
  }
}

/** @param {Uint8Array} key @param {Uint8Array} ext @param {Uint8Array} app @param {string} instanceId */
export const appMac = (key, ext, app, instanceId) => hmac(key, APP_DOMAIN, ext, app, utf8(instanceId));
/** @param {Uint8Array} key @param {Uint8Array} ext @param {Uint8Array} app @param {string} instanceId */
export const extMac = (key, ext, app, instanceId) => hmac(key, EXT_DOMAIN, app, ext, utf8(instanceId));

/** @param {Uint8Array} key @param {Uint8Array} ext @param {Uint8Array} app */
export const sessionKey = (key, ext, app) => hkdf(key, concatBytes(ext, app), SESSION_INFO, KEY_LEN);

/**
 * The bytes a frame MAC covers: direction byte, seq as u64 big endian, canonical JSON of the
 * envelope without `mac`.
 * @param {number} direction
 * @param {Record<string, unknown>} envelope
 */
export function frameMacInput(direction, envelope) {
  const { seq, mac: _mac, ...rest } = envelope;
  if (typeof seq !== "number" || !Number.isSafeInteger(seq) || seq < 1) {
    throw new AuthError("replay", "frame has no valid seq");
  }
  const head = new Uint8Array(9);
  head[0] = direction;
  new DataView(head.buffer).setBigUint64(1, BigInt(seq), false);
  return concatBytes(head, utf8(canonicalJson({ ...rest, seq })));
}

/**
 * An authenticated link. `seal` stamps seq and mac on outgoing frames; `open` checks incoming
 * ones. A failed `open` means the link must be closed.
 */
export class Session {
  #key;
  #local;
  #sent = 0;
  #received = 0;

  /** @param {Uint8Array} key session key S @param {number} local direction of frames we send */
  constructor(key, local) {
    this.#key = key;
    this.#local = local;
  }

  /**
   * Returns a copy of the envelope with `seq` (1, 2, ...) and `mac`. Call in send order.
   * @param {Record<string, unknown>} envelope
   * @returns {Promise<Record<string, unknown>>}
   */
  async seal(envelope) {
    const seq = ++this.#sent;
    /** @type {Record<string, unknown>} */
    const unsigned = { ...envelope, seq };
    delete unsigned.mac;
    const tag = await hmac(this.#key, frameMacInput(this.#local, unsigned));
    return { ...unsigned, mac: b64encode(tag) };
  }

  /**
   * MAC first, then the counter, which must be exactly the next one. State changes only on
   * success. Call in receive order.
   * The MAC covers the received object as is (every field except `mac`), so the app must send
   * exactly its serde envelope; the Rust side signs the parsed Envelope, which drops unknown
   * fields, and it only ever sends known ones.
   * @param {Record<string, unknown>} envelope
   */
  async open(envelope) {
    const { seq, mac } = envelope;
    if (typeof seq !== "number" || typeof mac !== "string") throw new AuthError("replay", "frame has no seq or mac");
    const opposite = this.#local === DIR_EXT_TO_APP ? DIR_APP_TO_EXT : DIR_EXT_TO_APP;
    const input = frameMacInput(opposite, envelope);
    let tag;
    try {
      tag = b64decodeFixed(mac, 32);
    } catch {
      throw new AuthError("bad_frame_mac", "frame authentication failed");
    }
    if (!(await hmacVerify(this.#key, tag, input))) throw new AuthError("bad_frame_mac", "frame authentication failed");
    if (seq !== this.#received + 1) throw new AuthError("replay", "frame replayed or out of order");
    this.#received = seq;
  }
}

/**
 * One connection's handshake. Single use: build a new one for every connection.
 * Paired: hello -> onHelloResult (verifies the app, returns auth params) -> session().
 * Pairing (no key yet): hello -> checkUnpairedHello; the key arrives in the pair.begin result.
 */
export class ExtHandshake {
  #key;
  #instanceId;
  #challenge;
  /** @type {Uint8Array | null} */
  #app = null;
  #state = "fresh";

  /**
   * @param {{ key: Uint8Array | null, instanceId: string, challenge?: Uint8Array }} init
   */
  constructor({ key, instanceId, challenge }) {
    this.#key = key;
    this.#instanceId = instanceId;
    this.#challenge = challenge ?? randomBytes(CHALLENGE_LEN);
  }

  /**
   * @param {{ extVersion: string, browser: { brand: string, version: string }, protocol?: [number, number] }} info
   */
  hello(info) {
    if (this.#state !== "fresh") throw new AuthError("out_of_order", "hello already sent");
    this.#state = "sent";
    return {
      protocol: info.protocol ?? [1, 1],
      ext_version: info.extVersion,
      browser: info.browser,
      instance_id: this.#instanceId,
      paired: this.#key !== null,
      challenge: b64encode(this.#challenge),
    };
  }

  /**
   * Checks the app's MAC and answers with ours.
   * @param {{ challenge?: unknown, mac?: unknown }} result
   * @returns {Promise<{ mac: string }>}
   */
  async onHelloResult(result) {
    if (this.#state !== "sent" || this.#key === null) {
      this.#state = "done";
      throw new AuthError("out_of_order", "handshake step out of order");
    }
    this.#state = "done";
    const app = decodeChallenge(result.challenge);
    if (typeof result.mac !== "string") throw new AuthError("bad_mac", "authentication failed");
    let mac;
    try {
      mac = b64decodeFixed(result.mac, 32);
    } catch {
      throw new AuthError("bad_mac", "authentication failed");
    }
    const ok = await hmacVerify(this.#key, mac, APP_DOMAIN, this.#challenge, app, utf8(this.#instanceId));
    if (!ok) throw new AuthError("bad_mac", "authentication failed");
    this.#app = app;
    this.#state = "authed";
    return { mac: b64encode(await extMac(this.#key, this.#challenge, app, this.#instanceId)) };
  }

  /**
   * While pairing the app has no key to prove: the result must carry a challenge and no mac.
   * @param {{ challenge?: unknown, mac?: unknown }} result
   */
  checkUnpairedHello(result) {
    if (this.#state !== "sent" || this.#key !== null) {
      this.#state = "done";
      throw new AuthError("out_of_order", "handshake step out of order");
    }
    this.#state = "done";
    decodeChallenge(result.challenge);
    if (result.mac !== undefined) throw new AuthError("invalid", "unexpected mac while pairing");
  }

  /** The extension side of the session; call once the app accepted `auth`. */
  async session() {
    if (this.#state !== "authed" || this.#key === null || this.#app === null) {
      throw new AuthError("out_of_order", "handshake step out of order");
    }
    this.#state = "session";
    return new Session(await sessionKey(this.#key, this.#challenge, this.#app), DIR_EXT_TO_APP);
  }
}

/** @param {unknown} text */
function decodeChallenge(text) {
  try {
    return b64decodeFixed(text, CHALLENGE_LEN);
  } catch {
    throw new AuthError("invalid", "`challenge` is not 32 bytes of base64");
  }
}
